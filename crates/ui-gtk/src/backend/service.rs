// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The backend over the running service: the library's queries and thumbnails,
//! the player's commands and events, the window state, and the desktop
//! integrations that follow the player (MPRIS and suspend inhibition).
//!
//! Library queries run on the front end's worker threads through
//! `MusicLibrary`; thumbnails come from the library service's artwork pool. The
//! service's events are read on a thread of this adapter, which turns them into
//! [`Update`]s for the main loop and feeds MPRIS. MPRIS calls that would start
//! playback come back to the window as [`Update::Request`], so they pass the
//! full-output gate.

mod artist_photos;
mod browse;
mod export;
mod history;
mod insights;
mod lyrics;
mod mpris;
mod services;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use helikodu_desktop::mpris::{Mpris, MprisConfig, MprisHooks, TrackDetails};
use helikodu_desktop::power::SuspendInhibition;
use helikodu_model::audio::{
    BackendSelection, DeviceSelection, MediaTime, OutputBackendKind, OutputSelection,
};
use helikodu_model::library::{MediaSource, TrackId, TrackRef};
use helikodu_model::player::{
    ContextSource, EngineError, EnqueueAt, EntryId, PlayModes, PlaybackState as ServicePlayback,
    QueueContext, RepeatMode, ShuffleMode, StopAfter, Volume,
};
use helikodu_model::query::{Field, Query, SortBy, SortKey};
use helikodu_model::settings::UiState;
use helikodu_service::bus::Subscription;
use helikodu_service::library::{
    AlbumFilter as ServiceAlbumFilter, AlbumId, AlbumQuality, AlbumQuery as ServiceAlbumQuery,
    AlbumSort as ServiceAlbumSort, AlbumSummary, ArtistQuery, ArtistRole, ArtistSummary, CoverHash,
    GenreId, GenreQuery, ItemContent, LibraryEvent, LibraryService, MAX_PAGE, MusicLibrary, Page,
    PersonId, PinTarget, PlaylistId, PlaylistType as ServicePlaylistType, QueueItemInfo,
    SavedSearchId, Thumbnail, ThumbnailSize, TrackFilter, TrackOrder, TrackSortKey, TrackSummary,
    parse_query_lenient,
};
use helikodu_service::player::SleepWhen as ServiceSleepWhen;
use helikodu_service::player::{
    AnalyzerWatch, ContextRequest, EndReason, Notice, PlayerCommand, PlayerEvent,
    QueueSnapshot as Snapshot,
};
use helikodu_service::waveforms::WaveformEvent;
use helikodu_service::{Event, Service, SubscribeOptions};

use super::{
    AlbumDetail, AlbumFilter, AlbumKey, AlbumQuery, AlbumRow, AlbumScope, AlbumSort, ArtistDetail,
    ArtistKey, ArtistRow, AudioFormat, Backend, BackendError, ChapterRow, CoverKey, DiscGroup,
    FolderFile, FolderListing, FolderRow, FolderState, GenreKey, GenreRow, LibraryCounts,
    LibrarySource, MediaKind, NowPlaying, OutputInfo, PinKey, PinRow, PlaybackState, PlayerControl,
    PlayerRequest, PlaylistCleanup, PlaylistKey, PlaylistRow, PlaylistType, QueueSnapshot, Repeat,
    SavedSearchKey, SavedSearchRow, SearchResults, SignalState, SmartTrack, ThumbnailState,
    TrackColumn, TrackFacts, TrackKey, TrackQuery, TrackRow, TrackScope, TrackSort, UiStateAccess,
    Update, YearRow,
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

fn playlist_key(id: PlaylistId) -> PlaylistKey {
    PlaylistKey(u64::try_from(id.get()).unwrap_or(0))
}

fn playlist_type(kind: ServicePlaylistType) -> PlaylistType {
    match kind {
        ServicePlaylistType::Folder => PlaylistType::Folder,
        ServicePlaylistType::Smart => PlaylistType::Smart,
        _ => PlaylistType::Static,
    }
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

/// The album and the number of other copies of each of `tracks`, in one query
/// rather than one or two per row; tracks not in the library are left out.
fn albums_and_copies(
    music: &MusicLibrary,
    tracks: &[TrackId],
) -> HashMap<TrackId, (AlbumKey, u32)> {
    music
        .albums_and_copies(tracks)
        .unwrap_or_default()
        .into_iter()
        .map(|(track, (album, copies))| (track, (album.map_or(AlbumKey(0), album_key), copies)))
        .collect()
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
    /// The library's playlist identifiers by key, learned from the lists.
    playlists: Mutex<HashMap<PlaylistKey, PlaylistId>>,
    /// The album list in album-artist order, for search and the grid.
    album_rows: Mutex<Vec<AlbumRow>>,
    /// The album list of the last other query of the grid.
    listed: Mutex<Option<(AlbumQuery, Vec<AlbumRow>)>>,
    /// The artist list, for search and names.
    artist_rows: Mutex<Vec<ArtistRow>>,
    /// Names asked for that are no artist of the list, and the count of library
    /// changes they were found missing at: a credit such as "A feat. B" is
    /// asked for at every track change, and must not read every artist again.
    not_artists: Mutex<(u64, HashSet<String>)>,
    /// How many times the library changed.
    changes: Arc<AtomicU64>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The audio file extensions the folder view lists.
const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "ogg", "oga", "opus", "m4a", "m4b", "mp4", "aac", "wav", "aif", "aiff", "ape",
    "wv", "mpc", "tta", "tak", "dsf", "dff", "wma", "spx",
];

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(e)))
}

fn person(key: ArtistKey) -> PersonId {
    PersonId::from_raw(i64::try_from(key.0).unwrap_or(-1))
}

fn genre_id(key: GenreKey) -> GenreId {
    GenreId::from_raw(i64::try_from(key.0).unwrap_or(-1))
}

fn track_id(key: TrackKey) -> TrackId {
    TrackId::new(key.0)
}

fn artist_row(artist: &ArtistSummary) -> ArtistRow {
    ArtistRow {
        key: ArtistKey(u64::try_from(artist.id.get()).unwrap_or(0)),
        name: artist.name.clone(),
        sort_name: artist
            .sort_name
            .clone()
            .unwrap_or_else(|| artist.name.clone()),
        album_count: artist.album_count,
        track_count: artist.track_count,
        photo: None,
        own_photo: false,
    }
}

/// A folder name as shown: the home folder as `~`.
fn display_path(path: &Path) -> String {
    let home = gtk::glib::home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether `path` has a folder in it that is not hidden.
fn has_subfolders(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            !e.file_name().as_encoded_bytes().starts_with(b".")
                && e.file_type().is_ok_and(|t| t.is_dir())
        })
    })
}

/// The library's id of a saved search.
fn saved_search_id(key: SavedSearchKey) -> SavedSearchId {
    SavedSearchId::from_raw(i64::try_from(key.0).unwrap_or(0))
}

/// The library's sort key for a column of the track table.
fn sort_key(column: TrackColumn) -> TrackSortKey {
    match column {
        TrackColumn::Number => TrackSortKey::Number,
        TrackColumn::Title => TrackSortKey::Title,
        TrackColumn::Artist => TrackSortKey::Artist,
        TrackColumn::Album => TrackSortKey::Album,
        TrackColumn::Year => TrackSortKey::Year,
        TrackColumn::Format => TrackSortKey::Format,
        TrackColumn::Rating => TrackSortKey::Rating,
        TrackColumn::Plays => TrackSortKey::Plays,
        TrackColumn::Length => TrackSortKey::Length,
        // Sorted through the query language instead (see `query_sort`).
        TrackColumn::Genre | TrackColumn::Added => TrackSortKey::Title,
    }
}

