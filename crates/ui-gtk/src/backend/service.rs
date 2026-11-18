// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The backend over the running service: the library's queries and thumbnails,
//! the player's commands and events, the window state, and the desktop
//! integrations that follow the player (MPRIS and suspend inhibition).
//!
//! Library queries run on the front end's worker threads through
//! `MusicLibrary`; thumbnails come from the library service's artwork pool. The
//! service's events are read on a thread of this adapter, which turns them into
//! [`Update`]s for the main loop and feeds MPRIS.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use helikodu_desktop::mpris::{Mpris, MprisConfig, MprisHooks, TrackDetails};
use helikodu_desktop::power::SuspendInhibition;
use helikodu_model::audio::{
    BackendSelection, DeviceSelection, MediaTime, OutputBackendKind, OutputSelection,
};
use helikodu_model::library::{MediaSource, TrackRef};
use helikodu_model::player::{
    ContextSource, EntryId, PlayModes, PlaybackState as ServicePlayback, RepeatMode, ShuffleMode,
    StopAfter, Volume,
};
use helikodu_model::settings::UiState;
use helikodu_service::bus::Subscription;
use helikodu_service::library::{
    AlbumId, AlbumQuery as ServiceAlbumQuery, AlbumSort as ServiceAlbumSort, AlbumSummary,
    CoverHash, LibraryEvent, MAX_PAGE, MusicLibrary, Page, QueueItemInfo, Thumbnail, ThumbnailSize,
};
use helikodu_service::player::{
    ContextRequest, EndReason, Notice, PlayerCommand, PlayerEvent, QueueSnapshot as Snapshot,
};
use helikodu_service::{Event, Service, SubscribeOptions};

use super::{
    AlbumDetail, AlbumKey, AlbumQuery, AlbumRow, AlbumSort, AudioFormat, Backend, BackendError,
    CoverKey, DiscGroup, LibraryCounts, LibrarySource, NowPlaying, OutputInfo, PlaybackState,
    PlayerControl, PlayerRequest, QueueSnapshot, Repeat, SignalState, ThumbnailState, TrackKey,
    TrackRow, UiStateAccess, Update,
};
use crate::i18n::{fill, tr};

/// How often positions reach the window; it interpolates between them.
const POSITION_INTERVAL: Duration = Duration::from_millis(150);

/// What the service backend also starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceOptions {
    /// Serve MPRIS2 for media keys and desktop media widgets.
    pub mpris: bool,
    /// Keep the system from suspending while playing.
    pub inhibit_suspend: bool,
}

impl Default for ServiceOptions {
    fn default() -> Self {
        Self {
            mpris: true,
            inhibit_suspend: true,
        }
    }
}

fn error(e: impl std::fmt::Display) -> BackendError {
    BackendError(e.to_string())
}

fn album_key(id: AlbumId) -> AlbumKey {
    AlbumKey(u64::try_from(id.get()).unwrap_or(0))
}

fn cover_key(hash: &CoverHash) -> CoverKey {
    CoverKey(*hash.as_bytes())
}

fn size_of(pixels: u32) -> ThumbnailSize {
    ThumbnailSize::from_pixels(pixels).unwrap_or_else(|| ThumbnailSize::for_display(pixels, 1))
}

/// The seed of an album's generated cover.
fn seed(key: AlbumKey) -> u64 {
    key.0.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17)
}

/// The library side of the service backend.
#[derive(Debug)]
pub struct ServiceLibrary {
    service: Arc<Service>,
    /// The library's album identifiers by key, learned from the lists.
    ids: Mutex<HashMap<AlbumKey, AlbumId>>,
}

impl ServiceLibrary {
    fn music(&self) -> Option<&MusicLibrary> {
        self.service.library().map(|l| l.library())
    }

    fn query(query: &AlbumQuery, page: Page) -> ServiceAlbumQuery {
        ServiceAlbumQuery {
            sort: match query.sort {
                AlbumSort::AlbumArtist => ServiceAlbumSort::Artist,
                AlbumSort::Title => ServiceAlbumSort::Title,
                AlbumSort::Year => ServiceAlbumSort::Year,
                AlbumSort::DateAdded => ServiceAlbumSort::Added,
            },
            page,
            ..ServiceAlbumQuery::default()
        }
    }

