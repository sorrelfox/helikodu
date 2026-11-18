// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The front end's view of the core: a small adapter between the widgets and
//! the `service` crate.
//!
//! Widgets and models see only the types of this module. [`Backend`] bundles a
//! [`LibrarySource`], which worker threads query for library data, and a
//! [`PlayerControl`], which takes [`PlayerRequest`]s on the main thread, and a
//! channel of [`Update`]s from the player and the library. [`service::connect`] builds one
//! over the running service; [`sample::backend`] builds one over invented
//! content for tests, screenshots and the demonstration mode. When the
//! service's interface changes, only `service.rs` follows it.

pub mod sample;
pub mod service;
pub mod services;

use std::cell::RefCell;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub use helikodu_model::player::{
    PlaybackStats, SignalLamp, SignalPath, SignalStage, StageKind, StageState,
};
pub use helikodu_model::settings::UiState;

/// Identifies an album for as long as the backend runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct AlbumKey(pub u64);

/// Identifies a track for as long as the backend runs. `TrackKey(0)` is a track
/// that is not in the library: a loose file or a missing playlist entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct TrackKey(pub u64);

/// Identifies an artist (an album artist) for as long as the backend runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ArtistKey(pub u64);

/// Identifies a genre for as long as the backend runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct GenreKey(pub u64);

/// Identifies a static playlist for as long as the backend runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct PlaylistKey(pub u64);

/// Identifies a cover picture: the content hash the thumbnail cache uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoverKey(pub [u8; 32]);

/// Where a thumbnail stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThumbnailState {
    /// The file is there.
    Ready(PathBuf),
    /// It is being made; [`Update::Thumbnail`] follows.
    Pending,
    /// There is none.
    Unavailable,
}

/// Why the backend could not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError(pub String);

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackendError {}

/// The order of the album grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AlbumSort {
    /// By album artist, then year.
    #[default]
    AlbumArtist,
    /// By title.
    Title,
    /// By year, newest first.
    Year,
    /// Most recently added first.
    DateAdded,
}

/// The quick filter of the album grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AlbumFilter {
    /// Every album.
    #[default]
    All,
    /// Albums whose tracks are all in a lossless format.
    Lossless,
    /// Lossless albums above 48 kHz or 16 bits.
    HiRes,
}

/// Which part of the library an album list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AlbumScope {
    /// Every album.
    #[default]
    All,
    /// The albums of an album artist.
    Artist(ArtistKey),
    /// The albums an artist appears on without being their album artist.
    AppearsOn(ArtistKey),
    /// The albums with a track of a genre.
    Genre(GenreKey),
}

/// Which albums, in which order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct AlbumQuery {
    /// The order.
    pub sort: AlbumSort,
    /// The quick filter.
    pub filter: AlbumFilter,
    /// The part of the library.
    pub scope: AlbumScope,
}

/// Totals for the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LibraryCounts {
    /// Albums.
    pub albums: usize,
    /// Track and album artists.
    pub artists: usize,
    /// Tracks.
    pub tracks: usize,
    /// Genres.
    pub genres: usize,
}

/// The audio format of a track or album, as far as it is known.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioFormat {
    /// The codec, for example `FLAC`.
    pub codec: Option<String>,
    /// The sample rate in Hz.
    pub sample_rate: Option<u32>,
    /// The bit depth of lossless formats.
    pub bits: Option<u16>,
    /// The bit rate of lossy formats, in kbit/s.
    pub bitrate_kbps: Option<u32>,
    /// The number of channels.
    pub channels: Option<u16>,
}

/// One album tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumRow {
    /// The album.
    pub key: AlbumKey,
    /// Its title.
    pub title: String,
    /// Its album artist.
    pub artist: String,
    /// Its year.
    pub year: Option<i32>,
    /// Its format, as far as the list knows it.
    pub format: AudioFormat,
    /// The number of tracks.
    pub track_count: usize,
    /// A stable value that picks the generated cover when there is no art.
    pub cover_seed: u64,
    /// Its cover, if it has one.
    pub cover: Option<CoverKey>,
}