/// The query language's sort keys for a column of the track table.
fn query_sort(keys: &[TrackSort]) -> Vec<SortKey> {
    keys.iter()
        .flat_map(|key| {
            let fields = match key.column {
                TrackColumn::Number => vec![Field::Disc, Field::Track],
                TrackColumn::Title => vec![Field::Title],
                TrackColumn::Artist => vec![Field::Artist],
                TrackColumn::Album => vec![Field::Album],
                TrackColumn::Year => vec![Field::Year],
                TrackColumn::Format => vec![Field::Codec, Field::SampleRate, Field::BitDepth],
                TrackColumn::Rating => vec![Field::Rating],
                TrackColumn::Plays => vec![Field::Plays],
                TrackColumn::Length => vec![Field::Duration],
                TrackColumn::Genre => vec![Field::Genre],
                TrackColumn::Added => vec![Field::Added],
            };
            fields.into_iter().map(move |field| SortKey {
                by: SortBy::Field(field),
                descending: key.descending,
            })
        })
        .collect()
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
            filter: match query.scope {
                AlbumScope::All => ServiceAlbumFilter::All,
                AlbumScope::Artist(key) => ServiceAlbumFilter::ByArtist(person(key)),
                AlbumScope::AppearsOn(key) => ServiceAlbumFilter::AppearsOn(person(key)),
                AlbumScope::Genre(key) => ServiceAlbumFilter::Genre(genre_id(key)),
                AlbumScope::Composer(key) => {
                    ServiceAlbumFilter::Credited(person(key), ArtistRole::Composer)
                }
                AlbumScope::Years(range) => ServiceAlbumFilter::Years(browse::year_span(range)),
            },
            quality: match query.filter {
                AlbumFilter::All => AlbumQuality::Any,
                AlbumFilter::Lossless => AlbumQuality::Lossless,
                AlbumFilter::HiRes => AlbumQuality::HiRes,
            },
            page,
            ..ServiceAlbumQuery::default()
        }
    }

    /// Every album artist, kept for search and names. With `look`, the
    /// artists' folders are looked in for new photos first: that groups every
    /// track by artist and folder (some 30 MB of sorting at 100,000 tracks),
    /// so only the Artists page does it; lookups take the photos as known.
    fn artist_list(&self, look: bool) -> Result<Vec<ArtistRow>, BackendError> {
        let music = self.library()?;
        let mut rows = Vec::new();
        loop {
            let mut query = ArtistQuery::new(ArtistRole::AlbumArtist);
            query.page = Page::new(rows.len(), MAX_PAGE);
            let page = music.artists(&query).map_err(error)?;
            let got = page.items.len();
            rows.extend(page.items.iter().map(artist_row));
            if got == 0 || rows.len() >= page.total {
                break;
            }
        }
        if let Ok(photos) = music.artist_photos(look) {
            artist_photos::show_all(&mut rows, &photos);
        }
        lock(&self.artist_rows).clone_from(&rows);
        Ok(rows)
    }

    /// Every album of `query`.
    fn all_albums(&self, query: &AlbumQuery) -> Result<Vec<AlbumRow>, BackendError> {
        let Some(music) = self.music() else {
            return Ok(Vec::new());
        };
        let albums = music
            .all_albums(&Self::query(query, Page::default()))
            .map_err(error)?;
        self.remember(&albums);
        Ok(albums.iter().map(Self::row).collect())
    }

    /// The row of a track from a list, remembering its album.
    fn summary_row(&self, track: &TrackSummary) -> TrackRow {
        let key = TrackKey(track.id.get());
        let album = track.album.map_or(AlbumKey(0), album_key);
        TrackRow {
            key,
            album,
            number: track.track_no,
            disc: track.disc_no,
            title: track.title.clone().unwrap_or_else(|| tr("Unknown title")),
            artist: track.artist.clone().unwrap_or_default(),
            duration: track.duration,
            from_context: true,
            cover_seed: seed(album),
            album_title: track.album_title.clone().unwrap_or_default(),
            year: track.year,
            missing: track.missing,
            copies: track.copies,
            ..TrackRow::default()
        }
    }

    /// The tracks of `query` through the query language, when its text is a query
    /// or it sorts by a column only the query language sorts by; `None` otherwise.
    fn query_keys(
        &self,
        music: &MusicLibrary,
        query: &TrackQuery,
    ) -> Result<Option<Vec<TrackKey>>, BackendError> {
        let text = query.text.trim();
        let parsed = (!text.is_empty()).then(|| parse_query_lenient(text));
        let is_query = parsed
            .as_ref()
            .is_some_and(|p| p.error.is_none() && !p.query.is_plain_text());
        let by_query = query
            .sort
            .iter()
            .any(|k| matches!(k.column, TrackColumn::Genre | TrackColumn::Added));
        if !is_query && !by_query {
            return Ok(None);
        }
        let mut search: Query = parsed.map(|p| p.query).unwrap_or_default();
        if !query.sort.is_empty() {
            search.sort = query_sort(&query.sort);
        }
        let ids = music.query_track_ids(&search, 0).map_err(error)?;
        let mut keys: Vec<TrackKey> = ids.iter().map(|id| TrackKey(id.get())).collect();
        // The scope, if any.
        let filter = match query.scope {
            TrackScope::All => None,
            TrackScope::Artist(key) => Some(TrackFilter::Artist(person(key), None)),
            TrackScope::Genre(key) => Some(TrackFilter::Genre(genre_id(key))),
        };
        if let Some(filter) = filter {
            let tracks = music.sorted_track_ids(filter, &[]).map_err(error)?;
            let scope: HashSet<TrackKey> =
                tracks.iter().map(|(id, _)| TrackKey(id.get())).collect();
            keys.retain(|k| scope.contains(k));
        }
        Ok(Some(keys))
    }

    /// The row of one track by its key, from the database, on `album` and with
    /// `copies` other copies.
    fn track_row(
        &self,
        music: &MusicLibrary,
        key: TrackKey,
        album: AlbumKey,
        copies: u32,
    ) -> Option<TrackRow> {
        let details = music.track_details(track_id(key)).ok().flatten()?;
        let stats = music.stats(track_id(key)).ok();
        let audio = &details.audio;
        Some(TrackRow {
            key,
            album,
            number: details.tags.track_no,
            disc: details.tags.disc_no,
            title: details
                .tags
                .title
                .clone()
                .unwrap_or_else(|| file_name(&details.path)),
            artist: details.tags.artists.join(", "),
            duration: audio.duration,
            plays: stats
                .as_ref()
                .map_or(0, |s| u32::try_from(s.play_count).unwrap_or(u32::MAX)),
            rating: stats.and_then(|s| s.rating).map(|r| r.stars_rounded()),
            from_context: true,
            cover_seed: seed(album),
            album_title: details.tags.album.clone().unwrap_or_default(),
            year: details.tags.year,
            format: AudioFormat {
                codec: audio.codec.as_ref().map(|c| c.to_uppercase()),
                sample_rate: audio.sample_rate,
                bits: audio.bit_depth,
                bitrate_kbps: audio.bitrate.map(|b| b / 1000),
                channels: audio.channels,
            },
            path: Some(details.path),
            copies,
            genre: details.tags.genres.join(", "),
            added: Some(details.added_at),
            ..TrackRow::default()
        })
    }

    fn playlist_id(&self, key: PlaylistKey) -> Result<PlaylistId, BackendError> {
        if let Some(id) = lock(&self.playlists).get(&key) {
            return Ok(*id);
        }
        self.playlists()?;
        lock(&self.playlists)
            .get(&key)
            .copied()
            .ok_or_else(|| BackendError(tr("The playlist is gone.")))
    }

    fn pin_target(&self, key: PinKey) -> Result<PinTarget, BackendError> {
        Ok(match key {
            PinKey::Album(album) => {
                PinTarget::Album(AlbumId::from_raw(i64::try_from(album.0).unwrap_or(-1)))
            }
            PinKey::Artist(artist) => PinTarget::Artist(person(artist)),
            PinKey::Playlist(playlist) => PinTarget::Playlist(self.playlist_id(playlist)?),
        })
    }

    fn library(&self) -> Result<&MusicLibrary, BackendError> {
        self.music()
            .ok_or_else(|| BackendError(tr("There is no library.")))
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
            format: album
                .format
                .as_ref()
                .map(|f| AudioFormat {
                    codec: f.codec.as_ref().map(|c| c.to_uppercase()),
                    sample_rate: f.sample_rate,
                    bits: f.bit_depth,
                    bitrate_kbps: f.bitrate.map(|b| b / 1000),
                    channels: None,
                })
                .unwrap_or_default(),
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
            composers: browse::composer_count(music),
            years: music.years().map_or(0, |y| y.len()),
        })
    }

    fn album_keys(&self, query: &AlbumQuery) -> Result<Vec<AlbumKey>, BackendError> {
        let Some(music) = self.music() else {
            return Ok(Vec::new());
        };
        let _ = music;
        let rows = self.all_albums(query)?;
        let keys = rows.iter().map(|r| r.key).collect();
        if *query == AlbumQuery::default() {
            *lock(&self.album_rows) = rows;
        } else {
            *lock(&self.listed) = Some((*query, rows));
        }
        Ok(keys)
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
        // The grid's rows come from the list its keys came with: a page from the
        // database sorts every album again.
        let slice = |rows: &[AlbumRow]| rows.iter().skip(offset).take(limit).cloned().collect();
        if *query == AlbumQuery::default() {
            let rows = lock(&self.album_rows);
            if !rows.is_empty() {
                return Ok(slice(&rows));
            }
        } else if let Some((listed, rows)) = lock(&self.listed).as_ref()
            && listed == query
        {
            return Ok(slice(rows));
        }
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
        let mut resume = None;
        for disc in &detail.discs {
            let mut tracks = Vec::new();
            for track in &disc.tracks {
                let stats = music.stats(track.id).ok();
                if let Some(at) = stats.as_ref().and_then(|s| s.resume_position) {
                    resume = Some((TrackKey(track.id.get()), at));
                }
                if let Ok(Some(source)) = music.track_source(track.id) {
                    size += std::fs::metadata(source.source.path()).map_or(0, |m| m.len());
                    if folder.is_none() {
                        folder = source.source.path().parent().map(Path::to_path_buf);
                    }
                }
                let path = music
                    .track_source(track.id)
                    .ok()
                    .flatten()
                    .map(|s| s.source.path().to_path_buf());
                let mut track_row = self.summary_row(track);
                track_row.plays = stats
                    .as_ref()
                    .map_or(0, |s| u32::try_from(s.play_count).unwrap_or(u32::MAX));
                track_row.rating = stats.and_then(|s| s.rating).map(|r| r.stars_rounded());
                track_row.cover = row.cover;
                track_row.format = row.format.clone();
                track_row.path = path;
                tracks.push(track_row);
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
        let mut reader = None;
        if let Some(first) = first
            && let Ok(Some(info)) = music.track_info(first.id)
        {
            reader = (!info.tags.composers.is_empty()).then(|| info.tags.composers.join(", "));
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
            kind: browse::album_kind(detail.discs.iter().flat_map(|d| &d.tracks)),
            resume,
            reader,
        }))
    }

    fn artists(&self) -> Result<Vec<ArtistRow>, BackendError> {
        self.artist_list(true)
    }

    fn artist(&self, key: ArtistKey) -> Result<Option<ArtistDetail>, BackendError> {
        let music = self.library()?;
        let known = lock(&self.artist_rows)
            .iter()
            .find(|a| a.key == key)
            .cloned();
        let row = match known {
            Some(row) => row,
            None => match self.artist_list(false)?.into_iter().find(|a| a.key == key) {
                Some(row) => row,
                None => return Ok(None),
            },
        };
        let query = |scope| AlbumQuery {
            scope,
            sort: AlbumSort::Year,
            ..AlbumQuery::default()
        };
        let own = self.all_albums(&query(AlbumScope::Artist(key)))?;
        let appears_on = self.all_albums(&query(AlbumScope::AppearsOn(key)))?;
        let mut types = HashMap::new();
        for page in 0.. {
            let found = music
                .albums(&ServiceAlbumQuery {
                    filter: ServiceAlbumFilter::ByArtist(person(key)),
                    page: Page::new(page * MAX_PAGE, MAX_PAGE),
                    ..ServiceAlbumQuery::default()
                })
                .map_err(error)?;
            for album in &found.items {
                types.insert(
                    album_key(album.id),
                    album.release_types.first().cloned().unwrap_or_default(),
                );
            }
            if found.items.len() < MAX_PAGE {
                break;
            }
        }
        let single = |row: &AlbumRow| {
            types
                .get(&row.key)
                .is_some_and(|t| t.eq_ignore_ascii_case("ep") || t.eq_ignore_ascii_case("single"))
        };
        let (singles, albums): (Vec<AlbumRow>, Vec<AlbumRow>) = own.into_iter().partition(single);
        let tracks = music
            .all_tracks(TrackFilter::Artist(person(key), None))
            .map_err(error)?;
        let duration = tracks.iter().filter_map(|t| t.duration).sum();
        let mut played: Vec<(u64, &TrackSummary)> = tracks
            .iter()
            .take(500)
            .map(|t| (music.stats(t.id).map_or(0, |s| s.play_count), t))
            .collect();
        played.sort_by_key(|(plays, _)| std::cmp::Reverse(*plays));
        let most_played = played
            .into_iter()
            .take(5)
            .map(|(plays, t)| {
                let mut row = self.summary_row(t);
                row.plays = u32::try_from(plays).unwrap_or(u32::MAX);
                row
            })
            .collect();
        Ok(Some(ArtistDetail {
            row,
            albums,
            singles,
            appears_on,
            most_played,
            duration,
        }))
    }

    fn find_artist(&self, name: &str) -> Result<Option<ArtistKey>, BackendError> {
        let name = name.to_lowercase();
        let find = |rows: &[ArtistRow]| {
            rows.iter()
                .find(|a| a.name.to_lowercase() == name)
                .map(|a| a.key)
        };
        if let Some(key) = find(&lock(&self.artist_rows)) {
            return Ok(Some(key));
        }
        let changes = self.changes.load(Ordering::Relaxed);
        {
            let mut missing = lock(&self.not_artists);
            if missing.0 != changes {
                *missing = (changes, HashSet::new());
            } else if missing.1.contains(&name) {
                return Ok(None);
            }
        }
        let found = find(&self.artist_list(false)?);
        if found.is_none() {
            let mut missing = lock(&self.not_artists);
            if missing.0 == changes {
                missing.1.insert(name);
            }
        }
        Ok(found)
    }

    fn set_artist_photo(&self, key: ArtistKey, file: &Path) -> Result<ArtistRow, BackendError> {
        self.set_photo(key, file)
    }

    fn remove_artist_photo(&self, key: ArtistKey) -> Result<ArtistRow, BackendError> {
        self.remove_photo(key)
    }

    fn genres(&self) -> Result<Vec<GenreRow>, BackendError> {
        let music = self.library()?;
        let mut rows = Vec::new();
        loop {
            let page = music
                .genres(&GenreQuery {
                    page: Page::new(rows.len(), MAX_PAGE),
                    ..GenreQuery::default()
                })
                .map_err(error)?;
            let got = page.items.len();
            rows.extend(page.items.iter().map(|g| GenreRow {
                key: GenreKey(u64::try_from(g.id.get()).unwrap_or(0)),
                name: g.name.clone(),
                album_count: g.album_count,
                track_count: g.track_count,
            }));
            if got == 0 || rows.len() >= page.total {
                return Ok(rows);
            }
        }
    }

    fn track_keys(&self, query: &TrackQuery) -> Result<Vec<TrackKey>, BackendError> {
        let music = self.library()?;
        let text = query.text.trim();
        if let Some(keys) = self.query_keys(music, query)? {
            return Ok(keys);
        }
        let mut found: Option<Vec<TrackKey>> = None;
        if !text.is_empty() {
            let keys: Vec<TrackKey> = music
                .search_ids(text)
                .map_err(error)?
                .iter()
                .map(|id| TrackKey(id.get()))
                .collect();
            if query.scope == TrackScope::All && query.sort.is_empty() {
                return Ok(keys);
            }
            found = Some(keys);
        }
        let filter = match query.scope {
            TrackScope::All => TrackFilter::All,
            TrackScope::Artist(key) => TrackFilter::Artist(person(key), None),
            TrackScope::Genre(key) => TrackFilter::Genre(genre_id(key)),
        };
        let sort: Vec<TrackOrder> = query
            .sort
            .iter()
            .map(|key| TrackOrder {
                key: sort_key(key.column),
                descending: key.descending,
            })
            .collect();
        // Only ids and albums, sorted in one query: the rows come later, by key.
        let mut tracks = music.sorted_track_ids(filter, &sort).map_err(error)?;
        if let Some(found) = found {
            let found: HashSet<TrackKey> = found.into_iter().collect();
            tracks.retain(|(id, _)| found.contains(&TrackKey(id.get())));
        }
        Ok(tracks.iter().map(|(id, _)| TrackKey(id.get())).collect())
    }

    fn track_rows(&self, keys: &[TrackKey]) -> Result<Vec<TrackRow>, BackendError> {
        let music = self.library()?;
        let ids: Vec<TrackId> = keys.iter().map(|key| track_id(*key)).collect();
        let known = albums_and_copies(music, &ids);
        Ok(keys
            .iter()
            .filter_map(|key| {
                let (album, copies) = known.get(&track_id(*key)).copied()?;
                self.track_row(music, *key, album, copies)
            })
            .collect())
    }

    fn search(&self, text: &str) -> Result<SearchResults, BackendError> {
        let music = self.library()?;
        let text = text.trim();
        if text.is_empty() {
            return Ok(SearchResults::default());
        }
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        let hit = |name: &str| {
            let name = name.to_lowercase();
            words.iter().all(|w| name.contains(w.as_str()))
        };
        if lock(&self.artist_rows).is_empty() {
            self.artist_list(false)?;
        }
        if lock(&self.album_rows).is_empty() {
            self.album_keys(&AlbumQuery::default())?;
        }
        let artists = lock(&self.artist_rows)
            .iter()
            .filter(|a| hit(&a.name))
            .take(5)
            .cloned()
            .collect();
        let albums = lock(&self.album_rows)
            .iter()
            .filter(|a| hit(&a.title))
            .take(8)
            .cloned()
            .collect();
        let page = music.search(text, 0, 50).map_err(error)?;
        // Looked up under the lock rather than copied: the map holds every
        // track of the track table.
        let ids: Vec<TrackId> = page.hits.iter().map(|found| found.hit.track).collect();
        let known = albums_and_copies(music, &ids);
        let tracks = page
            .hits
            .iter()
            .map(|found| {
                let key = TrackKey(found.hit.track.get());
                let (album, copies) = known.get(&found.hit.track).copied().unwrap_or_default();
                TrackRow {
                    key,
                    album,
                    title: found.hit.title.clone().unwrap_or_default(),
                    artist: found.hit.artist.clone().unwrap_or_default(),
                    album_title: found.hit.album.clone().unwrap_or_default(),
                    duration: found.hit.duration,
                    path: found.path.clone(),
                    cover_seed: seed(album),
                    from_context: true,
                    copies,
                    ..TrackRow::default()
                }
            })
            .collect();
        Ok(SearchResults {
            artists,
            albums,
            tracks,
            track_total: page.total,
        })
    }

    fn set_rating(&self, track: TrackKey, stars: Option<u8>) -> Result<(), BackendError> {
        // Through the library service, which announces the stored rating to
        // every list showing the track.
        let library = self
            .service
            .library()
            .ok_or_else(|| BackendError(tr("There is no library.")))?;
        let (reply, answer) = std::sync::mpsc::channel();
        // With "Write ratings to files" on, the rating goes into the file too.
        let settings = (*self.service.settings().get()).clone();
        library
            .rate_into_file(
                track_id(track),
                stars.filter(|s| *s > 0),
                settings,
                move |r| {
                    let _ = reply.send(r);
                },
            )
            .map_err(error)?;
        answer
            .recv()
            .map_err(|_| BackendError(tr("The library has stopped.")))?
            .map(|_| ())
            .map_err(error)
    }

    fn playlists(&self) -> Result<Vec<PlaylistRow>, BackendError> {
        let tree = self.library()?.playlist_tree().map_err(error)?;
        let mut ids = lock(&self.playlists);
        Ok(tree
            .iter()
            .map(|node| {
                let p = &node.summary;
                let key = playlist_key(p.id);
                ids.insert(key, p.id);
                PlaylistRow {
                    key,
                    // The built-in one is named in the interface's language.
                    name: if p.builtin {
                        tr("Favourites")
                    } else {
                        p.name.clone()
                    },
                    items: p.items,
                    duration: p.duration,
                    modified: Some(p.modified),
                    kind: playlist_type(p.kind),
                    parent: p.parent.map(playlist_key),
                    depth: node.depth,
                    locked: p.locked,
                    grouped: p.grouped,
                    from_file: p.source.is_some(),
                    dead: p.dead,
                    can_undo: p.can_undo,
                    can_redo: p.can_redo,
                    builtin: p.builtin,
                }
            })
            .collect())
    }

    fn is_favourite(&self, track: TrackKey) -> Result<bool, BackendError> {
        if track.0 == 0 {
            return Ok(false);
        }
        self.library()?.is_favourite(track_id(track)).map_err(error)
    }

    fn set_favourite(&self, track: TrackKey, favourite: bool) -> Result<bool, BackendError> {
        self.library()?
            .set_favourite(track_id(track), favourite)
            .map_err(error)
    }

    fn playlist_items(&self, key: PlaylistKey) -> Result<Vec<TrackRow>, BackendError> {
        let id = self.playlist_id(key)?;
        let items = self.library()?.playlist_items(id).map_err(error)?;
        Ok(items
            .iter()
            .map(|item| {
                let mut row = match &item.content {
                    ItemContent::Track(track) => self.summary_row(track),
                    ItemContent::Dead(dead) => TrackRow {
                        title: dead.title.clone().unwrap_or_else(|| tr("Unknown title")),
                        artist: dead.artist.clone().unwrap_or_default(),
                        album_title: dead.album.clone().unwrap_or_default(),
                        duration: dead.duration,
                        missing: true,
                        ..TrackRow::default()
                    },
                };
                row.entry = Some(item.position as u64);
                row
            })
            .collect())
    }

    fn create_playlist(&self, name: &str) -> Result<PlaylistKey, BackendError> {
        let id = self.library()?.create_playlist(name).map_err(error)?;
        let key = PlaylistKey(u64::try_from(id.get()).unwrap_or(0));
        lock(&self.playlists).insert(key, id);
        Ok(key)
    }

    fn rename_playlist(&self, key: PlaylistKey, name: &str) -> Result<(), BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?.rename_playlist(id, name).map_err(error)
    }

    fn delete_playlist(&self, key: PlaylistKey) -> Result<(), BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?.delete_playlist(id).map_err(error)
    }

    fn add_to_playlist(
        &self,
        key: PlaylistKey,
        tracks: &[TrackKey],
        position: Option<usize>,
    ) -> Result<(), BackendError> {
        let id = self.playlist_id(key)?;
        let tracks = tracks.iter().map(|t| track_id(*t)).collect();
        self.library()?
            .add_to_playlist(id, tracks, position)
            .map_err(error)
    }

    fn move_in_playlist(
        &self,
        key: PlaylistKey,
        positions: &[usize],
        to: usize,
    ) -> Result<(), BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?
            .move_playlist_items(id, positions.to_vec(), to)
            .map(|_| ())
            .map_err(error)
    }

    fn remove_from_playlist(
        &self,
        key: PlaylistKey,
        positions: &[usize],
    ) -> Result<(), BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?
            .remove_from_playlist(id, positions.to_vec())
            .map(|_| ())
            .map_err(error)
    }

    fn history(
        &self,
        since: Option<std::time::SystemTime>,
    ) -> Result<Vec<super::HistoryRow>, BackendError> {
        self.history_rows(since)
    }

    fn history_totals(&self) -> Result<super::HistoryTotals, BackendError> {
        self.totals()
    }

    fn export_history(
        &self,
        path: &Path,
        since: Option<std::time::SystemTime>,
    ) -> Result<usize, BackendError> {
        self.export(path, since)
    }

    fn import_playlist(&self, path: &Path) -> Result<(PlaylistKey, String), BackendError> {
        let report = self.library()?.import_playlist(path).map_err(error)?;
        let key = PlaylistKey(u64::try_from(report.playlist.get()).unwrap_or(0));
        lock(&self.playlists).insert(key, report.playlist);
        Ok((key, report.name))
    }

    fn export_playlist(&self, key: PlaylistKey, path: &Path) -> Result<usize, BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?
            .export_playlist(id, path)
            .map(|r| r.entries)
            .map_err(error)
    }

    fn create_playlist_folder(
        &self,
        name: &str,
        parent: Option<PlaylistKey>,
    ) -> Result<PlaylistKey, BackendError> {
        let parent = parent.map(|p| self.playlist_id(p)).transpose()?;
        let id = self
            .library()?
            .create_playlist_folder(name, parent)
            .map_err(error)?;
        let key = playlist_key(id);
        lock(&self.playlists).insert(key, id);
        Ok(key)
    }

    fn move_playlist(
        &self,
        key: PlaylistKey,
        parent: Option<PlaylistKey>,
        position: Option<usize>,
    ) -> Result<(), BackendError> {
        let id = self.playlist_id(key)?;
        let parent = parent.map(|p| self.playlist_id(p)).transpose()?;
        self.library()?
            .move_playlist(id, parent, position)
            .map_err(error)
    }

    fn duplicate_playlist(
        &self,
        key: PlaylistKey,
        name: &str,
    ) -> Result<PlaylistKey, BackendError> {
        let id = self.playlist_id(key)?;
        let copy = self
            .library()?
            .duplicate_playlist(id, name)
            .map_err(error)?;
        let key = playlist_key(copy);
        lock(&self.playlists).insert(key, copy);
        Ok(key)
    }

    fn set_playlist_locked(&self, key: PlaylistKey, locked: bool) -> Result<(), BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?
            .set_playlist_locked(id, locked)
            .map_err(error)
    }

    fn set_playlist_grouped(&self, key: PlaylistKey, grouped: bool) -> Result<(), BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?
            .set_playlist_grouped(id, grouped)
            .map_err(error)
    }

    fn undo_playlist(&self, key: PlaylistKey) -> Result<bool, BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?.undo_playlist(id).map_err(error)
    }

    fn redo_playlist(&self, key: PlaylistKey) -> Result<bool, BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?.redo_playlist(id).map_err(error)
    }

    fn playlist_cleanup(&self, key: PlaylistKey) -> Result<PlaylistCleanup, BackendError> {
        let id = self.playlist_id(key)?;
        let counts = self.library()?.playlist_cleanup_counts(id).map_err(error)?;
        Ok(PlaylistCleanup {
            duplicates: counts.duplicates,
            missing: counts.missing,
        })
    }

    fn remove_playlist_duplicates(&self, key: PlaylistKey) -> Result<usize, BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?
            .remove_playlist_duplicates(id)
            .map_err(error)
    }

    fn remove_missing_from_playlist(&self, key: PlaylistKey) -> Result<usize, BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?
            .remove_missing_from_playlist(id)
            .map_err(error)
    }

    fn shuffle_playlist(&self, key: PlaylistKey) -> Result<(), BackendError> {
        let id = self.playlist_id(key)?;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        self.library()?.shuffle_playlist(id, seed).map_err(error)
    }

    fn create_smart_playlist(
        &self,
        name: &str,
        query: &Query,
        parent: Option<PlaylistKey>,
    ) -> Result<PlaylistKey, BackendError> {
        let parent = parent.map(|p| self.playlist_id(p)).transpose()?;
        let id = self
            .library()?
            .create_smart_playlist(name, query, parent)
            .map_err(error)?;
        let key = playlist_key(id);
        lock(&self.playlists).insert(key, id);
        Ok(key)
    }

    fn set_smart_query(&self, key: PlaylistKey, query: &Query) -> Result<(), BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?
            .set_smart_playlist_query(id, query)
            .map_err(error)
    }

    fn smart_query(&self, key: PlaylistKey) -> Result<Query, BackendError> {
        let id = self.playlist_id(key)?;
        self.library()?.smart_playlist_query(id).map_err(error)
    }

    fn smart_tracks(&self, key: PlaylistKey) -> Result<Vec<SmartTrack>, BackendError> {
        /// The most tracks given in full, with their format and cover.
        const FULL: usize = 2000;
        let id = self.playlist_id(key)?;
        let music = self.library()?;
        let summaries = music.smart_playlist_tracks(id).map_err(error)?;
        let ids: Vec<TrackId> = summaries.iter().map(|t| t.id).collect();
        let added = music.added_times(&ids).map_err(error)?;
        Ok(summaries
            .iter()
            .enumerate()
            .map(|(i, track)| {
                let brief = self.summary_row(track);
                let row = if i < FULL {
                    self.track_row(music, brief.key, brief.album, brief.copies)
                        .map(|mut row| {
                            row.missing = brief.missing;
                            row
                        })
                        .unwrap_or(brief)
                } else {
                    brief
                };
                SmartTrack {
                    row,
                    added: added.get(&track.id).copied(),
                }
            })
            .collect())
    }

    fn query_match_count(&self, query: &Query) -> Result<(usize, usize), BackendError> {
        self.library()?.query_match_count(query).map_err(error)
    }

    fn pins(&self) -> Result<Vec<PinRow>, BackendError> {
        let pins = self.library()?.pins().map_err(error)?;
        Ok(pins
            .iter()
            .map(|pin| {
                let key = match pin.target {
                    PinTarget::Album(id) => PinKey::Album(album_key(id)),
                    PinTarget::Artist(id) => {
                        PinKey::Artist(ArtistKey(u64::try_from(id.get()).unwrap_or(0)))
                    }
                    PinTarget::Playlist(id) => {
                        let key = playlist_key(id);
                        lock(&self.playlists).insert(key, id);
                        PinKey::Playlist(key)
                    }
                };
                PinRow {
                    key,
                    name: pin.name.clone(),
                    cover: pin.cover.as_ref().map(cover_key),
                    cover_seed: match key {
                        PinKey::Album(album) => seed(album),
                        _ => 0,
                    },
                    playlist: pin.playlist.map(playlist_type),
                }
            })
            .collect())
    }

    fn pin(&self, key: PinKey) -> Result<(), BackendError> {
        let target = self.pin_target(key)?;
        self.library()?.pin(target).map_err(error)
    }

    fn unpin(&self, key: PinKey) -> Result<(), BackendError> {
        let target = self.pin_target(key)?;
        self.library()?.unpin(target).map(|_| ()).map_err(error)
    }

    fn folder_roots(&self) -> Result<Vec<FolderRow>, BackendError> {
        let states = self.library()?.root_states().map_err(error)?;
        Ok(states
            .iter()
            .map(|root| FolderRow {
                path: root.path.clone(),
                name: display_path(&root.path),
                root: true,
                state: if root.masked {
                    FolderState::Absent
                } else if root.polled {
                    FolderState::Polled
                } else {
                    FolderState::Watched
                },
                has_children: !root.masked && has_subfolders(&root.path),
            })
            .collect())
    }

    // The tree is read from the file system: the library's folder table holds
    // only folders with scanned files, not the folders between them, nor the
    // other files the status bar counts.
    fn subfolders(&self, path: &Path) -> Result<Vec<FolderRow>, BackendError> {
        let entries = std::fs::read_dir(path).map_err(error)?;
        let mut rows: Vec<FolderRow> = entries
            .flatten()
            .filter(|e| {
                !e.file_name().as_encoded_bytes().starts_with(b".")
                    && e.file_type().is_ok_and(|t| t.is_dir())
            })
            .map(|e| {
                let path = e.path();
                FolderRow {
                    name: file_name(&path),
                    root: false,
                    state: FolderState::Watched,
                    has_children: has_subfolders(&path),
                    path,
                }
            })
            .collect();
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(rows)
    }

    fn folder(&self, path: &Path) -> Result<FolderListing, BackendError> {
        let music = self.library()?;
        let state = self
            .folder_roots()?
            .into_iter()
            .filter(|r| path.starts_with(&r.path))
            .map(|r| r.state)
            .next();
        let mut files = Vec::new();
        let mut other_files = 0;
        let mut entries: Vec<std::fs::DirEntry> =
            std::fs::read_dir(path).map_err(error)?.flatten().collect();
        entries.sort_by_key(std::fs::DirEntry::path);
        for entry in entries {
            let file = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() || entry.file_name().as_encoded_bytes().starts_with(b".") {
                continue;
            }
            if !is_audio(&file) {
                other_files += 1;
                continue;
            }
            let track = music.track_at(&file).ok().flatten().and_then(|id| {
                let (album, copies) = albums_and_copies(music, &[id])
                    .get(&id)
                    .copied()
                    .unwrap_or_default();
                self.track_row(music, TrackKey(id.get()), album, copies)
            });
            // A file outside the library is read for its title and length;
            // nothing is stored.
            let (title, duration) = if track.is_none() {
                browse::file_tags(&file)
            } else {
                (None, None)
            };
            files.push(FolderFile {
                title,
                duration,
                name: file_name(&file),
                size: entry.metadata().map_or(0, |m| m.len()),
                track,
                path: file,
            });
        }
        Ok(FolderListing {
            files,
            other_files,
            state,
        })
    }

    fn saved_searches(&self) -> Result<Vec<SavedSearchRow>, BackendError> {
        Ok(self
            .library()?
            .saved_searches()
            .map_err(error)?
            .into_iter()
            .map(|entry| SavedSearchRow {
                key: SavedSearchKey(u64::try_from(entry.search.id.get()).unwrap_or(0)),
                name: entry.search.name,
                text: entry.search.text,
                count: entry.count,
            })
            .collect())
    }

    fn save_search(&self, name: &str, query: &Query) -> Result<SavedSearchKey, BackendError> {
        let id = self.library()?.save_search(name, query).map_err(error)?;
        Ok(SavedSearchKey(u64::try_from(id.get()).unwrap_or(0)))
    }

    fn rename_saved_search(&self, key: SavedSearchKey, name: &str) -> Result<(), BackendError> {
        self.library()?
            .rename_saved_search(saved_search_id(key), name)
            .map_err(error)
    }

    fn delete_saved_search(&self, key: SavedSearchKey) -> Result<(), BackendError> {
        self.library()?
            .delete_saved_search(saved_search_id(key))
            .map_err(error)
    }

    fn composers(&self) -> Result<Vec<ArtistRow>, BackendError> {
        browse::composers(self.library()?)
    }

    fn years(&self) -> Result<Vec<YearRow>, BackendError> {
        browse::years(self.library()?)
    }

    fn composer_duration(&self, composer: ArtistKey) -> Result<Duration, BackendError> {
        browse::composer_duration(self.library()?, composer)
    }

    fn set_album_kind(&self, album: AlbumKey, kind: MediaKind) -> Result<(), BackendError> {
        let music = self.library()?;
        let id = lock(&self.ids).get(&album).copied();
        let Some(detail) = id
            .map(|id| music.album(id))
            .transpose()
            .map_err(error)?
            .flatten()
        else {
            return Ok(());
        };
        let tracks: Vec<TrackId> = detail
            .discs
            .iter()
            .flat_map(|d| &d.tracks)
            .map(|t| t.id)
            .collect();
        browse::set_kind(music, &tracks, kind)
    }

    fn forget_resume(&self, tracks: &[TrackKey]) -> Result<(), BackendError> {
        browse::forget_resume(self.library()?, tracks)
    }

    fn chapters(&self, track: TrackKey) -> Result<Vec<ChapterRow>, BackendError> {
        browse::chapters(self.library()?, track)
    }

    fn track_facts(&self, track: TrackKey) -> Result<Option<TrackFacts>, BackendError> {
        browse::track_facts(self.library()?, track)
    }

    fn facet_values(
        &self,
        query: &TrackQuery,
        chosen: &[super::FacetChoice],
        field: super::FacetField,
    ) -> Result<super::FacetList, BackendError> {
        self.facet_list(query, chosen, field)
    }

    fn facet_track_keys(
        &self,
        query: &TrackQuery,
        chosen: &[super::FacetChoice],
    ) -> Result<Vec<TrackKey>, BackendError> {
        self.facet_keys(query, chosen)
    }

    fn statistics(
        &self,
        since: super::StatisticsSince,
    ) -> Result<super::LibraryStats, BackendError> {
        self.library_statistics(since)
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
    /// Whether position updates reach the window (only while it is shown).
    positions: Arc<AtomicBool>,
    /// MPRIS, which lists the playlists.
    mpris: Option<Arc<Mpris>>,
    /// The analyzer's measurements, while a view shows them.
    analyzer: std::cell::RefCell<Option<AnalyzerWatch>>,
}

