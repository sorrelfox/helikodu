// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialogs' access to the running service: the settings store, the output
//! backends and their devices, the library folders, scans and the watcher,
//! the tags and ratings of tracks, and the facts for Supported formats.
//!
//! Tags are read from the library. Writing them needs the library's tag
//! writer, which the service does not offer the front ends yet, so every file
//! opens read only and only the rating (kept in the library) can be saved.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use helikodu_model::library::TrackId;
use helikodu_model::settings::SettingsGroup;
use helikodu_service::Service;
use helikodu_service::formats::{sqlite_version, supported_formats};
use helikodu_service::library::{LibraryEvent, ScanEvent, ScanRequest, TrackInfo, WatchSettings};
use helikodu_service::player::{
    PlayerCommand, list_outputs, resolve_backend as resolve, sound_backends,
};

use super::super::services::{
    BackendSelection, DatabaseInfo, DeviceInfo, EditTrack, FileFacts, FolderState, FormatReport,
    LibraryFolder, OutputBackendKind, OutputSelection, ScanStatus, Services, Settings,
    StorageUsage, TagEdit, TagField, Versions, WriteResult,
};
use super::super::{AudioFormat, BackendError, CoverKey, TrackKey, Update};
use super::error;
use crate::i18n::tr;

/// The dialogs' access to the running service.
#[derive(Debug)]
pub(super) struct ServiceServices {
    pub(super) service: Arc<Service>,
}

fn date(time: SystemTime) -> Option<String> {
    let seconds = time.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    let time = gtk::glib::DateTime::from_unix_local(i64::try_from(seconds).ok()?).ok()?;
    time.format("%x").ok().map(|s| s.to_string())
}

fn date_time(time: SystemTime) -> Option<String> {
    let seconds = time.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    let time = gtk::glib::DateTime::from_unix_local(i64::try_from(seconds).ok()?).ok()?;
    time.format("%x %X").ok().map(|s| s.to_string())
}

/// The size of everything under `path`, in bytes.
fn folder_size(path: &Path) -> Option<u64> {
    let mut total = 0;
    let mut pending = vec![path.to_path_buf()];
    let mut seen = false;
    while let Some(folder) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        seen = true;
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    seen.then_some(total)
}

/// Starts the folder watcher when the settings ask for it, or stops it.
pub(super) fn follow_folders(service: &Service, initial_scan: bool) {
    let Some(library) = service.library() else {
        return;
    };
    let settings = service.settings().get();
    if settings.library.watch && !settings.library.folders.is_empty() {
        if let Err(e) = library.watch(&WatchSettings {
            library: settings.library.clone(),
            initial_scan,
        }) {
            eprintln!("helikodu: cannot watch the library folders: {e}");
        }
    } else {
        library.stop_watching();
    }
}

/// The update for a scan event, if the window shows it.
pub(super) fn scan_update(event: &LibraryEvent) -> Option<Update> {
    let status = match event {
        LibraryEvent::Scan(ScanEvent::Walking { root }) => ScanStatus {
            current: Some(root.clone()),
            ..ScanStatus::default()
        },
        LibraryEvent::Scan(ScanEvent::Compared { to_read, .. }) => ScanStatus {
            total: Some(*to_read),
            ..ScanStatus::default()
        },
        LibraryEvent::Scan(ScanEvent::Progress { done, total }) => ScanStatus {
            done: *done,
            total: Some(*total),
            ..ScanStatus::default()
        },
        LibraryEvent::ScanEnded(result) => ScanStatus {
            finished: true,
            error: result.as_ref().err().cloned(),
            ..ScanStatus::default()
        },
        _ => return None,
    };
    Some(Update::Scan(status))
}