/// One track of an album, a list or the queue.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackRow {
    /// The track.
    pub key: TrackKey,
    /// Its album.
    pub album: AlbumKey,
    /// Its number on its disc.
    pub number: Option<u32>,
    /// Its disc.
    pub disc: Option<u32>,
    /// Its title.
    pub title: String,
    /// Its artists.
    pub artist: String,
    /// Its length.
    pub duration: Option<Duration>,
    /// How often it was played.
    pub plays: u32,
    /// Its rating, 1 to 5.
    pub rating: Option<u8>,
    /// Its queue entry, for a track in the queue.
    pub entry: Option<u64>,
    /// In the queue: whether it came with the queue's context rather than being
    /// queued from elsewhere.
    pub from_context: bool,
    /// Its album's cover, if known.
    pub cover: Option<CoverKey>,
    /// The seed of its album's generated cover.
    pub cover_seed: u64,
    /// Its album's title.
    pub album_title: String,
    /// Its year.
    pub year: Option<i32>,
    /// Its format, as far as known.
    pub format: AudioFormat,
    /// Its file, if known.
    pub path: Option<PathBuf>,
    /// Whether it is missing: a playlist entry whose track left the library.
    pub missing: bool,
}

/// One disc of an album.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscGroup {
    /// The disc number, if the album has several.
    pub number: Option<u32>,
    /// Its subtitle.
    pub subtitle: Option<String>,
    /// Its tracks, in order.
    pub tracks: Vec<TrackRow>,
}

/// Everything the album page shows.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumDetail {
    /// The tile data.
    pub row: AlbumRow,
    /// The discs.
    pub discs: Vec<DiscGroup>,
    /// The genres, joined.
    pub genre: Option<String>,
    /// When it was added.
    pub added: Option<String>,
    /// The size of its files, in bytes.
    pub size: Option<u64>,
    /// Its folder.
    pub folder: Option<String>,
    /// The album gain in dB.
    pub album_gain_db: Option<f64>,
}

impl AlbumDetail {
    /// Every track, disc by disc.
    pub fn tracks(&self) -> impl Iterator<Item = &TrackRow> {
        self.discs.iter().flat_map(|d| d.tracks.iter())
    }

    /// The length of all tracks together.
    pub fn duration(&self) -> Duration {
        self.tracks().filter_map(|t| t.duration).sum()
    }
}

/// One artist of the artist list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArtistRow {
    /// The artist.
    pub key: ArtistKey,
    /// The name as credited.
    pub name: String,
    /// The name it sorts by ("Linden Quartet, The").
    pub sort_name: String,
    /// The number of albums.
    pub album_count: usize,
    /// The number of tracks.
    pub track_count: usize,
}

/// Everything the artist page shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArtistDetail {
    /// The list row.
    pub row: ArtistRow,
    /// The albums.
    pub albums: Vec<AlbumRow>,
    /// The EPs and singles.
    pub singles: Vec<AlbumRow>,
    /// The albums the artist appears on.
    pub appears_on: Vec<AlbumRow>,
    /// The most played tracks, most played first.
    pub most_played: Vec<TrackRow>,
    /// The length of all their tracks.
    pub duration: Duration,
}

/// One genre of the genre list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GenreRow {
    /// The genre.
    pub key: GenreKey,
    /// Its name.
    pub name: String,
    /// The number of albums with a track of it.
    pub album_count: usize,
    /// The number of its tracks.
    pub track_count: usize,
}

/// A column of the track table, which is also a sort key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrackColumn {
    /// Disc and track number.
    Number,
    /// The title.
    Title,
    /// The artists.
    Artist,
    /// The album title.
    Album,
    /// The year.
    Year,
    /// The audio format.
    Format,
    /// The rating.
    Rating,
    /// How often it was played.
    Plays,
    /// The length.
    Length,
}

/// One key of a track list's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackSort {
    /// The column.
    pub column: TrackColumn,
    /// Largest first.
    pub descending: bool,
}

/// Which part of the library a track list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TrackScope {
    /// Every track.
    #[default]
    All,
    /// The tracks of an artist, in any role.
    Artist(ArtistKey),
    /// The tracks of a genre.
    Genre(GenreKey),
}

