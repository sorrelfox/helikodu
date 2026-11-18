// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The converter, folder sync and the cover manager over the running service.
//!
//! Presets, choices and destinations live in `export.toml` beside the settings.
//! One job runs at a time, on threads of its own; it goes on when its window is
//! closed, and [`ExportServices::stop`] cancels it.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use helikodu_model::library::TrackId;
use helikodu_service::Service;
use helikodu_service::convert::{
    ExportStore, JobAction, JobItem, encoder_availability, plan_conversion, plan_sync, run_sync,
    source_tracks,
};
use helikodu_service::cover_manager::{
    self, AlbumCovers, CoverShow, WriteContext, album_covers, find_pictures, picture_at,
};
use helikodu_service::library::{AlbumId, MusicLibrary};

use super::super::export::{
    ConvertRow, CoverAction, CoverAlbum, EncodeFormat, ExportServices, ExportState, JobEvents,
    JobOptions, NameStyle, NamingPattern, Preset, SyncDestination, SyncPlan,
};
use super::super::{AlbumKey, BackendError, CoverKey, TrackKey};
use crate::i18n::tr;

/// The cancel switch of the job that runs, if one does.
static RUNNING: Mutex<Option<helikodu_service::library::CancelToken>> = Mutex::new(None);

fn running() -> std::sync::MutexGuard<'static, Option<helikodu_service::library::CancelToken>> {
    RUNNING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The tools over the running service.
#[derive(Debug, Clone)]
pub(in crate::backend) struct ServiceExport {
    pub(in crate::backend) service: Arc<Service>,
}

fn error(e: impl std::fmt::Display) -> BackendError {
    BackendError(e.to_string())
}

impl ServiceExport {
    fn store(&self) -> ExportStore {
        ExportStore::in_folder(&self.service.dirs().config)
    }

    fn music(&self) -> Result<MusicLibrary, BackendError> {
        self.service
            .library()
            .map(|l| l.library().clone())
            .ok_or_else(|| BackendError(tr("Helikodu runs without a library.")))
    }

    fn presets(&self) -> Vec<Preset> {
        self.store().load().all_presets()
    }

    /// Runs `work` on a thread of its own as the one job, cancelled by `stop`.
    fn spawn(
        &self,
        work: impl FnOnce(&helikodu_service::library::CancelToken) + Send + 'static,
    ) -> Result<(), BackendError> {
        let token = {
            let mut running = running();
            if running.is_some() {
                return Err(BackendError(tr(
                    "Another conversion or sync is running. Stop it first.",
                )));
            }
            let token = helikodu_service::library::CancelToken::new();
            *running = Some(token.clone());
            token
        };
        std::thread::Builder::new()
            .name("helikodu-export".into())
            .spawn(move || {
                work(&token);
                running().take();
            })
            .map(drop)
            .map_err(|e| {
                running().take();
                error(e)
            })
    }

    fn album(&self, music: &MusicLibrary, key: AlbumKey) -> Result<AlbumCovers, BackendError> {
        let id = AlbumId::from_raw(i64::try_from(key.0).unwrap_or(-1));
        album_covers(music)
            .map_err(error)?
            .into_iter()
            .find(|a| a.album == id)
            .ok_or_else(|| BackendError(tr("The album is gone.")))
    }
}

fn track_id(key: TrackKey) -> TrackId {
    TrackId::new(key.0)
}

fn album_key(id: AlbumId) -> AlbumKey {
    AlbumKey(u64::try_from(id.get()).unwrap_or(0))
}

fn seed(key: AlbumKey) -> u64 {
    key.0.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17)
}

/// The album as the cover manager lists it; `look` finds pictures in the
/// folders of albums without a cover.
pub(in crate::backend) fn cover_album(album: &AlbumCovers, look: bool) -> CoverAlbum {
    let key = album_key(album.album);
    let groups = CoverShow::ALL
        .into_iter()
        .filter(|g| *g != CoverShow::All && album.is_in(*g))
        .collect::<Vec<_>>();
    let folder = album.folder().map(Path::to_path_buf);
    let picture = match (&album.facts.folder_image, &album.facts.folder_cover) {
        (Some(path), Some(info)) => Some(cover_manager::FolderPicture {
            path: path.clone(),
            width: info.width.unwrap_or(0),
            height: info.height.unwrap_or(0),
            bytes: info.byte_len.unwrap_or(0),
        }),
        (Some(path), None) => picture_at(path),
        (None, _) if look && groups.contains(&CoverShow::NoCover) => folder
            .as_deref()
            .and_then(|f| find_pictures(f, None).into_iter().next()),
        _ => None,
    };
    CoverAlbum {
        album: key,
        title: album.title.clone().unwrap_or_default(),
        artist: album.artist.clone().unwrap_or_default(),
        year: album.year,
        cover: album.cover.as_ref().map(|h| CoverKey(*h.as_bytes())),
        cover_seed: seed(key),
        folder,
        groups,
        picture,
        cover_size: album
            .facts
            .cover
            .as_ref()
            .and_then(|c| Some((c.width?, c.height?))),
        files: album.facts.files.len(),
    }
}

impl ExportServices for ServiceExport {
    fn state(&self) -> ExportState {
        self.store().load()
    }

    fn save_state(&self, state: &ExportState) -> Result<(), BackendError> {
        self.store().save(state).map_err(error)
    }