fn edit_track(key: TrackKey, info: TrackInfo) -> EditTrack {
    let tags = &info.tags;
    let mut fields = std::collections::BTreeMap::new();
    let mut put = |field: TagField, values: Vec<String>| {
        let values: Vec<String> = values.into_iter().filter(|v| !v.is_empty()).collect();
        if !values.is_empty() {
            fields.insert(field, values);
        }
    };
    put(TagField::Title, tags.title.clone().into_iter().collect());
    put(TagField::Artist, tags.artists.clone());
    put(TagField::AlbumArtist, tags.album_artists.clone());
    put(TagField::Album, tags.album.clone().into_iter().collect());
    put(
        TagField::Track,
        tags.track_no.map(|n| n.to_string()).into_iter().collect(),
    );
    put(
        TagField::Disc,
        tags.disc_no.map(|n| n.to_string()).into_iter().collect(),
    );
    put(
        TagField::Year,
        tags.date
            .clone()
            .or_else(|| tags.year.map(|y| y.to_string()))
            .into_iter()
            .collect(),
    );
    put(TagField::Genre, tags.genres.clone());
    put(TagField::Composer, tags.composers.clone());
    let meta = std::fs::metadata(&info.path).ok();
    let audio = &info.audio;
    let lossless = audio.bit_depth.is_some();
    let entry = info.library.as_ref();
    let read_only = if info.sub_index != 0 {
        tr("This track is part of a larger file, whose tags it shares.")
    } else {
        tr(
            "Changing the tags of files comes in a later version. The rating is kept in the library.",
        )
    };
    EditTrack {
        key,
        path: info.path.clone(),
        fields,
        other: Vec::new(),
        rating: entry.and_then(|e| e.stats.rating.map(|r| r.stars_rounded())),
        cover: None::<CoverKey>,
        cover_seed: key.0.wrapping_mul(0x9E37_79B9_7F4A_7C15),
        lyrics: None,
        read_only: Some(read_only),
        facts: FileFacts {
            size: meta.as_ref().map(std::fs::Metadata::len),
            modified: meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(date_time),
            format: AudioFormat {
                codec: audio.codec.as_ref().map(|c| c.to_uppercase()),
                sample_rate: audio.sample_rate,
                bits: audio.bit_depth,
                bitrate_kbps: (!lossless)
                    .then_some(audio.bitrate.map(|b| b / 1000))
                    .flatten(),
                channels: audio.channels,
            },
            duration: audio.duration,
            encoder: None,
            tag_formats: Vec::new(),
            pictures: 0,
            track_gain_db: info.loudness.and_then(|l| l.track_gain_db),
            track_peak: info.loudness.and_then(|l| l.track_peak),
            album_gain_db: info.loudness.and_then(|l| l.album_gain_db),
            album_peak: info.loudness.and_then(|l| l.album_peak),
            added: entry.and_then(|e| date(e.added_at)),
            last_played: entry.and_then(|e| e.stats.last_played.and_then(date)),
            plays: entry.map(|e| e.stats.play_count),
            skips: entry.map(|e| e.stats.skip_count),
        },
    }
}

impl Services for ServiceServices {
    fn settings(&self) -> Arc<Settings> {
        self.service.settings().get()
    }

    fn update_settings(&self, change: &mut dyn FnMut(&mut Settings)) -> Result<(), BackendError> {
        let groups = self
            .service
            .settings()
            .update(|s| change(s))
            .map_err(error)?;
        if groups.contains(&SettingsGroup::Library) {
            follow_folders(&self.service, false);
        }
        Ok(())
    }

    fn settings_file(&self) -> Option<PathBuf> {
        Some(self.service.settings().path().to_path_buf())
    }

    fn output_backends(&self) -> Vec<OutputBackendKind> {
        sound_backends()
    }

    fn resolve_backend(&self, selection: BackendSelection) -> OutputBackendKind {
        resolve(selection)
    }

    fn devices(&self, backend: OutputBackendKind) -> Result<Vec<DeviceInfo>, BackendError> {
        list_outputs(backend).map_err(error)
    }

    fn output(&self) -> OutputSelection {
        self.service.player().status().output
    }

    fn set_output(&self, selection: OutputSelection) {
        let _ = self
            .service
            .player()
            .send(PlayerCommand::SetOutput(selection));
    }