/// Which tracks, in which order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TrackQuery {
    /// The part of the library.
    pub scope: TrackScope,
    /// Words the tracks must match, as in search; empty for all.
    pub text: String,
    /// The order, most significant key first; empty for album order.
    pub sort: Vec<TrackSort>,
}

/// A static playlist in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistRow {
    /// The playlist.
    pub key: PlaylistKey,
    /// Its name.
    pub name: String,
    /// The number of its entries.
    pub items: usize,
    /// The length of its entries.
    pub duration: Duration,
    /// When it last changed.
    pub modified: Option<SystemTime>,
}

/// What a search found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchResults {
    /// Matching artists.
    pub artists: Vec<ArtistRow>,
    /// Matching albums.
    pub albums: Vec<AlbumRow>,
    /// The best matching tracks.
    pub tracks: Vec<TrackRow>,
    /// The number of matching tracks.
    pub track_total: usize,
}

/// How a library folder is kept current.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FolderState {
    /// Changes are seen as they happen.
    #[default]
    Watched,
    /// It is rescanned now and then (a network or FUSE file system).
    Polled,
    /// Its drive is not connected.
    Absent,
}

/// A folder of the folder tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRow {
    /// The folder.
    pub path: PathBuf,
    /// Its name as shown: the path with `~` for a library folder, else the name.
    pub name: String,
    /// Whether it is a library folder.
    pub root: bool,
    /// How it is kept current.
    pub state: FolderState,
    /// Whether it has folders in it.
    pub has_children: bool,
}

/// An audio file of a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderFile {
    /// The file.
    pub path: PathBuf,
    /// Its name as shown (lossily where it is not UTF-8).
    pub name: String,
    /// Its track, when it is in the library.
    pub track: Option<TrackRow>,
    /// Its size in bytes.
    pub size: u64,
}

/// The audio files of a folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderListing {
    /// The audio files, by name.
    pub files: Vec<FolderFile>,
    /// The number of other files.
    pub other_files: usize,
    /// Whether the folder is in the library, and how it is kept current.
    pub state: Option<FolderState>,
}

