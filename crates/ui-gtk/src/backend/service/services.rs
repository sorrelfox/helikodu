// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialogs' access to the running service: the settings store, the output
//! backends and their devices, the library folders, scans and the watcher,
//! the tags and ratings of tracks, and the facts for Supported formats.
//!
//! The tag editor opens each file with the library's tag reader and keeps the
//! stamp it had, so a file changed meanwhile is refused; saving writes the
//! files one after the other through the library service, which reads them
//! again at once, and reports each file's result. Ratings are kept in the
//! library. A CUE sheet track or a file that cannot be read opens read only
//! with what the library stored.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use helikodu_model::library::TrackId;
use helikodu_model::settings::SettingsGroup;
use helikodu_service::Service;
use helikodu_service::formats::{sqlite_version, supported_formats};
use helikodu_service::library::tag_editing::{
    BatchItem, CoverChange, EditStamp, EditableFile, FieldKey, ReadOptions, TagChanges,
    TagField as WriterField, WriteError, load_for_editing,
};
use helikodu_service::library::{LibraryEvent, ScanEvent, ScanRequest, TrackInfo, WatchSettings};
use helikodu_service::player::{
    PlayerCommand, list_outputs, resolve_backend as resolve, sound_backends,
};

use super::super::services::{
    BackendSelection, CoverEdit, DatabaseInfo, DeviceInfo, EditTrack, FileFacts, FolderState,
    FormatReport, LibraryFolder, OutputBackendKind, OutputSelection, ScanStatus, Services,
    Settings, StorageUsage, TagEdit, TagField, Versions, WriteResult,
};
use super::super::{AudioFormat, BackendError, CoverKey, TrackKey, Update};
use super::error;
use crate::i18n::{fill, tr};

/// The dialogs' access to the running service.
#[derive(Debug)]
pub(super) struct ServiceServices {
    pub(super) service: Arc<Service>,
    /// The tracks the tag editor opened: their files as they were then.
    pub(super) opened: Mutex<HashMap<TrackKey, Opened>>,
}