    fn library_folders(&self) -> Result<Vec<LibraryFolder>, BackendError> {
        let settings = self.service.settings().get();
        let roots = match self.service.library() {
            Some(library) => library.library().root_states().map_err(error)?,
            None => Vec::new(),
        };
        let mut folders: Vec<LibraryFolder> = settings
            .library
            .folders
            .iter()
            .map(|path| {
                let root = roots.iter().find(|r| r.path == *path);
                let state = match root {
                    Some(r) if r.masked => FolderState::NotConnected,
                    _ if !settings.library.watch => FolderState::Manual,
                    Some(r) if r.polled => {
                        FolderState::Polled(settings.library.rescan_interval_min)
                    }
                    _ => FolderState::Watched,
                };
                LibraryFolder {
                    path: path.clone(),
                    state,
                    tracks: None,
                }
            })
            .collect();
        // Folders scanned before they were in the settings.
        for root in &roots {
            if !folders.iter().any(|f| f.path == root.path) {
                folders.push(LibraryFolder {
                    path: root.path.clone(),
                    state: if root.masked {
                        FolderState::NotConnected
                    } else {
                        FolderState::Manual
                    },
                    tracks: None,
                });
            }
        }
        Ok(folders)
    }

    fn scan(&self, folders: &[PathBuf]) -> Result<(), BackendError> {
        let Some(library) = self.service.library() else {
            return Err(BackendError(tr("Helikodu runs without a library.")));
        };
        let settings = self.service.settings().get();
        library
            .scan(ScanRequest {
                folders: folders.to_vec(),
                exclude: settings.library.skip.clone(),
                ..ScanRequest::default()
            })
            .map_err(error)
    }

    fn database(&self) -> Option<DatabaseInfo> {
        let path = self.service.library()?.library().path().to_path_buf();
        let size = std::fs::metadata(&path).ok().map(|m| m.len());
        Some(DatabaseInfo { path, size })
    }

    fn storage(&self) -> StorageUsage {
        let dirs = self.service.dirs();
        StorageUsage {
            covers: folder_size(&dirs.cache.join("thumbnails")),
            analysis: None,
            logs: None,
            undo: None,
        }
    }

    fn edit_tracks(&self, tracks: &[TrackKey]) -> Result<Vec<EditTrack>, BackendError> {
        let Some(library) = self.service.library() else {
            return Ok(Vec::new());
        };
        let music = library.library();
        let mut found = Vec::new();
        for key in tracks {
            if let Some(info) = music.track_info(TrackId::new(key.0)).map_err(error)? {
                found.push(edit_track(*key, info));
            }
        }
        Ok(found)
    }

    fn write_tags(&self, edit: &TagEdit) -> Vec<WriteResult> {
        let library = self.service.library();
        let names = self.edit_tracks(&edit.tracks).unwrap_or_default();
        edit.tracks
            .iter()
            .map(|key| {
                let file = names
                    .iter()
                    .find(|t| t.key == *key)
                    .map(EditTrack::file_name)
                    .unwrap_or_default();
                let error = if edit.touches_files() {
                    Some(tr("Changing the tags of files comes in a later version."))
                } else {
                    match (library, edit.rating) {
                        (Some(library), Some(stars)) => {
                            let (reply, answer) = std::sync::mpsc::channel();
                            let sent = library.rate(TrackId::new(key.0), stars, move |r| {
                                let _ = reply.send(r);
                            });
                            match sent.map(|()| answer.recv()) {
                                Ok(Ok(Ok(()))) => None,
                                Ok(Ok(Err(e))) | Err(e) => Some(e.to_string()),
                                Ok(Err(_)) => Some(tr("The library has stopped.")),
                            }
                        }
                        _ => None,
                    }
                };
                WriteResult {
                    track: *key,
                    file,
                    error,
                }
            })
            .collect()
    }

    fn formats(&self) -> FormatReport {
        supported_formats()
    }

    fn versions(&self) -> Versions {
        Versions {
            ffmpeg: None,
            pipewire: None,
            sqlite: Some(sqlite_version().to_owned()),
        }
    }
}