/// Library data. Called on worker threads, never on the main thread.
pub trait LibrarySource: Send + Sync + fmt::Debug {
    /// The totals for the sidebar.
    fn counts(&self) -> Result<LibraryCounts, BackendError>;
    /// The albums matching `query`, in its order.
    fn album_keys(&self, query: &AlbumQuery) -> Result<Vec<AlbumKey>, BackendError>;
    /// The tiles at positions `offset..offset + limit` of `query`'s list.
    fn album_rows(
        &self,
        query: &AlbumQuery,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<AlbumRow>, BackendError>;
    /// One album with its tracks.
    fn album(&self, key: AlbumKey) -> Result<Option<AlbumDetail>, BackendError>;
    /// Whether the lossless and hi-res quick filters work.
    fn supports_quality_filters(&self) -> bool {
        true
    }
    /// The thumbnail of `cover` at `size` device pixels (48, 96, 192 or 384).
    /// Never blocks: it may be scheduled. Called on the main thread.
    fn thumbnail(&self, cover: &CoverKey, size: u32) -> ThumbnailState {
        let _ = (cover, size);
        ThumbnailState::Unavailable
    }
    /// Drops the scheduled thumbnails for which `keep` is false.
    fn retain_thumbnails(&self, keep: &mut dyn FnMut(&CoverKey, u32) -> bool) {
        let _ = keep;
    }
    /// The album artists, in the order of their sort names.
    fn artists(&self) -> Result<Vec<ArtistRow>, BackendError>;
    /// One artist with their releases and most played tracks.
    fn artist(&self, key: ArtistKey) -> Result<Option<ArtistDetail>, BackendError>;
    /// The artist named `name`, ignoring case.
    fn find_artist(&self, name: &str) -> Result<Option<ArtistKey>, BackendError>;
    /// The genres, by name.
    fn genres(&self) -> Result<Vec<GenreRow>, BackendError>;
    /// The tracks matching `query`, in its order.
    fn track_keys(&self, query: &TrackQuery) -> Result<Vec<TrackKey>, BackendError>;
    /// The rows of `keys`; a key without a track is left out.
    fn track_rows(&self, keys: &[TrackKey]) -> Result<Vec<TrackRow>, BackendError>;
    /// Whether track lists can be sorted by `column`.
    fn sortable(&self, column: TrackColumn) -> bool {
        let _ = column;
        true
    }
    /// The artists, albums and tracks matching `text`.
    fn search(&self, text: &str) -> Result<SearchResults, BackendError>;
    /// Sets the rating of `track` in whole stars, or clears it.
    fn set_rating(&self, track: TrackKey, stars: Option<u8>) -> Result<(), BackendError>;
    /// The static playlists, in the order they were made.
    fn playlists(&self) -> Result<Vec<PlaylistRow>, BackendError>;
    /// The entries of a playlist, in order; each `entry` is its position.
    fn playlist_items(&self, key: PlaylistKey) -> Result<Vec<TrackRow>, BackendError>;
    /// Makes an empty playlist.
    fn create_playlist(&self, name: &str) -> Result<PlaylistKey, BackendError>;
    /// Renames a playlist.
    fn rename_playlist(&self, key: PlaylistKey, name: &str) -> Result<(), BackendError>;
    /// Deletes a playlist.
    fn delete_playlist(&self, key: PlaylistKey) -> Result<(), BackendError>;
    /// Inserts `tracks` at `position`, or adds them at the end.
    fn add_to_playlist(
        &self,
        key: PlaylistKey,
        tracks: &[TrackKey],
        position: Option<usize>,
    ) -> Result<(), BackendError>;
    /// Moves the entries at `positions` before the entry now at `to`.
    fn move_in_playlist(
        &self,
        key: PlaylistKey,
        positions: &[usize],
        to: usize,
    ) -> Result<(), BackendError>;
    /// Removes the entries at `positions`.
    fn remove_from_playlist(
        &self,
        key: PlaylistKey,
        positions: &[usize],
    ) -> Result<(), BackendError>;
    /// Imports a playlist file; returns the new playlist and its name.
    fn import_playlist(&self, path: &Path) -> Result<(PlaylistKey, String), BackendError>;
    /// Exports a playlist to a file in the format its extension names (M3U8,
    /// M3U, PLS or XSPF; M3U8 for any other); returns the number of entries
    /// written.
    fn export_playlist(&self, key: PlaylistKey, path: &Path) -> Result<usize, BackendError>;
    /// The library folders.
    fn folder_roots(&self) -> Result<Vec<FolderRow>, BackendError>;
    /// The folders in `path`, by name.
    fn subfolders(&self, path: &Path) -> Result<Vec<FolderRow>, BackendError>;
    /// The audio files in `path`.
    fn folder(&self, path: &Path) -> Result<FolderListing, BackendError>;
}

/// The window state kept between runs.
pub trait UiStateAccess: fmt::Debug {
    /// The saved state, or the defaults.
    fn load(&self) -> UiState;
    /// Saves `state`.
    fn save(&self, state: &UiState) -> Result<(), BackendError>;
}

/// Keeps the window state in memory only.
#[derive(Debug, Default)]
pub struct MemoryUiState(RefCell<UiState>);

impl UiStateAccess for MemoryUiState {
    fn load(&self) -> UiState {
        self.0.borrow().clone()
    }