    fn remember(&self, albums: &[AlbumSummary]) {
        let mut ids = self.ids.lock().unwrap_or_else(PoisonError::into_inner);
        for album in albums {
            ids.insert(album_key(album.id), album.id);
        }
    }

    fn row(album: &AlbumSummary) -> AlbumRow {
        let key = album_key(album.id);
        AlbumRow {
            key,
            title: album.title.clone().unwrap_or_else(|| tr("Unknown album")),
            artist: album
                .artist
                .clone()
                .unwrap_or_else(|| tr("Various artists")),
            year: album.original_year.or(album.year),
            format: AudioFormat::default(),
            track_count: album.track_count,
            cover_seed: seed(key),
            cover: album.cover.as_ref().map(cover_key),
        }
    }
}

fn date(seconds: i64) -> Option<String> {
    let time = gtk::glib::DateTime::from_unix_local(seconds).ok()?;
    time.format("%x").ok().map(|s| s.to_string())
}

impl LibrarySource for ServiceLibrary {
    fn counts(&self) -> Result<LibraryCounts, BackendError> {
        let Some(music) = self.music() else {
            return Ok(LibraryCounts::default());
        };
        let totals = music.overview(0).map_err(error)?.totals;
        Ok(LibraryCounts {
            albums: totals.albums,
            artists: totals.artists,
            tracks: totals.tracks,
            genres: totals.genres,
        })
    }

    fn album_keys(&self, query: &AlbumQuery) -> Result<Vec<AlbumKey>, BackendError> {
        let Some(music) = self.music() else {
            return Ok(Vec::new());
        };
        let mut keys = Vec::new();
        loop {
            let page = music
                .albums(&Self::query(query, Page::new(keys.len(), MAX_PAGE)))
                .map_err(error)?;
            self.remember(&page.items);
            let got = page.items.len();
            keys.extend(page.items.iter().map(|a| album_key(a.id)));
            if got == 0 || keys.len() >= page.total {
                return Ok(keys);
            }
        }
    }