impl ServicePlayer {
    fn shuffle(&self, on: bool) {
        self.command(PlayerCommand::SetShuffle(if on {
            ShuffleMode::Tracks
        } else {
            ShuffleMode::Off
        }));
    }

    fn command(&self, command: PlayerCommand) {
        if self.service.player().send(command).is_err() {
            let _ = self.updates.try_send(Update::Problem {
                message: tr("The player has stopped. Restart Helikodu to play again."),
            });
        }
    }
}

impl PlayerControl for ServicePlayer {
    fn resume_pending(&self) -> bool {
        self.service.player().status().resume_pending
    }

    fn playlists_changed(&self) {
        if let Some(mpris) = self.mpris.clone() {
            // It reads the library and talks to the bus: not on the main thread.
            let _ = thread::Builder::new()
                .name("helikodu-mpris-playlists".into())
                .spawn(move || mpris.playlists_changed());
        }
    }

    fn follow_analyzer(&self, per_second: Option<u32>) {
        // The old feed ends before a new one starts.
        drop(self.analyzer.take());
        let Some(per_second) = per_second.filter(|&n| n > 0) else {
            return;
        };
        let updates = self.updates.clone();
        let interval = Duration::from_secs(1) / per_second;
        let watch = self
            .service
            .player()
            .watch_analyzer(interval, move |frame| {
                updates.try_send(Update::Analyzer(Arc::new(frame))).is_ok()
            });
        match watch {
            Ok(watch) => {
                self.analyzer.replace(Some(watch));
            }
            Err(e) => log::warn!("cannot start the analyzer: {e}"),
        }
    }