/// A track's file as the tag editor opened it.
#[derive(Debug, Clone)]
pub(super) struct Opened {
    /// The file.
    path: PathBuf,
    /// Its size and modification time then; a file changed since is not
    /// overwritten.
    stamp: Option<EditStamp>,
    /// Why its tags cannot be written, if they cannot.
    read_only: Option<String>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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

/// The fields of the form as the tag writer names them.
fn form_key(field: &TagField) -> FieldKey {
    FieldKey::Known(match field {
        TagField::Title => WriterField::Title,
        TagField::Artist => WriterField::Artist,
        TagField::AlbumArtist => WriterField::AlbumArtist,
        TagField::Album => WriterField::Album,
        TagField::Track => WriterField::TrackNumber,
        TagField::TrackTotal => WriterField::TrackTotal,
        TagField::Disc => WriterField::DiscNumber,
        TagField::DiscTotal => WriterField::DiscTotal,
        TagField::Year => WriterField::Date,
        TagField::Genre => WriterField::Genre,
        TagField::Composer => WriterField::Composer,
        TagField::Comment => WriterField::Comment,
        TagField::Custom(name) => return FieldKey::parse(name),
    })
}

/// Fields the form shows, or File info, and so not among the other fields.
const NOT_OTHER: &[WriterField] = &[
    WriterField::Title,
    WriterField::Artist,
    WriterField::Artists,
    WriterField::AlbumArtist,
    WriterField::AlbumArtists,
    WriterField::Album,
    WriterField::TrackNumber,
    WriterField::TrackTotal,
    WriterField::DiscNumber,
    WriterField::DiscTotal,
    WriterField::Date,
    WriterField::Genre,
    WriterField::Composer,
    WriterField::Comment,
    WriterField::ReplayGainTrackGain,
    WriterField::ReplayGainTrackPeak,
    WriterField::ReplayGainAlbumGain,
    WriterField::ReplayGainAlbumPeak,
];

/// The values of the form's fields.
type FormValues = BTreeMap<TagField, Vec<String>>;

/// The form's values and the other fields of a file opened for editing.
fn file_fields(file: &EditableFile) -> (FormValues, Vec<(String, String)>) {
    let mut fields = BTreeMap::new();
    for field in TagField::FORM {
        let mut values = file.values(&form_key(&field));
        // The individual artists, when the file has no credited artist.
        if values.is_empty() {
            let list = match field {
                TagField::Artist => Some(WriterField::Artists),
                TagField::AlbumArtist => Some(WriterField::AlbumArtists),
                _ => None,
            };
            if let Some(list) = list {
                values = file.values(&FieldKey::Known(list));
            }
        }
        values.retain(|v| !v.trim().is_empty());
        if !values.is_empty() {
            fields.insert(field, values);
        }
    }
    let mut other = Vec::new();
    for field in WriterField::ALL {
        if NOT_OTHER.contains(field) {
            continue;
        }
        let values = file.values(&FieldKey::Known(*field));
        if !values.is_empty() {
            other.push((field.picard_name().to_uppercase(), values.join("; ")));
        }
    }
    for (key, _) in &file.tags.extra {
        if FieldKey::parse(key) != FieldKey::Custom(key.clone())
            || other.iter().any(|(name, _)| name.eq_ignore_ascii_case(key))
        {
            continue;
        }
        let values: Vec<&str> = file.tags.extra_values(key).collect();
        other.push((key.clone(), values.join("; ")));
    }
    (fields, other)
}

/// The form's values from what the library stored, for a track whose file
/// cannot be opened for editing.
fn library_fields(info: &TrackInfo) -> FormValues {
    let tags = &info.tags;
    let mut fields = BTreeMap::new();
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
    fields
}

/// Why the tags of a file cannot be written.
fn write_error_text(error: &WriteError) -> String {
    match error {
        WriteError::ReadOnlyFormat => tr("The tags of this format cannot be written."),
        WriteError::ChangedSinceRead => {
            tr("The file changed since it was opened. Open it again to edit it.")
        }
        WriteError::Cancelled => tr("Not saved: the save was cancelled."),
        other => other.to_string(),
    }
}

/// `key` opened for editing: the file's own tags when it can be read, else
/// what the library stored, read only.
fn edit_track(key: TrackKey, info: &TrackInfo, file: Result<EditableFile, String>) -> EditTrack {
    let (fields, other, read_only, cover, lyrics, tag_formats, encoder) = match file {
        Ok(file) => {
            let (fields, other) = file_fields(&file);
            let read_only =
                (!file.writable).then(|| tr("The tags of this format cannot be written."));
            let encoder = [WriterField::EncoderSettings, WriterField::EncodedBy]
                .iter()
                .find_map(|f| file.values(&FieldKey::Known(*f)).into_iter().next());
            (
                fields,
                other,
                read_only,
                file.front_cover.map(|h| CoverKey(*h.as_bytes())),
                file.lyrics.clone(),
                file.tag_formats.clone(),
                encoder,
            )
        }
        Err(reason) => (
            library_fields(info),
            Vec::new(),
            Some(reason),
            None,
            None,
            Vec::new(),
            None,
        ),
    };
    let meta = std::fs::metadata(&info.path).ok();
    let audio = &info.audio;
    let lossless = audio.bit_depth.is_some();
    let entry = info.library.as_ref();
    EditTrack {
        key,
        path: info.path.clone(),
        fields,
        other,
        rating: entry.and_then(|e| e.stats.rating.map(|r| r.stars_rounded())),
        cover,
        cover_seed: key.0.wrapping_mul(0x9E37_79B9_7F4A_7C15),
        lyrics,
        read_only,
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
            encoder,
            tag_formats,
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

/// The changes of `edit` as the tag writer takes them; the rating is kept in
/// the library and is not among them.
fn tag_changes(edit: &TagEdit) -> Result<TagChanges, String> {
    let mut changes = TagChanges::new();
    for (field, values) in &edit.fields {
        let key = form_key(field);
        let values: Vec<String> = match (&key, field) {
            // Other fields are one text; a field with several values is split.
            (FieldKey::Known(known), TagField::Custom(_)) if known.is_multi_valued() => values
                .iter()
                .flat_map(|v| v.split(';'))
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
                .collect(),
            _ => values.clone(),
        };
        if values.is_empty() {
            changes.clear(key.clone());
        } else {
            changes.set(key.clone(), values);
        }
        // The list of individual artists would contradict the new credit.
        match key {
            FieldKey::Known(WriterField::Artist) => {
                changes.clear(WriterField::Artists);
            }
            FieldKey::Known(WriterField::AlbumArtist) => {
                changes.clear(WriterField::AlbumArtists);
            }
            _ => {}
        }
    }
    changes.cover = match &edit.cover {
        None => None,
        Some(CoverEdit::Remove) => Some(CoverChange::RemoveFront),
        Some(CoverEdit::Set(path)) => {
            Some(CoverChange::SetFront(std::fs::read(path).map_err(|e| {
                fill(
                    &tr("The picture {} cannot be read: {}"),
                    &[&path.display().to_string(), &e.to_string()],
                )
            })?))
        }
    };
    changes.lyrics = edit
        .lyrics
        .as_ref()
        .map(|text| (!text.trim().is_empty()).then(|| text.clone()));
    Ok(changes)
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
        let mut opened = lock(&self.opened);
        for key in tracks {
            let Some(info) = music.track_info(TrackId::new(key.0)).map_err(error)? else {
                continue;
            };
            let file = if info.sub_index != 0 {
                Err(tr(
                    "This track is part of a larger file, whose tags it shares.",
                ))
            } else {
                load_for_editing(&info.path, &ReadOptions::default())
                    .map_err(|e| fill(&tr("The file cannot be read: {}"), &[&e.to_string()]))
            };
            let track = edit_track(*key, &info, file.as_ref().map_err(Clone::clone).cloned());
            opened.insert(
                *key,
                Opened {
                    path: info.path.clone(),
                    stamp: file.as_ref().ok().map(|f| f.stamp),
                    read_only: track.read_only.clone(),
                },
            );
            found.push(track);
        }
        Ok(found)
    }

    fn write_tags(&self, edit: &TagEdit) -> Vec<WriteResult> {
        let Some(library) = self.service.library() else {
            let message = tr("Helikodu runs without a library.");
            return edit
                .tracks
                .iter()
                .map(|key| WriteResult {
                    track: *key,
                    file: String::new(),
                    error: Some(message.clone()),
                })
                .collect();
        };
        let file_name = |path: &std::path::Path| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        // Each track's file as it was opened, and what keeps it from being written.
        let opened = lock(&self.opened).clone();
        let mut results: Vec<WriteResult> = Vec::new();
        let mut items = Vec::new();
        let mut written_keys = Vec::new();
        let changes = if edit.touches_files() {
            Some(tag_changes(edit))
        } else {
            None
        };
        for key in &edit.tracks {
            let Some(Opened {
                path,
                stamp,
                read_only,
            }) = opened.get(key)
            else {
                results.push(WriteResult {
                    track: *key,
                    file: String::new(),
                    error: edit
                        .touches_files()
                        .then(|| tr("Open the track in the tag editor first.")),
                });
                continue;
            };
            if let (Some(reason), true) = (read_only, edit.touches_files()) {
                results.push(WriteResult {
                    track: *key,
                    file: file_name(path),
                    error: Some(reason.clone()),
                });
                continue;
            }
            match &changes {
                None => results.push(WriteResult {
                    track: *key,
                    file: file_name(path),
                    error: None,
                }),
                Some(Err(reason)) => results.push(WriteResult {
                    track: *key,
                    file: file_name(path),
                    error: Some(reason.clone()),
                }),
                Some(Ok(changes)) => {
                    items.push(BatchItem {
                        path: path.clone(),
                        expected: *stamp,
                        changes: changes.clone(),
                    });
                    written_keys.push(*key);
                }
            }
        }
        if !items.is_empty() {
            let settings = self.service.settings().get();
            match library.write_tags(&items, &settings, &mut |_| {}) {
                Ok(report) => {
                    let mut opened = lock(&self.opened);
                    for (key, (path, result)) in written_keys.iter().zip(&report.batch.results) {
                        let error = match result {
                            Ok(outcome) => {
                                opened.insert(
                                    *key,
                                    Opened {
                                        path: path.clone(),
                                        stamp: Some(outcome.stamp),
                                        read_only: None,
                                    },
                                );
                                None
                            }
                            Err(e) => Some(write_error_text(e)),
                        };
                        results.push(WriteResult {
                            track: *key,
                            file: file_name(path),
                            error,
                        });
                    }
                }
                Err(e) => {
                    for (key, item) in written_keys.iter().zip(&items) {
                        results.push(WriteResult {
                            track: *key,
                            file: file_name(&item.path),
                            error: Some(e.to_string()),
                        });
                    }
                }
            }
        }
        // The rating is kept in the library, for every track whose file was saved.
        if let Some(stars) = edit.rating {
            for result in results.iter_mut().filter(|r| r.error.is_none()) {
                let (reply, answer) = std::sync::mpsc::channel();
                let sent = library.rate(TrackId::new(result.track.0), stars, move |r| {
                    let _ = reply.send(r);
                });
                result.error = match sent.map(|()| answer.recv()) {
                    Ok(Ok(Ok(()))) => None,
                    Ok(Ok(Err(e))) | Err(e) => Some(e.to_string()),
                    Ok(Err(_)) => Some(tr("The library has stopped.")),
                };
            }
        }
        // In the order of the selection.
        results.sort_by_key(|r| edit.tracks.iter().position(|k| *k == r.track));
        results
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