    fn encoders(&self) -> Vec<(EncodeFormat, Option<String>)> {
        EncodeFormat::ALL
            .into_iter()
            .map(|format| {
                let reason = match encoder_availability(format) {
                    helikodu_service::convert::EncoderAvailability::Available { .. } => None,
                    _ => Some(tr("The FFmpeg on this computer has no encoder for it.")),
                };
                (format, reason)
            })
            .collect()
    }

    fn plan_conversion(
        &self,
        tracks: &[TrackKey],
        preset: &Preset,
        folder: &Path,
        pattern: &NamingPattern,
        style: NameStyle,
        replace: bool,
    ) -> Result<Vec<ConvertRow>, BackendError> {
        let music = self.music()?;
        let ids: Vec<TrackId> = tracks.iter().copied().map(track_id).collect();
        let sources = source_tracks(&music, &ids).map_err(error)?;
        Ok(
            plan_conversion(&sources, preset, folder, pattern, style, replace)
                .into_iter()
                .map(|item| ConvertRow {
                    track: TrackKey(item.track.track.get()),
                    seconds: item.track.duration().map_or(0.0, |d| d.as_secs_f64()),
                    relative: item.relative,
                    target: item.target,
                    action: item.action,
                    expected_bytes: item.expected_bytes,
                })
                .collect(),
        )
    }

    fn start_conversion(
        &self,
        rows: &[ConvertRow],
        preset: &Preset,
        options: JobOptions,
        copy_tags: bool,
        events: JobEvents,
    ) -> Result<(), BackendError> {
        let music = self.music()?;
        let ids: Vec<TrackId> = rows.iter().map(|r| track_id(r.track)).collect();
        let sources = source_tracks(&music, &ids).map_err(error)?;
        let settings = preset.settings();
        let items: Vec<JobItem> = rows
            .iter()
            .filter_map(|row| {
                let source = sources.iter().find(|s| s.track == track_id(row.track))?;
                let action = match row.action {
                    helikodu_service::convert::PlannedAction::Convert => {
                        JobAction::Convert(settings)
                    }
                    helikodu_service::convert::PlannedAction::Copy => JobAction::Copy,
                    // Left in so that the indices of the rows hold; skipped as it
                    // is there.
                    helikodu_service::convert::PlannedAction::AlreadyThere => JobAction::Copy,
                };
                Some(JobItem {
                    track: Some(source.track),
                    source: source.source.clone(),
                    copy_tags,
                    tags: Some(source.tags.clone()),
                    target: row.target.clone(),
                    action,
                })
            })
            .collect();
        if items.len() != rows.len() {
            return Err(BackendError(tr("Some tracks are gone. Look again.")));
        }
        let options = JobOptions {
            copy_cover: copy_tags,
            ..options
        };
        let mut events = events;
        self.spawn(move |cancel| {
            helikodu_service::convert::job::run(Some(&music), &items, options, cancel, |e| {
                events(e);
            });
        })
    }

    fn plan_sync(&self, destination: &SyncDestination) -> Result<SyncPlan, BackendError> {
        let music = self.music()?;
        plan_sync(&music, destination, &self.presets()).map_err(error)
    }

    fn start_sync(
        &self,
        destination: &SyncDestination,
        workers: usize,
        events: JobEvents,
    ) -> Result<(), BackendError> {
        let music = self.music()?;
        let presets = self.presets();
        let destination = destination.clone();
        if !destination.is_connected() {
            return Err(BackendError(tr("The destination is not connected.")));
        }
        let plan = plan_sync(&music, &destination, &presets).map_err(error)?;
        let store = self.store();
        let mut events = events;
        self.spawn(move |cancel| {
            let options = JobOptions {
                workers,
                ..JobOptions::default()
            };
            let summary = run_sync(
                Some(&music),
                &destination,
                &plan,
                &presets,
                options,
                cancel,
                &mut events,
            );
            if !summary.cancelled {
                let mut state = store.load();
                if let Some(d) = state
                    .destinations
                    .iter_mut()
                    .find(|d| d.id == destination.id)
                {
                    d.last_synced = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .and_then(|t| i64::try_from(t.as_secs()).ok());
                    let _ = store.save(&state);
                }
            }
        })
    }

    fn stop(&self) {
        if let Some(token) = running().as_ref() {
            token.cancel();
        }
    }

    fn cover_albums(&self) -> Result<Vec<CoverAlbum>, BackendError> {
        let music = self.music()?;
        Ok(album_covers(&music)
            .map_err(error)?
            .iter()
            .map(|a| cover_album(a, true))
            .collect())
    }

    fn apply_cover(
        &self,
        album: AlbumKey,
        action: &CoverAction,
    ) -> Result<Vec<cover_manager::CoverResult>, BackendError> {
        let music = self.music()?;
        let album = self.album(&music, album)?;
        let settings = self.service.settings().get();
        let context = WriteContext {
            library: settings.library.clone(),
            advanced: settings.advanced,
            worker_program: None,
        };
        match action {
            CoverAction::Use { picture, save } => {
                let bytes = std::fs::read(picture).map_err(error)?;
                cover_manager::use_picture(&music, &album, &bytes, *save, &context)
            }
            CoverAction::SaveEmbedded => cover_manager::save_embedded(&music, &album),
            CoverAction::RemoveFromFiles => {
                cover_manager::remove_from_files(&music, &album, &context)
            }
        }
        .map_err(error)
    }
}
