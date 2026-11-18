// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The converter, folder sync and the cover manager over invented content:
//! presets and three destinations kept in memory, plans made from the sample
//! library, jobs that "finish" at once without writing anything, and covers
//! whose groups are dealt out among the sample albums.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use helikodu_model::query::Field;
use helikodu_service::convert::{
    ConvertSummary, FieldMap, ItemOutcome, ManifestEntry, SyncItem, SyncSource,
};

use super::super::export::{
    CharacterSet, ConvertEvent, ConvertRow, CoverAction, CoverAlbum, CoverResult, CoverShow,
    EncodeFormat, ExportServices, ExportState, FolderPicture, JobEvents, JobOptions, NamingPattern,
    PlannedAction, Preset, SyncAction, SyncDestination, SyncPlan,
};
use super::super::{AlbumKey, AlbumQuery, BackendError, LibrarySource, TrackKey};
use super::SampleLibrary;
use crate::i18n::tr;

/// The state the sample tools keep for as long as the program runs.
static STATE: Mutex<Option<ExportState>> = Mutex::new(None);

fn first_state() -> ExportState {
    let mut state = ExportState::default();
    let home = gtk::glib::home_dir();
    state.converter.folder = Some(home.join("Music/Phone"));
    let mut phone = SyncDestination::new(
        "dest-1".into(),
        "Phone".into(),
        PathBuf::from("/run/media/user/PIXEL/Music"),
    );
    phone.sources = vec![
        SyncSource::Playlist { id: 1 },
        SyncSource::Playlist { id: 2 },
    ];
    phone.last_synced = Some(1_794_211_200);
    let stick = SyncDestination::new(
        "dest-2".into(),
        "Car stick".into(),
        PathBuf::from("/run/media/user/CARSTICK"),
    );
    let kitchen = SyncDestination::new(
        "dest-3".into(),
        "Kitchen player".into(),
        PathBuf::from("/mnt/kitchen/music"),
    );
    state.destinations = vec![phone, stick, kitchen];
    state
}

fn with_state<T>(f: impl FnOnce(&mut ExportState) -> T) -> T {
    let mut guard = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    f(guard.get_or_insert_with(first_state))
}

/// The tools over the sample library.
#[derive(Debug)]
pub(in crate::backend) struct SampleExport {
    pub(in crate::backend) library: Option<Arc<SampleLibrary>>,
}

impl SampleExport {
    fn library(&self) -> Result<&SampleLibrary, BackendError> {
        self.library
            .as_deref()
            .ok_or_else(|| BackendError("no library".into()))
    }
}

/// Sends the events of a job that is over at once: every row written.
fn finish_at_once(rows: Vec<(usize, u64)>, total: usize, mut events: JobEvents) {
    std::thread::spawn(move || {
        events(ConvertEvent::Started { items: total });
        let mut summary = ConvertSummary {
            total,
            ..ConvertSummary::default()
        };
        for (index, bytes) in rows {
            events(ConvertEvent::Progress {
                index,
                fraction: 1.0,
            });
            events(ConvertEvent::Done {
                index,
                outcome: ItemOutcome::Written(bytes),
            });
            summary.written += 1;
            summary.bytes += bytes;
        }
        events(ConvertEvent::Finished(summary));
    });
}

impl ExportServices for SampleExport {
    fn state(&self) -> ExportState {
        with_state(|s| s.clone())
    }

    fn save_state(&self, state: &ExportState) -> Result<(), BackendError> {
        with_state(|s| *s = state.clone());
        Ok(())
    }

    fn encoders(&self) -> Vec<(EncodeFormat, Option<String>)> {
        EncodeFormat::ALL.into_iter().map(|f| (f, None)).collect()
    }

    fn plan_conversion(
        &self,
        tracks: &[TrackKey],
        preset: &Preset,
        folder: &Path,
        pattern: &NamingPattern,
        style: CharacterSet,
        replace: bool,
    ) -> Result<Vec<ConvertRow>, BackendError> {
        let rows = self.library()?.track_rows(tracks)?;
        let format = preset.encode_format();
        Ok(rows
            .iter()
            .map(|row| {
                let values = FieldMap::default()
                    .with(Field::AlbumArtist, row.artist.clone())
                    .with(Field::Artist, row.artist.clone())
                    .with(Field::Album, row.album_title.clone())
                    .with(Field::Title, row.title.clone())
                    .with(
                        Field::Track,
                        row.number.map(|n| n.to_string()).unwrap_or_default(),
                    );
                let codec = row.format.codec.clone().unwrap_or_default().to_lowercase();
                let copy =
                    preset.takes_as_is(&codec, row.format.bitrate_kbps, row.format.sample_rate);
                let extension = if copy {
                    codec.clone()
                } else {
                    format.extension().to_owned()
                };
                let mut relative = pattern
                    .render_path(&values, style, &tr("Unknown"), &format!(".{extension}"))
                    .into_os_string();
                relative.push(".");
                relative.push(extension);
                let relative = PathBuf::from(relative);
                let seconds = row.duration.map_or(0.0, |d| d.as_secs_f64());
                let target = folder.join(&relative);
                ConvertRow {
                    track: row.key,
                    action: if !replace && target.exists() {
                        PlannedAction::AlreadyThere
                    } else if copy {
                        PlannedAction::Copy
                    } else {
                        PlannedAction::Convert
                    },
                    relative,
                    target,
                    expected_bytes: preset.expected_bytes(
                        seconds,
                        row.format.sample_rate.unwrap_or(44_100),
                        row.format.channels.unwrap_or(2),
                        u32::from(row.format.bits.unwrap_or(16)),
                    ),
                    seconds,
                }
            })
            .collect())
    }