    fn save(&self, state: &UiState) -> Result<(), BackendError> {
        self.0.replace(state.clone());
        Ok(())
    }
}

/// What the user asks the player to do.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerRequest {
    /// Play an album from one of its tracks (an index into all its tracks).
    PlayAlbum {
        /// The album.
        album: AlbumKey,
        /// The first track to play.
        start: usize,
        /// Play its tracks in random order.
        shuffle: bool,
    },
    /// Play files, replacing the queue, or add them to its end.
    PlayFiles {
        /// The files.
        files: Vec<PathBuf>,
        /// Add them instead of replacing the queue.
        enqueue: bool,
    },
    /// Play tracks, replacing the queue; `title` names where they came from.
    PlayTracks {
        /// The tracks, in order.
        tracks: Vec<TrackKey>,
        /// The index of the first to play.
        start: usize,
        /// Where they came from, as the queue panel shows it.
        title: String,
        /// Play them in random order.
        shuffle: bool,
    },
    /// Play all tracks of an artist.
    PlayArtist {
        /// The artist.
        artist: ArtistKey,
        /// In random order.
        shuffle: bool,
    },
    /// Play all tracks of a genre.
    PlayGenre {
        /// The genre.
        genre: GenreKey,
        /// In random order.
        shuffle: bool,
    },
    /// "Play next" (`next`) or "Add to queue".
    Enqueue {
        /// The tracks.
        tracks: Vec<TrackKey>,
        /// After the current track instead of at the end.
        next: bool,
    },
    /// Remove these queue entries.
    RemoveEntries(Vec<u64>),
    /// Move a queue entry to an index in play order (counted without it).
    MoveEntry {
        /// The entry.
        entry: u64,
        /// Where to.
        to: usize,
    },
    /// Play this queue entry now.
    PlayEntry(u64),
    /// Empty the queue except the playing entry.
    ClearQueue,
    /// Shuffle on or off.
    SetShuffle(bool),
    /// Repeat mode.
    SetRepeat(Repeat),
    /// Stop after the current track, or not.
    SetStopAfter(bool),
    /// Play or pause.
    PlayPause,
    /// Play, or go on playing; nothing changes while playing.
    Play,
    /// Stop.
    Stop,
    /// The next track.
    Next,
    /// The previous track, or the start of this one.
    Previous,
    /// A position in the current track.
    Seek(Duration),
    /// Forward or back by this many seconds.
    SeekBy(f64),
    /// The volume control, 0 to 1.
    SetVolume(f64),
}

/// What happens at the end of a track or of the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    /// Playback ends after the last track.
    #[default]
    Off,
    /// The queue starts again.
    All,
    /// The track plays again.
    One,
}

impl Repeat {
    /// The next mode of the repeat button: off, all, one.
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

/// The transport state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackState {
    /// Nothing is playing.
    #[default]
    Idle,
    /// A track is being opened.
    Loading,
    /// Playing.
    Playing,
    /// Paused.
    Paused,
}

/// What the signal lamp shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignalState {
    /// Nothing is playing.
    #[default]
    Idle,
    /// Playing, but the engine has not reported the signal path.
    Unreported,
    /// The samples reach the device unchanged.
    BitPerfect,
    /// Something changes the samples; `dsp` when a DSP stage does.
    Processed {
        /// Whether a DSP stage is on.
        dsp: bool,
    },
    /// The device is gone.
    NotConnected,
}

/// The output button's content.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OutputInfo {
    /// The device name.
    pub device: String,
    /// The signal state.
    pub signal: SignalState,
    /// The output's sample rate in Hz.
    pub sample_rate: Option<u32>,
    /// The output's bit depth.
    pub bits: Option<u16>,
}

impl SignalState {
    /// The lamp of a reported path: bit-perfect, or processed in any other way
    /// (gain, DSP, resampling, dither, fewer bits, a sound server's mixing).
    pub fn of(path: &SignalPath) -> Self {
        match path.lamp {
            SignalLamp::BitPerfect => Self::BitPerfect,
            SignalLamp::LosslessProcessed | SignalLamp::Resampled | SignalLamp::Lossy => {
                Self::Processed {
                    dsp: path.dsp_active(),
                }
            }
        }
    }
}

/// The bits that reach the device: the source's on a bit-perfect path (24-bit
/// audio in 32-bit words is 24-bit), otherwise those of the format the device
/// was opened with.
pub fn output_bits(path: &SignalPath) -> u32 {
    if path.lamp == SignalLamp::BitPerfect {
        path.source.bits
    } else {
        path.device.format.format.bits()
    }
}

impl OutputInfo {
    /// Follows the path the engine reported: its lamp, and the rate and bits
    /// of the format the device was opened with.
    pub fn follow(&mut self, path: &SignalPath) {
        self.signal = SignalState::of(path);
        self.sample_rate = Some(path.device.format.rate.hz());
        self.bits = u16::try_from(output_bits(path)).ok();
    }
}

/// The track that plays.
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlaying {
    /// The track.
    pub track: TrackRow,
    /// Its album's title.
    pub album_title: String,
    /// Its album's year.
    pub year: Option<i32>,
    /// Its album's cover seed.
    pub cover_seed: u64,
    /// Its cover, if it has one.
    pub cover: Option<CoverKey>,
    /// The format of the track.
    pub format: AudioFormat,
}