    fn follow_position(&self, shown: bool) {
        if self.positions.swap(shown, Ordering::Relaxed) || !shown {
            return;
        }
        // The position missed while hidden, at once.
        let status = self.service.player().status();
        if let Some(now) = status.now_playing {
            let _ = self.updates.try_send(Update::Position {
                elapsed: status.position.as_duration(),
                length: now.length.map(MediaTime::as_duration),
            });
        }
    }

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
            PlayerRequest::PlayTracks {
                tracks,
                start,
                title,
                shuffle,
            } => {
                self.command(PlayerCommand::SetShuffle(if shuffle {
                    ShuffleMode::Tracks
                } else {
                    ShuffleMode::Off
                }));
                self.command(PlayerCommand::PlayTracks {
                    context: QueueContext {
                        source: ContextSource::Query(title.clone()),
                        title: Some(title),
                    },
                    tracks: tracks
                        .into_iter()
                        .map(|t| TrackRef::Library(track_id(t)))
                        .collect(),
                    start,
                });
            }
            PlayerRequest::PlayPlaylist {
                key,
                tracks,
                start,
                title,
                shuffle,
            } => {
                self.shuffle(shuffle);
                self.command(PlayerCommand::PlayTracks {
                    context: QueueContext {
                        source: ContextSource::Playlist(key.0),
                        title: Some(title),
                    },
                    tracks: tracks
                        .into_iter()
                        .map(|t| TrackRef::Library(track_id(t)))
                        .collect(),
                    start,
                });
            }
            PlayerRequest::PlayArtist { artist, shuffle } => {
                self.shuffle(shuffle);
                self.command(PlayerCommand::PlayContext {
                    context: ContextRequest::Artist(person(artist)),
                    title: None,
                    start: 0,
                });
            }
            PlayerRequest::PlayGenre { genre, shuffle } => {
                self.shuffle(shuffle);
                self.command(PlayerCommand::PlayContext {
                    context: ContextRequest::Genre(genre_id(genre)),
                    title: None,
                    start: 0,
                });
            }
            PlayerRequest::PlayComposer { composer, shuffle } => {
                self.shuffle(shuffle);
                self.command(PlayerCommand::PlayContext {
                    context: ContextRequest::Composer(person(composer)),
                    title: None,
                    start: 0,
                });
            }
            PlayerRequest::PlayYears { years, shuffle } => {
                self.shuffle(shuffle);
                self.command(PlayerCommand::PlayContext {
                    context: ContextRequest::Years(browse::year_span(years)),
                    title: None,
                    start: 0,
                });
            }
            PlayerRequest::Enqueue { tracks, next } => self.command(PlayerCommand::Enqueue {
                tracks: tracks
                    .into_iter()
                    .map(|t| TrackRef::Library(track_id(t)))
                    .collect(),
                at: if next {
                    EnqueueAt::Next
                } else {
                    EnqueueAt::End
                },
            }),
            PlayerRequest::EnqueueFiles { files, next } => self.command(PlayerCommand::Enqueue {
                tracks: files
                    .into_iter()
                    .map(|f| TrackRef::File(MediaSource::file(f)))
                    .collect(),
                at: if next {
                    EnqueueAt::Next
                } else {
                    EnqueueAt::End
                },
            }),
            PlayerRequest::RemoveEntries(entries) => self.command(PlayerCommand::Remove(
                entries.into_iter().map(EntryId::new).collect(),
            )),
            PlayerRequest::MoveEntry { entry, to } => self.command(PlayerCommand::Move {
                entry: EntryId::new(entry),
                to,
            }),
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
            PlayerRequest::Play => self.command(PlayerCommand::Play),
            PlayerRequest::ResumeSession => self.command(PlayerCommand::ResumeSession),
            PlayerRequest::SessionEnding => self.command(PlayerCommand::SessionEnding),
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
            PlayerRequest::AdjustLyricsOffset(delta) => {
                if let Some(lyrics) = self.service.lyrics() {
                    lyrics.adjust_offset(delta);
                }
            }
            PlayerRequest::ReloadLyrics => {
                if let Some(lyrics) = self.service.lyrics() {
                    lyrics.reload();
                }
            }
            PlayerRequest::SetShuffleMode(mode) => self.command(PlayerCommand::SetShuffle(mode)),
            PlayerRequest::SetRepeatMode(mode) => self.command(PlayerCommand::SetRepeat(mode)),
            PlayerRequest::SetStopAfterMode(mode) => {
                self.command(PlayerCommand::SetStopAfter(mode));
            }
            PlayerRequest::SetConsume(on) => self.command(PlayerCommand::SetConsume(on)),
            PlayerRequest::SetPlayScope(scope) => self.command(PlayerCommand::SetPlayScope(scope)),
            PlayerRequest::SetFollowCursor(on) => self.command(PlayerCommand::SetFollowCursor(on)),
            PlayerRequest::SetCursor(entry) => {
                self.command(PlayerCommand::SetCursor(entry.map(EntryId::new)));
            }
            PlayerRequest::NextAlbum => self.command(PlayerCommand::NextAlbum),
            PlayerRequest::PreviousAlbum => self.command(PlayerCommand::PreviousAlbum),
            PlayerRequest::RandomAlbum => self.command(PlayerCommand::RandomAlbum),
            PlayerRequest::LoopStartHere => self.command(PlayerCommand::LoopStartHere),
            PlayerRequest::LoopEndHere => self.command(PlayerCommand::LoopEndHere),
            PlayerRequest::SetLoop(None) => self.command(PlayerCommand::SetLoop(None)),
            PlayerRequest::SetLoop(Some((start, end))) => {
                if let Some(range) = helikodu_model::player::LoopRange::new(
                    MediaTime::from_duration(start),
                    MediaTime::from_duration(end),
                ) {
                    self.command(PlayerCommand::SetLoop(Some(range)));
                }
            }
            PlayerRequest::StartSleepTimer { length, when } => {
                self.command(PlayerCommand::StartSleepTimer {
                    length,
                    when: match when {
                        super::SleepWhen::AtOnce => ServiceSleepWhen::AtOnce,
                        super::SleepWhen::AfterTrack => ServiceSleepWhen::AfterTrack,
                    },
                });
            }
            PlayerRequest::ExtendSleepTimer(by) => {
                self.command(PlayerCommand::ExtendSleepTimer(by));
            }
            PlayerRequest::CancelSleepTimer => self.command(PlayerCommand::CancelSleepTimer),
            PlayerRequest::SetTrackFlags { track, flags } => {
                self.command(PlayerCommand::SetTrackFlags {
                    track: track_id(track),
                    flags,
                });
            }
        }
    }
}

