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

use std::cell::RefCell;
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

pub use helikodu_model::settings::UiState;

/// Identifies an album for as long as the backend runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AlbumKey(pub u64);

/// Identifies a track for as long as the backend runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TrackKey(pub u64);

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

/// Which albums, in which order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct AlbumQuery {
    /// The order.
    pub sort: AlbumSort,
    /// The quick filter.
    pub filter: AlbumFilter,
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

/// One track of an album or the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Another program asked to show the window (MPRIS `Raise`).
    Raise,
    /// Another program asked to quit (MPRIS `Quit`).
    Quit,
}

/// Takes requests on the main thread.
pub trait PlayerControl: fmt::Debug {
    /// Hands `request` to the player.
    fn send(&self, request: PlayerRequest);
}

/// The front end's handle on the core.
#[derive(Debug, Clone)]
pub struct Backend {
    library: Arc<dyn LibrarySource>,
    player: Rc<dyn PlayerControl>,
    updates: Rc<RefCell<Option<async_channel::Receiver<Update>>>>,
    ui_state: Rc<dyn UiStateAccess>,
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
        self.player.send(request);
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
        gtk::gio::spawn_blocking(move || query(library.as_ref()))
            .await
            .unwrap_or_else(|_| Err(BackendError("a library query failed".into())))
    }
}