/// The queue as the panel shows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QueueSnapshot {
    /// What the queue continues from, for example an album title.
    pub context: Option<String>,
    /// The tracks, in playing order.
    pub items: Vec<TrackRow>,
    /// The index of the current track.
    pub current: Option<usize>,
}

/// What the core reports.
// A few updates a second pass through a channel; boxing the large ones would
// save nothing that matters.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// The transport state changed.
    State(PlaybackState),
    /// Another track plays, or none.
    NowPlaying(Option<NowPlaying>),
    /// The position in the current track.
    Position {
        /// From its start.
        elapsed: Duration,
        /// Its length, if known.
        length: Option<Duration>,
    },
    /// The volume control moved, 0 to 1.
    Volume(f64),
    /// The output or its state changed.
    Output(OutputInfo),
    /// The queue changed.
    Queue(QueueSnapshot),
    /// Shuffle and repeat.
    Modes {
        /// Shuffle on.
        shuffle: bool,
        /// Repeat mode.
        repeat: Repeat,
    },
    /// Stop-after is on or off.
    StopAfter(bool),
    /// Something went wrong; playback goes on where it can.
    Problem {
        /// What to tell the user.
        message: String,
    },
    /// Something the user should know that is not an error.
    Notice {
        /// What to tell the user.
        message: String,
    },
    /// A thumbnail asked for earlier is ready, or cannot be made.
    Thumbnail {
        /// The cover.
        cover: CoverKey,
        /// Its size in device pixels.
        size: u32,
        /// The file, if it was made.
        path: Option<PathBuf>,
    },
    /// The library changed: lists should be fetched again.
    LibraryChanged,
    /// A track's rating was stored (whole stars, or none): the rows showing it
    /// change in place.
    RatingChanged {
        /// The track.
        track: TrackKey,
        /// Its stars.
        stars: Option<u8>,
    },
    /// A library scan made progress or ended.
    Scan(services::ScanStatus),
    /// The signal path of what is heard, as the engine reported it; `None`
    /// when nothing is heard.
    SignalPath(Option<Box<SignalPath>>),
    /// The output's counters, a few times a second while playing.
    Stats(PlaybackStats),
    /// A loudness measurement made progress or ended.
    Loudness(services::LoudnessStatus),
    /// Another program asked the player for this (MPRIS `Play`, `PlayPause`,
    /// `OpenUri`). It is sent with [`Backend::send`], so it passes the same gate
    /// as the window's own requests.
    Request(PlayerRequest),
    /// Another program asked to show the window (MPRIS `Raise`).
    Raise,
    /// Another program asked to quit (MPRIS `Quit`).
    Quit,
}

/// Takes requests on the main thread.
pub trait PlayerControl: fmt::Debug {
    /// Hands `request` to the player.
    fn send(&self, request: PlayerRequest);

    /// Whether the window is shown and wants position updates; while it is
    /// not, they are not sent, and the current position follows when it is
    /// shown again.
    fn follow_position(&self, _shown: bool) {}
}

/// The front end's handle on the core.
#[derive(Debug, Clone)]
pub struct Backend {
    library: Arc<dyn LibrarySource>,
    player: Rc<dyn PlayerControl>,
    updates: Rc<RefCell<Option<async_channel::Receiver<Update>>>>,
    ui_state: Rc<dyn UiStateAccess>,
    services: Arc<dyn services::Services>,
    gate: GateSlot,
    workers: Workers,
}

/// How many threads run the library queries and load the covers. One: the
/// C allocator gives each thread its own arena and keeps what a large query
/// freed there (the 100,000-track sort, the album pages), so a second thread
/// kept its own copy of that memory, some 10 MB, for little gain in speed.
const WORKERS: usize = 1;

type Job = Box<dyn FnOnce() + Send>;

/// The threads that run library queries and load covers. Long-lived threads
/// rather than a pool that grows: each thread keeps its own memory arena and
/// database cache. They end with the last backend.
#[derive(Clone)]
struct Workers(async_channel::Sender<Job>);