/// The A-B loop as the window shows it.
fn loop_points(ab: Option<helikodu_service::player::AbLoop>) -> Option<super::LoopPoints> {
    ab.map(|ab| super::LoopPoints {
        start: ab.start.map(MediaTime::as_duration),
        end: ab.end.map(MediaTime::as_duration),
    })
}

/// The sleep timer as the window shows it.
fn sleep_timer(
    timer: Option<helikodu_service::player::SleepTimerStatus>,
) -> Option<super::SleepTimer> {
    timer.map(|t| super::SleepTimer {
        remaining: t.remaining,
        length: t.length,
        when: if t.when == ServiceSleepWhen::AfterTrack {
            super::SleepWhen::AfterTrack
        } else {
            super::SleepWhen::AtOnce
        },
    })
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
        // Repeating the album is "on" for the coarse view, as for MPRIS.
        repeat: match modes.repeat {
            RepeatMode::One => Repeat::One,
            RepeatMode::All | RepeatMode::Album => Repeat::All,
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
        cover_seed: seed(album),
        album_title: info.album.clone().unwrap_or_default(),
        path: info.path.clone(),
        ..TrackRow::default()
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
    /// Whether the window is shown; position updates wait otherwise.
    positions: Arc<AtomicBool>,
    /// Whether the engine has reported the path of what is heard; the output
    /// button then follows the path rather than the decoded format.
    path_known: bool,
    /// What the current entry plays, for its waveform.
    playing: Option<MediaSource>,
    /// Counts the changes of the library.
    changes: Arc<AtomicU64>,
}

impl Forwarder {
    fn send(&self, update: Update) -> bool {
        self.updates.send_blocking(update).is_ok()
    }

    /// Sends the waveform of `track`, which now plays: its peaks if they are
    /// stored, otherwise none until they are.
    fn send_waveform(&mut self, track: &TrackRef) -> bool {
        self.playing = match track {
            TrackRef::File(source) => Some(source.clone()),
            TrackRef::Library(id) => self
                .service
                .library()
                .and_then(|l| l.library().track_source(*id).ok().flatten())
                .map(|found| found.source),
        };
        let peaks = self
            .playing
            .as_ref()
            .and_then(|source| self.service.waveforms()?.peaks(source));
        self.send(Update::Waveform(peaks))
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
                    ..TrackRow::default()
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
        let service = self.service.clone();
        let _ = library.describe(tracks, move |infos| {
            let Ok(infos) = infos else { return };
            let music = service.library().map(LibraryService::library);
            let album = Self::context_album(&snapshot);
            let ids: Vec<TrackId> = infos
                .iter()
                .filter_map(|info| match &info.track {
                    TrackRef::Library(id) => Some(*id),
                    _ => None,
                })
                .collect();
            let known = music
                .map(|music| albums_and_copies(music, &ids))
                .unwrap_or_default();
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
                    let mut row = track_row(info, Some(entry.id), entry.from_context, album);
                    if let TrackRef::Library(id) = &info.track {
                        row.copies = known.get(id).map_or(0, |(_, copies)| *copies);
                    }
                    row
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
                path: Some(path.clone()),
                ..TrackRow::default()
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
                // The path of the new track may have come first; otherwise it
                // follows, and until then the old one stands.
                if !self.path_known {
                    self.output.signal = SignalState::Unreported;
                    self.output.sample_rate = Some(spec.rate.hz());
                    self.output.bits = bits;
                }
                let format = AudioFormat {
                    codec: path.extension().map(|e| e.to_string_lossy().to_uppercase()),
                    sample_rate: Some(spec.rate.hz()),
                    bits,
                    bitrate_kbps: None,
                    channels: Some(spec.channel_count()),
                };
                self.send_now_playing(*entry, track.clone(), path.clone(), format);
                self.send_waveform(track)
                    && self.send(Update::Position {
                        elapsed: Duration::ZERO,
                        length: length.map(MediaTime::as_duration),
                    })
                    && self.send(Update::Output(self.output.clone()))
            }
            // Nothing draws the position while the window is hidden.
            PlayerEvent::Position { .. } if !self.positions.load(Ordering::Relaxed) => true,
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
            PlayerEvent::ModesChanged(m) => {
                self.send(modes(*m)) && self.send(Update::PlayModes(*m))
            }
            PlayerEvent::StopAfterChanged(stop) => {
                self.send(Update::StopAfter(*stop != StopAfter::Off))
                    && self.send(Update::StopAfterMode(*stop))
            }
            PlayerEvent::LoopChanged(ab) => self.send(Update::Loop(loop_points(*ab))),
            PlayerEvent::SleepTimerChanged(timer) => {
                self.send(Update::SleepTimer(sleep_timer(*timer)))
            }
            PlayerEvent::CursorChanged(entry) => self.send(Update::Cursor(entry.map(EntryId::get))),
            PlayerEvent::TrackFlagsChanged { track, flags } => self.send(Update::TrackFlags {
                track: TrackKey(track.get()),
                flags: *flags,
            }),
            PlayerEvent::VolumeChanged(volume) => {
                self.send(Update::Volume(f64::from(volume.position())))
            }
            PlayerEvent::SignalPath(path) => {
                self.path_known = path.is_some();
                if let Some(path) = path {
                    self.output.follow(path);
                }
                self.send(Update::SignalPath(path.clone()))
            }
            // Like the position, the counters wait while the window is hidden.
            PlayerEvent::PlaybackStats(_) if !self.positions.load(Ordering::Relaxed) => true,
            PlayerEvent::PlaybackStats(stats) => self.send(Update::Stats(*stats)),
            PlayerEvent::OutputChanged(selection) => {
                self.output.device = output_name(selection);
                self.send(Update::Output(self.output.clone()))
            }
            // The window tells of a lost device itself, with the output
            // button showing it as not connected.
            PlayerEvent::Problem {
                error: EngineError::DeviceLost { .. },
                ..
            } => {
                self.output.signal = SignalState::NotConnected;
                self.output.sample_rate = None;
                self.output.bits = None;
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
                    Notice::FlagsNotSaved { reason, .. } => Some(fill(
                        &tr("The playback settings of the track could not be saved: {}"),
                        &[reason],
                    )),
                    _ => None,
                };
                message.is_none_or(|message| self.send(Update::Notice { message }))
            }
            PlayerEvent::Ended { reason, .. } => {
                self.output.signal = SignalState::Idle;
                self.output.sample_rate = None;
                self.output.bits = None;
                // The sleep timer pauses, keeping the position, as stop-after does.
                let state = if matches!(*reason, EndReason::StopAfter | EndReason::SleepTimer) {
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
            LibraryEvent::Changed => {
                self.changes.fetch_add(1, Ordering::Relaxed);
                self.send(Update::LibraryChanged)
            }
            LibraryEvent::RatingChanged { track, stars } => self.send(Update::RatingChanged {
                track: TrackKey(track.get()),
                stars: *stars,
            }),
            LibraryEvent::Scan(_) | LibraryEvent::ScanEnded(_) => {
                if matches!(event, LibraryEvent::ScanEnded(_)) {
                    services::follow_folders(&self.service, false);
                }
                services::scan_update(event).is_none_or(|update| self.send(update))
            }
            LibraryEvent::Loudness(_) => {
                services::loudness_update(event).is_none_or(|update| self.send(update))
            }
            LibraryEvent::Analysis(progress) => self.send(Update::Analysis(*progress)),
            LibraryEvent::DuplicatesApplied(result) => self.send(Update::DuplicatesApplied(
                result
                    .as_ref()
                    .map(|summary| summary.read)
                    .map_err(Clone::clone),
            )),
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
            && self.send(Update::State(playback(status.playback)))
            && self.send(Update::PlayModes(status.modes))
            && self.send(Update::StopAfterMode(status.stop_after))
            && self.send(Update::Loop(loop_points(status.ab_loop)))
            && self.send(Update::SleepTimer(sleep_timer(status.sleep_timer)))
            && self.send(Update::Cursor(status.cursor.map(EntryId::get)));
        self.send_queue();
        if let Some(path) = &status.signal_path {
            self.path_known = true;
            self.output.follow(path);
            let _ = self.send(Update::Output(self.output.clone()));
        }
        let _ = self.send(Update::SignalPath(status.signal_path.clone()));
        if let Some(stats) = status.stats {
            let _ = self.send(Update::Stats(stats));
        }
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
            let _ = self.send_waveform(&now.track);
            let _ = self.send(Update::Position {
                elapsed: status.position.as_duration(),
                length: now.length.map(MediaTime::as_duration),
            });
        }
        for update in lyrics::initial(&self.service) {
            let _ = self.send(update);
        }
        if let Some(mpris) = &self.mpris {
            mpris.sync(self.service.player());
        }
    }

    fn run(mut self, events: &Subscription<Event>) {
        self.initial();
        loop {
            // The timeout only notices a closed window while nothing happens;
            // a long one keeps an idle player from waking this thread often.
            match events.recv_timeout(Duration::from_secs(5)) {
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
                Ok(Event::Lyrics(event)) => {
                    for update in lyrics::updates(&event) {
                        if !self.send(update) {
                            return;
                        }
                    }
                }
                Ok(Event::Waveform(WaveformEvent::Ready { source })) => {
                    if self.playing.as_ref() == Some(&source) {
                        let peaks = self.service.waveforms().and_then(|w| w.peaks(&source));
                        if !self.send(Update::Waveform(peaks)) {
                            return;
                        }
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
        let requests = sender.clone();
        let commands = service.player().sender();
        // What would start playback goes to the window, through the full-output
        // gate; the rest goes straight to the player.
        let describing = Arc::clone(&service);
        let playlists = mpris::Playlists {
            service: Arc::clone(&service),
            updates: sender.clone(),
        };
        let hooks = MprisHooks::new(move |command| {
            let request = match command {
                PlayerCommand::Play => PlayerRequest::Play,
                PlayerCommand::PlayPause => PlayerRequest::PlayPause,
                PlayerCommand::PlayFiles { files, .. } => PlayerRequest::PlayFiles {
                    files,
                    enqueue: false,
                },
                other => {
                    // A player that has ended takes no commands.
                    let _ = commands.send(other);
                    return;
                }
            };
            let _ = requests.try_send(Update::Request(request));
        })
        .with_raise(move || {
            let _ = raise.try_send(Update::Raise);
        })
        .with_quit(move || {
            let _ = quit.try_send(Update::Quit);
        })
        .with_describe(move |tracks| mpris::describe(&describing, tracks))
        .with_playlists(playlists);
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
            .follow(events, |e| log::warn!("cannot keep the system awake: {e}"))
            .ok()
    } else {
        None
    };
    for message in notices {
        log::warn!("{message}");
    }
    let events =
        service.subscribe(SubscribeOptions::all().with_position_interval(POSITION_INTERVAL));
    let positions = Arc::new(AtomicBool::new(true));
    let changes = Arc::new(AtomicU64::new(0));
    let forwarder = Forwarder {
        service: Arc::clone(&service),
        updates: sender.clone(),
        mpris: mpris.clone(),
        output: OutputInfo::default(),
        pending_cover: Arc::new(Mutex::new(None)),
        positions: Arc::clone(&positions),
        path_known: false,
        playing: None,
        changes: Arc::clone(&changes),
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
        playlists: Mutex::new(HashMap::new()),
        album_rows: Mutex::new(Vec::new()),
        listed: Mutex::new(None),
        artist_rows: Mutex::new(Vec::new()),
        not_artists: Mutex::new((0, HashSet::new())),
        changes,
    });
    let player = Rc::new(ServicePlayer {
        service: Arc::clone(&service),
        library: Arc::clone(&library),
        updates: sender,
        positions,
        mpris,
        analyzer: std::cell::RefCell::new(None),
    });
    services::follow_folders(&service, true);
    let services = Arc::new(services::ServiceServices {
        service: Arc::clone(&service),
        opened: Mutex::new(HashMap::new()),
    });
    Ok(Backend::new(library, player, receiver)
        .with_ui_state(Rc::new(ServiceUiState(service)))
        .with_services(services))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_and_shuffle_map_both_ways() {
        let update = modes(PlayModes {
            shuffle: ShuffleMode::Tracks,
            repeat: RepeatMode::One,
            ..PlayModes::default()
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