    fn album_rows(
        &self,
        query: &AlbumQuery,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<AlbumRow>, BackendError> {
        let Some(music) = self.music() else {
            return Ok(Vec::new());
        };
        let page = music
            .albums(&Self::query(query, Page::new(offset, limit.min(MAX_PAGE))))
            .map_err(error)?;
        self.remember(&page.items);
        Ok(page.items.iter().map(Self::row).collect())
    }

    fn album(&self, key: AlbumKey) -> Result<Option<AlbumDetail>, BackendError> {
        let Some(music) = self.music() else {
            return Ok(None);
        };
        let id = self
            .ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .copied();
        let Some(id) = id else {
            return Ok(None);
        };
        let Some(detail) = music.album(id).map_err(error)? else {
            return Ok(None);
        };
        let mut row = Self::row(&detail.summary);
        let mut folder = None;
        let mut size = 0;
        let several = detail.discs.len() > 1;
        let mut discs = Vec::new();
        for disc in &detail.discs {
            let mut tracks = Vec::new();
            for track in &disc.tracks {
                let stats = music.stats(track.id).ok();
                if let Ok(Some(source)) = music.track_source(track.id) {
                    size += std::fs::metadata(source.source.path()).map_or(0, |m| m.len());
                    if folder.is_none() {
                        folder = source.source.path().parent().map(Path::to_path_buf);
                    }
                }
                tracks.push(TrackRow {
                    key: TrackKey(track.id.get()),
                    album: key,
                    number: track.track_no,
                    disc: track.disc_no,
                    title: track.title.clone().unwrap_or_else(|| tr("Unknown title")),
                    artist: track.artist.clone().unwrap_or_default(),
                    duration: track.duration,
                    plays: stats
                        .as_ref()
                        .map_or(0, |s| u32::try_from(s.play_count).unwrap_or(u32::MAX)),
                    rating: stats.and_then(|s| s.rating).map(|r| r.stars_rounded()),
                    entry: None,
                    from_context: true,
                    cover: row.cover,
                });
            }
            discs.push(DiscGroup {
                number: several.then_some(disc.number),
                subtitle: disc.subtitle.clone(),
                tracks,
            });
        }
        // The list has no audio format; the first track gives the album's.
        let first = detail.discs.iter().flat_map(|d| &d.tracks).next();
        let mut album_gain_db = None;
        if let Some(first) = first
            && let Ok(Some(info)) = music.track_info(first.id)
        {
            let audio = &info.audio;
            row.format = AudioFormat {
                codec: audio.codec.as_ref().map(|c| c.to_uppercase()),
                sample_rate: audio.sample_rate,
                bits: audio.bit_depth,
                bitrate_kbps: audio.bitrate.map(|b| b / 1000),
                channels: audio.channels,
            };
            album_gain_db = info.loudness.and_then(|l| l.album_gain_db);
        }
        Ok(Some(AlbumDetail {
            row,
            discs,
            genre: (!detail.genres.is_empty()).then(|| {
                detail
                    .genres
                    .iter()
                    .map(|g| g.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
            added: detail.summary.added_at.and_then(date),
            size: (size > 0).then_some(size),
            folder: folder.map(|f| f.display().to_string()),
            album_gain_db,
        }))
    }

    fn supports_quality_filters(&self) -> bool {
        false
    }

    fn thumbnail(&self, cover: &CoverKey, size: u32) -> ThumbnailState {
        let (Some(library), Some(hash)) = (self.service.library(), CoverHash::from_bytes(&cover.0))
        else {
            return ThumbnailState::Unavailable;
        };
        match library.thumbnail(&hash, size_of(size)) {
            Thumbnail::Ready(path) => ThumbnailState::Ready(path),
            Thumbnail::Scheduled => ThumbnailState::Pending,
            Thumbnail::Unavailable => ThumbnailState::Unavailable,
        }
    }

    fn retain_thumbnails(&self, keep: &mut dyn FnMut(&CoverKey, u32) -> bool) {
        if let Some(library) = self.service.library() {
            library.retain_thumbnails(|hash, size| keep(&cover_key(hash), size.pixels()));
        }
    }
}

/// The player side of the service backend.
#[derive(Debug)]
pub struct ServicePlayer {
    service: Arc<Service>,
    library: Arc<ServiceLibrary>,
    updates: async_channel::Sender<Update>,
}

impl ServicePlayer {
    fn command(&self, command: PlayerCommand) {
        if self.service.player().send(command).is_err() {
            let _ = self.updates.try_send(Update::Problem {
                message: tr("The player has stopped. Restart Helikodu to play again."),
            });
        }
    }
}

impl PlayerControl for ServicePlayer {
    fn send(&self, request: PlayerRequest) {
        match request {
            PlayerRequest::PlayAlbum {
                album,
                start,
                shuffle,
            } => {
                let id = self
                    .library
                    .ids
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&album)
                    .copied();
                let Some(id) = id else { return };
                self.command(PlayerCommand::SetShuffle(if shuffle {
                    ShuffleMode::Tracks
                } else {
                    ShuffleMode::Off
                }));
                self.command(PlayerCommand::PlayContext {
                    context: ContextRequest::Album(id),
                    title: None,
                    start,
                });
            }
            PlayerRequest::PlayFiles { files, enqueue } => {
                if enqueue {
                    self.command(PlayerCommand::Enqueue {
                        tracks: files
                            .into_iter()
                            .map(|f| TrackRef::File(MediaSource::file(f)))
                            .collect(),
                        at: helikodu_model::player::EnqueueAt::End,
                    });
                } else {
                    self.command(PlayerCommand::PlayFiles { files, start: 0 });
                }
            }
            PlayerRequest::PlayEntry(entry) => {
                self.command(PlayerCommand::PlayEntry(EntryId::new(entry)));
            }
            PlayerRequest::ClearQueue => self.command(PlayerCommand::Clear),
            PlayerRequest::SetShuffle(on) => self.command(PlayerCommand::SetShuffle(if on {
                ShuffleMode::Tracks
            } else {
                ShuffleMode::Off
            })),
            PlayerRequest::SetRepeat(repeat) => {
                self.command(PlayerCommand::SetRepeat(match repeat {
                    Repeat::Off => RepeatMode::Off,
                    Repeat::All => RepeatMode::All,
                    Repeat::One => RepeatMode::One,
                }));
            }
            PlayerRequest::SetStopAfter(on) => {
                self.command(PlayerCommand::SetStopAfter(if on {
                    StopAfter::Track
                } else {
                    StopAfter::Off
                }));
            }
            PlayerRequest::PlayPause => self.command(PlayerCommand::PlayPause),
            PlayerRequest::Stop => self.command(PlayerCommand::Stop),
            PlayerRequest::Next => self.command(PlayerCommand::Next),
            PlayerRequest::Previous => self.command(PlayerCommand::Previous),
            PlayerRequest::Seek(to) => {
                self.command(PlayerCommand::Seek(MediaTime::from_duration(to)));
            }
            PlayerRequest::SeekBy(seconds) => {
                let by = Duration::from_secs_f64(seconds.abs());
                self.command(if seconds < 0.0 {
                    PlayerCommand::SeekBackward(by)
                } else {
                    PlayerCommand::SeekForward(by)
                });
            }
            PlayerRequest::SetVolume(volume) => {
                self.command(PlayerCommand::SetVolume(Volume::clamped(volume as f32)));
            }
        }
    }
}

/// The window state in `ui-state.toml`.
#[derive(Debug)]
struct ServiceUiState(Arc<Service>);

impl UiStateAccess for ServiceUiState {
    fn load(&self) -> UiState {
        self.0.ui_state().load()
    }

    fn save(&self, state: &UiState) -> Result<(), BackendError> {
        self.0.ui_state().save(state).map_err(error)
    }
}

/// The name of an output for the output button.
fn output_name(selection: &OutputSelection) -> String {
    match (&selection.backend, &selection.device) {
        (BackendSelection::Backend(OutputBackendKind::Null), _) => tr("No sound (null output)"),
        (BackendSelection::Backend(OutputBackendKind::Wav), _) => tr("WAV file"),
        (_, DeviceSelection::Device(id)) => id.as_str().to_owned(),
        (BackendSelection::Automatic, DeviceSelection::SystemDefault) => tr("System default"),
        (BackendSelection::Backend(kind), DeviceSelection::SystemDefault) => {
            fill(&tr("Default device ({})"), &[kind.as_str()])
        }
    }
}

fn modes(modes: PlayModes) -> Update {
    Update::Modes {
        shuffle: modes.shuffle != ShuffleMode::Off,
        repeat: match modes.repeat {
            RepeatMode::One => Repeat::One,
            RepeatMode::All => Repeat::All,
            _ => Repeat::Off,
        },
    }
}

fn playback(state: ServicePlayback) -> PlaybackState {
    match state {
        ServicePlayback::Playing => PlaybackState::Playing,
        ServicePlayback::Paused => PlaybackState::Paused,
        ServicePlayback::Buffering => PlaybackState::Loading,
        _ => PlaybackState::Idle,
    }
}

fn file_title(path: Option<&Path>) -> String {
    path.and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One queue entry as the panel shows it.
fn track_row(
    info: &QueueItemInfo,
    entry: Option<EntryId>,
    from_context: bool,
    album: AlbumKey,
) -> TrackRow {
    TrackRow {
        key: TrackKey(match info.track {
            TrackRef::Library(id) => id.get(),
            TrackRef::File(_) => 0,
        }),
        album,
        number: info.track_no,
        disc: None,
        title: info
            .title
            .clone()
            .unwrap_or_else(|| file_title(info.path.as_deref())),
        artist: info.artist.clone().unwrap_or_default(),
        duration: info.duration,
        plays: 0,
        rating: None,
        entry: entry.map(EntryId::get),
        from_context,
        cover: info.cover.as_ref().map(cover_key),
    }
}

/// What the event thread keeps between events.
struct Forwarder {
    service: Arc<Service>,
    updates: async_channel::Sender<Update>,
    mpris: Option<Arc<Mpris>>,
    output: OutputInfo,
    /// The now-playing entry and the cover MPRIS waits for.
    pending_cover: Arc<Mutex<Option<(EntryId, CoverHash)>>>,
}

impl Forwarder {
    fn send(&self, update: Update) -> bool {
        self.updates.send_blocking(update).is_ok()
    }

    /// The album of the queue's context, for the entries that came with it.
    fn context_album(snapshot: &Snapshot) -> AlbumKey {
        match snapshot.context.source {
            ContextSource::Album(id) => AlbumKey(id),
            _ => AlbumKey(0),
        }
    }

    fn send_queue(&self) {
        let snapshot = self.service.player().queue();
        let Some(library) = self.service.library() else {
            let items = snapshot
                .entries
                .iter()
                .map(|e| TrackRow {
                    key: TrackKey(0),
                    album: AlbumKey(0),
                    number: None,
                    disc: None,
                    title: match &e.track {
                        TrackRef::File(source) => file_title(Some(source.path())),
                        TrackRef::Library(id) => id.to_string(),
                    },
                    artist: String::new(),
                    duration: None,
                    plays: 0,
                    rating: None,
                    entry: Some(e.id.get()),
                    from_context: e.from_context,
                    cover: None,
                })
                .collect();
            let _ = self.updates.send_blocking(Update::Queue(QueueSnapshot {
                context: snapshot.context.title.clone(),
                items,
                current: snapshot.current,
            }));
            return;
        };
        let tracks = snapshot.entries.iter().map(|e| e.track.clone()).collect();
        let updates = self.updates.clone();
        let _ = library.describe(tracks, move |infos| {
            let Ok(infos) = infos else { return };
            let album = Self::context_album(&snapshot);
            let items = snapshot
                .entries
                .iter()
                .zip(&infos)
                .map(|(entry, info)| {
                    let album = if entry.from_context {
                        album
                    } else {
                        AlbumKey(0)
                    };
                    track_row(info, Some(entry.id), entry.from_context, album)
                })
                .collect();
            let _ = updates.send_blocking(Update::Queue(QueueSnapshot {
                context: snapshot.context.title.clone(),
                items,
                current: snapshot.current,
            }));
        });
    }

    fn send_now_playing(
        &self,
        entry: EntryId,
        track: TrackRef,
        path: PathBuf,
        format: AudioFormat,
    ) {
        let snapshot = self.service.player().queue();
        let from_context = snapshot
            .entries
            .iter()
            .find(|e| e.id == entry)
            .is_some_and(|e| e.from_context);
        let album = if from_context {
            Self::context_album(&snapshot)
        } else {
            AlbumKey(0)
        };
        let Some(library) = self.service.library() else {
            let row = TrackRow {
                key: TrackKey(0),
                album,
                number: None,
                disc: None,
                title: file_title(Some(&path)),
                artist: String::new(),
                duration: None,
                plays: 0,
                rating: None,
                entry: Some(entry.get()),
                from_context,
                cover: None,
            };
            let _ = self
                .updates
                .send_blocking(Update::NowPlaying(Some(NowPlaying {
                    track: row,
                    album_title: String::new(),
                    year: None,
                    cover_seed: seed(album),
                    cover: None,
                    format,
                })));
            return;
        };
        let updates = self.updates.clone();
        let mpris = self.mpris.clone();
        let pending = Arc::clone(&self.pending_cover);
        let service = Arc::clone(&self.service);
        let _ = library.describe(vec![track], move |infos| {
            let Some(info) = infos.ok().and_then(|i| i.into_iter().next()) else {
                return;
            };
            let row = track_row(&info, Some(entry), from_context, album);
            if let Some(mpris) = &mpris {
                let mut details = TrackDetails::from(&info);
                if let (Some(hash), Some(library)) = (info.cover, service.library()) {
                    match library.thumbnail(&hash, ThumbnailSize::LARGE) {
                        Thumbnail::Ready(path) => details.cover = Some(path),
                        Thumbnail::Scheduled => {
                            *pending.lock().unwrap_or_else(PoisonError::into_inner) =
                                Some((entry, hash));
                        }
                        Thumbnail::Unavailable => {}
                    }
                }
                mpris.set_details(entry, details);
            }
            let _ = updates.send_blocking(Update::NowPlaying(Some(NowPlaying {
                album_title: info.album.clone().unwrap_or_default(),
                year: None,
                cover_seed: seed(album),
                cover: row.cover,
                track: row,
                format,
            })));
        });
    }

    fn player_event(&mut self, event: &PlayerEvent) -> bool {
        if let Some(mpris) = &self.mpris {
            mpris.handle(event, self.service.player());
        }
        match event {
            PlayerEvent::State {
                playback: state, ..
            } => self.send(Update::State(playback(*state))),
            PlayerEvent::TrackChanged {
                entry,
                track,
                path,
                spec,
                length,
                ..
            } => {
                let bits = u16::try_from(spec.format.bits()).ok();
                self.output.signal = SignalState::Unreported;
                self.output.sample_rate = Some(spec.rate.hz());
                self.output.bits = bits;
                let format = AudioFormat {
                    codec: path.extension().map(|e| e.to_string_lossy().to_uppercase()),
                    sample_rate: Some(spec.rate.hz()),
                    bits,
                    bitrate_kbps: None,
                    channels: Some(spec.channel_count()),
                };
                self.send_now_playing(*entry, track.clone(), path.clone(), format);
                self.send(Update::Position {
                    elapsed: Duration::ZERO,
                    length: length.map(MediaTime::as_duration),
                }) && self.send(Update::Output(self.output.clone()))
            }
            PlayerEvent::Position {
                position, length, ..
            } => self.send(Update::Position {
                elapsed: position.as_duration(),
                length: length.map(MediaTime::as_duration),
            }),
            PlayerEvent::QueueChanged { .. } => {
                self.send_queue();
                true
            }
            PlayerEvent::ModesChanged(m) => self.send(modes(*m)),
            PlayerEvent::StopAfterChanged(stop) => {
                self.send(Update::StopAfter(*stop != StopAfter::Off))
            }
            PlayerEvent::VolumeChanged(volume) => {
                self.send(Update::Volume(f64::from(volume.position())))
            }
            PlayerEvent::OutputChanged(selection) => {
                self.output.device = output_name(selection);
                self.send(Update::Output(self.output.clone()))
            }
            PlayerEvent::Problem { file, error, .. } => {
                let message = match file {
                    Some((_, path)) => fill(
                        &tr("Cannot play {}: {}"),
                        &[&file_title(Some(path)), &error.to_string()],
                    ),
                    None => error.to_string(),
                };
                self.send(Update::Problem { message })
            }
            PlayerEvent::Notice(notice) => {
                let message = match notice {
                    Notice::RecoveredAfterCrash { .. } => Some(tr(
                        "Helikodu did not end normally last time. The queue was restored, paused.",
                    )),
                    Notice::ContextUnavailable { reason } => {
                        Some(fill(&tr("Cannot play this: {}"), &[reason]))
                    }
                    Notice::SessionNotSaved { reason } => {
                        Some(fill(&tr("The queue could not be saved: {}"), &[reason]))
                    }
                    _ => None,
                };
                message.is_none_or(|message| self.send(Update::Notice { message }))
            }
            PlayerEvent::Ended { reason, .. } => {
                self.output.signal = SignalState::Idle;
                self.output.sample_rate = None;
                self.output.bits = None;
                let state = if *reason == EndReason::StopAfter {
                    PlaybackState::Paused
                } else {
                    PlaybackState::Idle
                };
                self.send(Update::State(state)) && self.send(Update::Output(self.output.clone()))
            }
            _ => true,
        }
    }

    fn library_event(&self, event: &LibraryEvent) -> bool {
        match event {
            LibraryEvent::ThumbnailReady { hash, size, path } => {
                let mut pending = self
                    .pending_cover
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if let (Some((entry, waiting)), Some(mpris)) = (*pending, &self.mpris)
                    && waiting == *hash
                    && *size == ThumbnailSize::LARGE
                {
                    mpris.set_cover(entry, path.as_deref());
                    *pending = None;
                }
                drop(pending);
                self.send(Update::Thumbnail {
                    cover: cover_key(hash),
                    size: size.pixels(),
                    path: path.clone(),
                })
            }
            LibraryEvent::Changed => self.send(Update::LibraryChanged),
            _ => true,
        }
    }

    /// The state at the start, as if each part had just changed.
    fn initial(&mut self) {
        let status = self.service.player().status();
        self.output.device = output_name(&status.output);
        let _ = self.send(Update::Output(self.output.clone()))
            && self.send(Update::Volume(f64::from(status.volume.position())))
            && self.send(modes(status.modes))
            && self.send(Update::StopAfter(status.stop_after != StopAfter::Off))
            && self.send(Update::State(playback(status.playback)));
        self.send_queue();
        if let Some(now) = status.now_playing {
            let format = AudioFormat {
                codec: now
                    .path
                    .extension()
                    .map(|e| e.to_string_lossy().to_uppercase()),
                sample_rate: Some(now.spec.rate.hz()),
                bits: u16::try_from(now.spec.format.bits()).ok(),
                bitrate_kbps: None,
                channels: Some(now.spec.channel_count()),
            };
            self.send_now_playing(now.entry, now.track.clone(), now.path.clone(), format);
            let _ = self.send(Update::Position {
                elapsed: status.position.as_duration(),
                length: now.length.map(MediaTime::as_duration),
            });
        }
        if let Some(mpris) = &self.mpris {
            mpris.sync(self.service.player());
        }
    }

    fn run(mut self, events: &Subscription<Event>) {
        self.initial();
        loop {
            match events.recv_timeout(Duration::from_millis(200)) {
                Ok(Event::Player(event)) => {
                    if !self.player_event(&event) {
                        return;
                    }
                }
                Ok(Event::Library(event)) => {
                    if !self.library_event(&event) {
                        return;
                    }
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    if self.updates.is_closed() {
                        return;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

/// A backend over the running `service`. With `options`, MPRIS is served and
/// suspend is inhibited while playing; failures there are reported as notices
/// and do not stop the program.
pub fn connect(service: Service, options: ServiceOptions) -> Result<Backend, BackendError> {
    let service = Arc::new(service);
    let (sender, receiver) = async_channel::unbounded();
    let mut notices = Vec::new();
    let mpris = if options.mpris {
        let raise = sender.clone();
        let quit = sender.clone();
        let hooks = MprisHooks::for_player(service.player().sender())
            .with_raise(move || {
                let _ = raise.try_send(Update::Raise);
            })
            .with_quit(move || {
                let _ = quit.try_send(Update::Quit);
            });
        match Mpris::start(MprisConfig::default(), hooks) {
            Ok(mpris) => Some(Arc::new(mpris)),
            Err(e) => {
                notices.push(format!("MPRIS: {e}"));
                None
            }
        }
    } else {
        None
    };
    let inhibition = if options.inhibit_suspend {
        let events = service
            .player()
            .subscribe(SubscribeOptions::player().with_position_interval(Duration::from_secs(5)));
        SuspendInhibition::system()
            .with_reason(tr("Playing music"))
            .follow(events, |e| {
                eprintln!("helikodu: cannot keep the system awake: {e}")
            })
            .ok()
    } else {
        None
    };
    for message in notices {
        eprintln!("helikodu: {message}");
    }
    let events =
        service.subscribe(SubscribeOptions::all().with_position_interval(POSITION_INTERVAL));
    let forwarder = Forwarder {
        service: Arc::clone(&service),
        updates: sender.clone(),
        mpris,
        output: OutputInfo::default(),
        pending_cover: Arc::new(Mutex::new(None)),
    };
    thread::Builder::new()
        .name("helikodu-ui-events".into())
        .spawn(move || {
            // The inhibition lives as long as this thread, which ends with the window.
            let _inhibition = inhibition;
            forwarder.run(&events);
        })
        .map_err(error)?;
    let library = Arc::new(ServiceLibrary {
        service: Arc::clone(&service),
        ids: Mutex::new(HashMap::new()),
    });
    let player = Rc::new(ServicePlayer {
        service: Arc::clone(&service),
        library: Arc::clone(&library),
        updates: sender,
    });
    Ok(Backend::new(library, player, receiver).with_ui_state(Rc::new(ServiceUiState(service))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_and_shuffle_map_both_ways() {
        let update = modes(PlayModes {
            shuffle: ShuffleMode::Tracks,
            repeat: RepeatMode::One,
        });
        assert_eq!(
            update,
            Update::Modes {
                shuffle: true,
                repeat: Repeat::One
            }
        );
    }

    #[test]
    fn thumbnail_sizes() {
        assert_eq!(size_of(192), ThumbnailSize::LARGE);
        assert_eq!(size_of(160), ThumbnailSize::LARGE);
        assert_eq!(size_of(400), ThumbnailSize::LARGE_2X);
    }
}