impl Workers {
    fn start() -> Self {
        let (sender, receiver) = async_channel::unbounded::<Job>();
        for _ in 0..WORKERS {
            let jobs = receiver.clone();
            let _ = std::thread::Builder::new()
                .name("helikodu-worker".into())
                .spawn(move || {
                    while let Ok(job) = jobs.recv_blocking() {
                        // A job that panics fails alone; its caller sees the
                        // reply channel close.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    }
                });
        }
        Self(sender)
    }
}

impl fmt::Debug for Workers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Workers")
            .field("queued", &self.0.len())
            .finish()
    }
}

/// Looks at each request before it reaches the player and keeps it back by
/// returning `false` (the full-output confirmation does).
pub type RequestGate = Rc<dyn Fn(&PlayerRequest) -> bool>;

#[derive(Clone, Default)]
struct GateSlot(Rc<RefCell<Option<RequestGate>>>);

impl fmt::Debug for GateSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("GateSlot")
            .field(&self.0.borrow().is_some())
            .finish()
    }
}

impl Backend {
    /// A backend over `library` and `player`, whose updates arrive on `updates`.
    pub fn new(
        library: Arc<dyn LibrarySource>,
        player: Rc<dyn PlayerControl>,
        updates: async_channel::Receiver<Update>,
    ) -> Self {
        Self {
            library,
            player,
            updates: Rc::new(RefCell::new(Some(updates))),
            ui_state: Rc::new(MemoryUiState::default()),
            services: Arc::new(sample::MemoryServices::default()),
            gate: GateSlot::default(),
            workers: Workers::start(),
        }
    }

    /// The same backend keeping the window state with `ui_state`.
    #[must_use]
    pub fn with_ui_state(mut self, ui_state: Rc<dyn UiStateAccess>) -> Self {
        self.ui_state = ui_state;
        self
    }

    /// The window state kept between runs.
    pub fn ui_state(&self) -> Rc<dyn UiStateAccess> {
        self.ui_state.clone()
    }

    /// The library, to be queried off the main thread.
    pub fn library(&self) -> Arc<dyn LibrarySource> {
        self.library.clone()
    }

    /// Sends a request to the player.
    pub fn send(&self, request: PlayerRequest) {
        let gate = self.gate.0.borrow().clone();
        if gate.is_some_and(|gate| !gate(&request)) {
            return;
        }
        self.player.send(request);
    }

    /// Tells the player whether the window shows the position (see
    /// [`PlayerControl::follow_position`]).
    pub fn follow_position(&self, shown: bool) {
        self.player.follow_position(shown);
    }

    /// Sends a request to the player past the gate.
    pub fn send_ungated(&self, request: PlayerRequest) {
        self.player.send(request);
    }

    /// Sets the gate requests pass before they reach the player.
    pub fn set_gate(&self, gate: Option<RequestGate>) {
        self.gate.0.replace(gate);
    }

    /// The same backend with the dialogs' access to the core.
    #[must_use]
    pub fn with_services(mut self, services: Arc<dyn services::Services>) -> Self {
        self.services = services;
        self
    }

    /// The dialogs' access to the core.
    pub fn services(&self) -> Arc<dyn services::Services> {
        self.services.clone()
    }

    /// The player's updates. Only the first caller gets them.
    pub fn take_updates(&self) -> Option<async_channel::Receiver<Update>> {
        self.updates.borrow_mut().take()
    }

    /// Runs `query` on a worker thread and returns its result on the main loop.
    pub async fn query<T, F>(&self, query: F) -> Result<T, BackendError>
    where
        T: Send + 'static,
        F: FnOnce(&dyn LibrarySource) -> Result<T, BackendError> + Send + 'static,
    {
        let library = self.library.clone();
        self.work(move || query(library.as_ref()))
            .await
            .unwrap_or_else(|| Err(BackendError("a library query failed".into())))
    }

    /// Runs `job` on one of the worker threads and returns its result on the
    /// main loop; `None` if it panicked.
    pub(crate) async fn work<T, F>(&self, job: F) -> Option<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (reply, result) = async_channel::bounded(1);
        let job: Job = Box::new(move || {
            let _ = reply.send_blocking(job());
        });
        self.workers.0.send(job).await.ok()?;
        result.recv().await.ok()
    }
}