    fn start_conversion(
        &self,
        rows: &[ConvertRow],
        _preset: &Preset,
        _options: JobOptions,
        _copy_tags: bool,
        events: JobEvents,
    ) -> Result<(), BackendError> {
        let done = rows
            .iter()
            .enumerate()
            .map(|(i, r)| (i, r.expected_bytes))
            .collect();
        finish_at_once(done, rows.len(), events);
        Ok(())
    }

    fn plan_sync(&self, destination: &SyncDestination) -> Result<SyncPlan, BackendError> {
        let library = self.library()?;
        let mut plan = SyncPlan::default();
        if destination.sources.is_empty() {
            return Ok(plan);
        }
        let albums = library.album_rows(&AlbumQuery::default(), 0, 4)?;
        let item = |action, relative: String, from: &str, to: &str, bytes| SyncItem {
            action,
            relative: PathBuf::from(relative),
            track: None,
            from_codec: from.to_owned(),
            to_format: to.to_owned(),
            bytes,
        };
        for (n, album) in albums.iter().enumerate() {
            let lossless = n % 2 == 0;
            plan.items.push(item(
                if lossless {
                    SyncAction::Convert
                } else {
                    SyncAction::Copy
                },
                format!(
                    "{}/{}/0{} {}.opus",
                    album.artist,
                    album.title,
                    n + 1,
                    album.title
                ),
                if lossless { "flac" } else { "opus" },
                if lossless { "Opus" } else { "" },
                3_500_000 + n as u64 * 400_000,
            ));
        }
        plan.items.push(item(
            SyncAction::Playlist,
            "Playlists/Late evening.m3u8".into(),
            "",
            "",
            3_000,
        ));
        plan.items.push(item(
            SyncAction::Remove,
            "Polaroid Youth/Cassette Summer/04 Rooftops.m4a".into(),
            "aac",
            "",
            8_100_000,
        ));
        plan.to_copy = 126;
        plan.to_convert = 201;
        plan.already_there = 148;
        plan.to_remove = 12;
        plan.bytes_needed = 2_900 * 1024 * 1024;
        plan.bytes_free = Some(41_200 * 1024 * 1024);
        plan.manifest.files.push(ManifestEntry {
            path: PathBuf::from("Polaroid Youth/Cassette Summer/04 Rooftops.m4a"),
            source: PathBuf::new(),
            source_size: 0,
            source_mtime: 0,
            preset: String::new(),
        });
        Ok(plan)
    }

    fn start_sync(
        &self,
        destination: &SyncDestination,
        _workers: usize,
        events: JobEvents,
    ) -> Result<(), BackendError> {
        let plan = self.plan_sync(destination)?;
        let done = plan
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| (i, item.bytes))
            .collect();
        finish_at_once(done, plan.items.len(), events);
        Ok(())
    }

    fn stop(&self) {}

    fn cover_albums(&self) -> Result<Vec<CoverAlbum>, BackendError> {
        let library = self.library()?;
        let rows = library.album_rows(&AlbumQuery::default(), 0, 10_000)?;
        let music = gtk::glib::home_dir().join("Music");
        Ok(rows
            .into_iter()
            .enumerate()
            .map(|(n, row)| {
                let folder = music.join(&row.artist).join(&row.title);
                let group = match n % 7 {
                    0 | 1 => CoverShow::NoCover,
                    2 => CoverShow::OnlyInFolder,
                    3 | 4 => CoverShow::OnlyInFiles,
                    5 => CoverShow::Small,
                    _ => CoverShow::NotSame,
                };
                let picture = match (group, n % 3) {
                    (CoverShow::NoCover, 0) | (CoverShow::OnlyInFolder, _) => Some(FolderPicture {
                        path: folder.join(if n % 2 == 0 {
                            "scan-front.png"
                        } else {
                            "AlbumArt_Large.jpg"
                        }),
                        width: 1500,
                        height: 1500,
                        bytes: 2_100_000,
                    }),
                    _ => None,
                };
                let no_cover = group == CoverShow::NoCover;
                CoverAlbum {
                    album: row.key,
                    title: row.title,
                    artist: row.artist,
                    year: row.year,
                    cover: if no_cover { None } else { row.cover },
                    cover_seed: row.cover_seed,
                    folder: Some(folder),
                    groups: vec![group],
                    picture,
                    cover_size: (!no_cover).then_some(if group == CoverShow::Small {
                        (300, 300)
                    } else {
                        (1200, 1200)
                    }),
                    files: row.track_count,
                }
            })
            .collect())
    }

    fn apply_cover(
        &self,
        album: AlbumKey,
        _action: &CoverAction,
    ) -> Result<Vec<CoverResult>, BackendError> {
        let _ = album;
        Ok(Vec::new())
    }
}
