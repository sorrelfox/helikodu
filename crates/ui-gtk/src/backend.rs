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

pub mod browse;
pub mod columns;
pub mod export;
pub mod fields;
pub mod generators;
pub mod grouping;
pub mod insights;
pub mod interchange;
pub mod labels;
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

pub use browse::{ChapterRow, MediaKind, YearRange, YearRow};
pub use columns::{ExtraColumns, ExtraValues, FieldName, FieldWords, Vocabulary, YourField};
pub use generators::{
    GeneratorInfo, GeneratorRun, GeneratorSort, GeneratorSource, GeneratorStep, GeneratorSteps,
    GeneratorTrack, NotYet, SimilarFound, SimilarRow, SimilarSeed, StepProblem,
};
pub use grouping::{TrackGroup, TrackGrouping};
pub use helikodu_model::player::{AnalyzerFrame, ChannelLevel};
pub use helikodu_model::player::{
    PlayModes, PlayScope, RepeatMode, ShuffleMode, StopAfter, TrackFlags,
};
pub use helikodu_model::player::{
    PlaybackStats, SignalLamp, SignalPath, SignalStage, StageKind, StageState,
};
pub use helikodu_model::settings::UiState;
pub use insights::{
    FacetChoice, FacetField, FacetKey, FacetList, FacetValue, LibraryStats, PlayedAlbumRow,
    PlayedArtistRow, StatisticsSince,
};
pub use labels::{
    LabelChoice, LabelChoices, LabelContents, LabelDone, LabelEdit, LabelEditError, LabelFor,
    LabelKey, LabelRow, LabelState, LabelUndo,
};

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

/// Identifies a saved search for as long as the backend runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct SavedSearchKey(pub u64);

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
    /// The albums with a track by a composer.
    Composer(ArtistKey),
    /// The albums released in a span of years.
    Years(YearRange),
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
    /// Composers.
    pub composers: usize,
    /// Release years.
    pub years: usize,
    /// The user's labels.
    pub labels: usize,
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
    /// How many other byte-identical copies of its file the library has (other
    /// paths to the same file not counted).
    pub copies: u32,
    /// Its genres, joined with `, `.
    pub genre: String,
    /// When it came into the library, if known.
    pub added: Option<SystemTime>,
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
    /// What kind of recording it is.
    pub kind: MediaKind,
    /// The track where listening last stopped, and where in it, for an album
    /// whose tracks remember that (audiobooks and long tracks).
    pub resume: Option<(TrackKey, Duration)>,
    /// Who reads an audiobook: its composer tag, where audiobooks keep it.
    pub reader: Option<String>,
    /// The record label of its release, from the tags of its files.
    pub record_label: Option<String>,
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
    /// Their photo, if they have one: chosen in the app or found in their folder.
    pub photo: Option<CoverKey>,
    /// Whether the photo was chosen in the app (and can be removed).
    pub own_photo: bool,
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
    /// The genres.
    Genre,
    /// When the track came into the library.
    Added,
    /// Its labels, own and those of its album and artists.
    Labels,
    /// One of the user's fields.
    Field(FieldName),
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
    /// What the tracks must match, as in search: words, or a query of the query
    /// language when the text has fields; empty for all.
    pub text: String,
    /// The order, most significant key first; empty for album order.
    pub sort: Vec<TrackSort>,
}

/// What a row of the playlists is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlaylistType {
    /// A folder of playlists.
    Folder,
    /// A playlist of entries put in by hand or imported.
    #[default]
    Static,
    /// A playlist whose tracks rules select.
    Smart,
    /// A playlist made by steps, kept until it is made again.
    Generator,
}

/// A playlist or a playlist folder in the sidebar, in the sidebar's order:
/// each folder is followed by what is in it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlaylistRow {
    /// The playlist.
    pub key: PlaylistKey,
    /// Its name.
    pub name: String,
    /// The number of its entries (of a folder: what is in it).
    pub items: usize,
    /// The length of its entries.
    pub duration: Duration,
    /// When it last changed.
    pub modified: Option<SystemTime>,
    /// What it is.
    pub kind: PlaylistType,
    /// The folder it is in.
    pub parent: Option<PlaylistKey>,
    /// How many folders it is in.
    pub depth: usize,
    /// Whether its entries are locked against changes.
    pub locked: bool,
    /// Whether it shows its entries in groups by album.
    pub grouped: bool,
    /// Whether it follows a playlist file found in a library folder.
    pub from_file: bool,
    /// The entries that name no track in the library.
    pub dead: usize,
    /// Whether an edit can be undone.
    pub can_undo: bool,
    /// Whether an undone edit can be done again.
    pub can_redo: bool,
    /// Whether it is the built-in Favourites playlist, which the heart on the
    /// deck fills: listed first, and never renamed, moved, locked or deleted.
    pub builtin: bool,
}

/// Something pinned to the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PinKey {
    /// An album.
    Album(AlbumKey),
    /// An artist.
    Artist(ArtistKey),
    /// A playlist.
    Playlist(PlaylistKey),
}

/// A saved search, as the side column of the search page lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSearchRow {
    /// The saved search.
    pub key: SavedSearchKey,
    /// Its name.
    pub name: String,
    /// Its query text.
    pub text: String,
    /// How many tracks it finds now.
    pub count: usize,
}

impl PinKey {
    /// The name of its sidebar row.
    pub fn row_name(self) -> String {
        match self {
            Self::Album(k) => format!("pin:album:{}", k.0),
            Self::Artist(k) => format!("pin:artist:{}", k.0),
            Self::Playlist(k) => format!("pin:playlist:{}", k.0),
        }
    }

    /// The pin a sidebar row name stands for.
    pub fn from_row_name(name: &str) -> Option<Self> {
        let rest = name.strip_prefix("pin:")?;
        let (kind, id) = rest.split_once(':')?;
        let id: u64 = id.parse().ok()?;
        Some(match kind {
            "album" => Self::Album(AlbumKey(id)),
            "artist" => Self::Artist(ArtistKey(id)),
            "playlist" => Self::Playlist(PlaylistKey(id)),
            _ => return None,
        })
    }
}

/// A pinned row of the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinRow {
    /// What is pinned.
    pub key: PinKey,
    /// Its name.
    pub name: String,
    /// The cover of an album.
    pub cover: Option<CoverKey>,
    /// The seed of an album's generated cover.
    pub cover_seed: u64,
    /// What a pinned playlist is.
    pub playlist: Option<PlaylistType>,
}

/// What the tidying commands of a playlist would remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlaylistCleanup {
    /// Entries that repeat an earlier one.
    pub duplicates: usize,
    /// Entries that cannot play.
    pub missing: usize,
}

/// A track a smart playlist selects, with when it came into the library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartTrack {
    /// The track.
    pub row: TrackRow,
    /// When it was added.
    pub added: Option<SystemTime>,
}

/// The query tree that smart playlists keep, and its text form.
pub use helikodu_model::query::Query;

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
    /// Its title, read from the file when it is not in the library.
    pub title: Option<String>,
    /// Its length, read from the file when it is not in the library.
    pub duration: Option<Duration>,
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
    /// Makes the picture at `file` the artist's photo (the file itself is left
    /// as it is) and returns their row with it.
    fn set_artist_photo(&self, key: ArtistKey, file: &Path) -> Result<ArtistRow, BackendError> {
        let _ = (key, file);
        Err(BackendError("artist photos are not supported".into()))
    }
    /// Forgets the photo chosen for the artist and returns their row with the
    /// photo shown now: the one in their folder, or none.
    fn remove_artist_photo(&self, key: ArtistKey) -> Result<ArtistRow, BackendError> {
        let _ = key;
        Err(BackendError("artist photos are not supported".into()))
    }
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
    /// The playlists and folders in the sidebar's order: each folder followed
    /// by what is in it.
    fn playlists(&self) -> Result<Vec<PlaylistRow>, BackendError>;
    /// Whether `track` is in Favourites.
    fn is_favourite(&self, track: TrackKey) -> Result<bool, BackendError> {
        let _ = track;
        Ok(false)
    }
    /// Adds `track` to Favourites, or takes it out; returns whether it is a
    /// favourite now.
    fn set_favourite(&self, track: TrackKey, favourite: bool) -> Result<bool, BackendError> {
        let _ = (track, favourite);
        Err(unsupported())
    }
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
    /// Makes an empty folder in `parent`, or at the top.
    fn create_playlist_folder(
        &self,
        name: &str,
        parent: Option<PlaylistKey>,
    ) -> Result<PlaylistKey, BackendError> {
        let _ = (name, parent);
        Err(unsupported())
    }
    /// Moves a playlist or folder into `parent` (the top for `None`) at
    /// `position`, or after what is there.
    fn move_playlist(
        &self,
        key: PlaylistKey,
        parent: Option<PlaylistKey>,
        position: Option<usize>,
    ) -> Result<(), BackendError> {
        let _ = (key, parent, position);
        Err(unsupported())
    }
    /// Copies a playlist into a new playlist named `name`; the copy of a smart
    /// playlist holds the tracks it selects now.
    fn duplicate_playlist(
        &self,
        key: PlaylistKey,
        name: &str,
    ) -> Result<PlaylistKey, BackendError> {
        let _ = (key, name);
        Err(unsupported())
    }
    /// Locks a playlist against changes, or unlocks it.
    fn set_playlist_locked(&self, key: PlaylistKey, locked: bool) -> Result<(), BackendError> {
        let _ = (key, locked);
        Err(unsupported())
    }
    /// Shows a playlist in groups by album, or not.
    fn set_playlist_grouped(&self, key: PlaylistKey, grouped: bool) -> Result<(), BackendError> {
        let _ = (key, grouped);
        Err(unsupported())
    }
    /// Undoes the last edit of a playlist; whether there was one.
    fn undo_playlist(&self, key: PlaylistKey) -> Result<bool, BackendError> {
        let _ = key;
        Ok(false)
    }
    /// Does the last undone edit of a playlist again; whether there was one.
    fn redo_playlist(&self, key: PlaylistKey) -> Result<bool, BackendError> {
        let _ = key;
        Ok(false)
    }
    /// What removing duplicates and missing tracks would remove.
    fn playlist_cleanup(&self, key: PlaylistKey) -> Result<PlaylistCleanup, BackendError> {
        let _ = key;
        Ok(PlaylistCleanup::default())
    }
    /// Removes the entries that repeat an earlier one; how many went.
    fn remove_playlist_duplicates(&self, key: PlaylistKey) -> Result<usize, BackendError> {
        let _ = key;
        Err(unsupported())
    }
    /// Removes the entries that cannot play; how many went.
    fn remove_missing_from_playlist(&self, key: PlaylistKey) -> Result<usize, BackendError> {
        let _ = key;
        Err(unsupported())
    }
    /// Shuffles the order of a playlist for good.
    fn shuffle_playlist(&self, key: PlaylistKey) -> Result<(), BackendError> {
        let _ = key;
        Err(unsupported())
    }
    /// Makes a smart playlist of the tracks `query` selects.
    fn create_smart_playlist(
        &self,
        name: &str,
        query: &Query,
        parent: Option<PlaylistKey>,
    ) -> Result<PlaylistKey, BackendError> {
        let _ = (name, query, parent);
        Err(unsupported())
    }
    /// Replaces the rules of a smart playlist.
    fn set_smart_query(&self, key: PlaylistKey, query: &Query) -> Result<(), BackendError> {
        let _ = (key, query);
        Err(unsupported())
    }
    /// The rules of a smart playlist.
    fn smart_query(&self, key: PlaylistKey) -> Result<Query, BackendError> {
        let _ = key;
        Err(unsupported())
    }
    /// The tracks a smart playlist selects now, in its order.
    fn smart_tracks(&self, key: PlaylistKey) -> Result<Vec<SmartTrack>, BackendError> {
        let _ = key;
        Ok(Vec::new())
    }
    /// How many tracks `query` selects, and how many there are in all.
    fn query_match_count(&self, query: &Query) -> Result<(usize, usize), BackendError> {
        let _ = query;
        Ok((0, 0))
    }
    /// What is pinned to the sidebar, in order.
    fn pins(&self) -> Result<Vec<PinRow>, BackendError> {
        Ok(Vec::new())
    }
    /// Pins an album, an artist or a playlist to the sidebar.
    fn pin(&self, key: PinKey) -> Result<(), BackendError> {
        let _ = key;
        Err(unsupported())
    }
    /// Unpins it.
    fn unpin(&self, key: PinKey) -> Result<(), BackendError> {
        let _ = key;
        Err(unsupported())
    }
    /// The library folders.
    fn folder_roots(&self) -> Result<Vec<FolderRow>, BackendError>;
    /// The folders in `path`, by name.
    fn subfolders(&self, path: &Path) -> Result<Vec<FolderRow>, BackendError>;
    /// The audio files in `path`.
    fn folder(&self, path: &Path) -> Result<FolderListing, BackendError>;
    /// The saved searches in the user's order, each with its count.
    fn saved_searches(&self) -> Result<Vec<SavedSearchRow>, BackendError> {
        Ok(Vec::new())
    }
    /// Saves `query` as `name`, after the other saved searches.
    fn save_search(
        &self,
        name: &str,
        query: &helikodu_model::query::Query,
    ) -> Result<SavedSearchKey, BackendError> {
        let _ = (name, query);
        Err(BackendError(crate::i18n::tr(
            "Searches cannot be saved here.",
        )))
    }
    /// Renames the saved search `key`.
    fn rename_saved_search(&self, key: SavedSearchKey, name: &str) -> Result<(), BackendError> {
        let _ = (key, name);
        Err(BackendError(crate::i18n::tr(
            "Searches cannot be saved here.",
        )))
    }
    /// Deletes the saved search `key`.
    fn delete_saved_search(&self, key: SavedSearchKey) -> Result<(), BackendError> {
        let _ = key;
        Err(BackendError(crate::i18n::tr(
            "Searches cannot be saved here.",
        )))
    }
    /// The listens since `since` (all with `None`), newest first.
    fn history(&self, _since: Option<SystemTime>) -> Result<Vec<HistoryRow>, BackendError> {
        Ok(Vec::new())
    }
    /// The totals of the whole history and the rule for counting a play.
    fn history_totals(&self) -> Result<HistoryTotals, BackendError> {
        Ok(HistoryTotals::default())
    }
    /// Writes the listens since `since` to `path` in the format its extension
    /// names (`.log`: the scrobbler log; otherwise ListenBrainz JSON Lines);
    /// returns how many were written.
    fn export_history(
        &self,
        _path: &Path,
        _since: Option<SystemTime>,
    ) -> Result<usize, BackendError> {
        Ok(0)
    }
    /// The people in the composer role, by sort name.
    fn composers(&self) -> Result<Vec<ArtistRow>, BackendError> {
        Ok(Vec::new())
    }
    /// The release years, newest first, then "Unknown".
    fn years(&self) -> Result<Vec<YearRow>, BackendError> {
        Ok(Vec::new())
    }
    /// The length of all tracks of a composer.
    fn composer_duration(&self, composer: ArtistKey) -> Result<Duration, BackendError> {
        let _ = composer;
        Ok(Duration::ZERO)
    }
    /// Gives every track of `album` the kind `kind`.
    fn set_album_kind(&self, album: AlbumKey, kind: MediaKind) -> Result<(), BackendError> {
        let _ = (album, kind);
        Err(BackendError("media kinds cannot be changed here".into()))
    }
    /// Forgets where listening to `tracks` stopped, to start them again.
    fn forget_resume(&self, tracks: &[TrackKey]) -> Result<(), BackendError> {
        let _ = tracks;
        Ok(())
    }
    /// The chapters of the file of `track` in file order, when it is one of them;
    /// empty otherwise.
    fn chapters(&self, track: TrackKey) -> Result<Vec<ChapterRow>, BackendError> {
        let _ = track;
        Ok(Vec::new())
    }
    /// What the album page shows about `track` beyond its row; `None` when it
    /// is not known.
    fn track_facts(&self, track: TrackKey) -> Result<Option<TrackFacts>, BackendError> {
        let _ = track;
        Ok(None)
    }
    /// The values of `field` among the tracks of `query` (its text filters first)
    /// that pass every choice of `chosen`, each with its number of tracks.
    fn facet_values(
        &self,
        query: &TrackQuery,
        chosen: &[FacetChoice],
        field: FacetField,
    ) -> Result<FacetList, BackendError> {
        let _ = (query, chosen, field);
        Err(unsupported())
    }
    /// `keys` (a track list in its order) in groups by `by`: the groups in the
    /// order of their values, the tracks without a value first, the tracks of
    /// each group in the order of `keys`.
    fn group_tracks(
        &self,
        keys: &[TrackKey],
        by: &TrackGrouping,
    ) -> Result<Vec<TrackGroup>, BackendError> {
        let _ = (keys, by);
        Err(unsupported())
    }
    /// The names of the user's fields a list can be grouped by, in their order.
    fn group_fields(&self) -> Result<Vec<String>, BackendError> {
        Ok(Vec::new())
    }
    /// The tracks of `query` that pass every choice of `chosen`, in its order.
    fn facet_track_keys(
        &self,
        query: &TrackQuery,
        chosen: &[FacetChoice],
    ) -> Result<Vec<TrackKey>, BackendError> {
        let _ = (query, chosen);
        Err(unsupported())
    }
    /// What the Library statistics view shows, counted now.
    fn statistics(&self, since: StatisticsSince) -> Result<LibraryStats, BackendError> {
        let _ = since;
        Err(unsupported())
    }
    /// The fields the user defined, in their order.
    fn your_fields(&self) -> Result<Vec<YourField>, BackendError> {
        Ok(Vec::new())
    }
    /// What `columns` show for each of `keys`; a key without a track is left out.
    fn extra_values(
        &self,
        keys: &[TrackKey],
        columns: &ExtraColumns,
    ) -> Result<Vec<(TrackKey, ExtraValues)>, BackendError> {
        let _ = (keys, columns);
        Ok(Vec::new())
    }
    /// The labels and the user's fields, as the search completion offers them.
    fn vocabulary(&self) -> Result<Vocabulary, BackendError> {
        Ok(Vocabulary::default())
    }
    /// Every label, in name order.
    fn labels(&self) -> Result<Vec<LabelRow>, BackendError> {
        Ok(Vec::new())
    }
    /// Every label with how it stands for `target`.
    fn label_choices(&self, target: &LabelFor) -> Result<LabelChoices, BackendError> {
        let _ = target;
        Ok(LabelChoices::default())
    }
    /// Makes a change of labels.
    fn change_label(&self, edit: &LabelEdit) -> Result<LabelDone, LabelEditError> {
        let _ = edit;
        Err(LabelEditError::Failed(unsupported()))
    }
    /// Puts the labels back as they were before the changes `undo` came from.
    fn undo_labels(&self, undo: &LabelUndo) -> Result<(), BackendError> {
        let _ = undo;
        Err(unsupported())
    }
    /// What the label `key` was given to; `None` when there is no such label.
    fn label_contents(&self, key: LabelKey) -> Result<Option<LabelContents>, BackendError> {
        let _ = key;
        Ok(None)
    }
    /// Every track that has the label `key`, in album order.
    fn label_track_keys(&self, key: LabelKey) -> Result<Vec<TrackKey>, BackendError> {
        let _ = key;
        Ok(Vec::new())
    }
    /// The generator playlist `key`, if there is one.
    fn generator(&self, key: PlaylistKey) -> Result<Option<GeneratorInfo>, BackendError> {
        let _ = key;
        Ok(None)
    }
    /// Makes a generator playlist named `name` in the folder `parent` with the
    /// single step "from the library".
    fn create_generator(
        &self,
        name: &str,
        parent: Option<PlaylistKey>,
    ) -> Result<PlaylistKey, BackendError> {
        let _ = (name, parent);
        Err(unsupported())
    }
    /// The tracks of the generator `key` as opening it shows them: made again
    /// when it is made on open (or `again`), its last result otherwise. A
    /// step that cannot run leaves the last result.
    fn generator_tracks(
        &self,
        key: PlaylistKey,
        again: bool,
    ) -> Result<GeneratorRun, BackendError> {
        let _ = (key, again);
        Err(unsupported())
    }
    /// Runs `steps` for the generator `key` without storing anything.
    fn try_generator(
        &self,
        key: PlaylistKey,
        steps: &GeneratorSteps,
    ) -> Result<GeneratorRun, BackendError> {
        let _ = (key, steps);
        Err(unsupported())
    }
    /// Stores new steps or a new "made again when opened" of the generator
    /// `key`; new steps make it again.
    fn set_generator(
        &self,
        key: PlaylistKey,
        steps: Option<&GeneratorSteps>,
        on_open: Option<bool>,
    ) -> Result<(), BackendError> {
        let _ = (key, steps, on_open);
        Err(unsupported())
    }
    /// The `limit` tracks that sound most like `seed`, or why there is no list.
    fn similar_tracks(
        &self,
        seed: SimilarSeed,
        limit: usize,
    ) -> Result<SimilarFound, BackendError> {
        let _ = (seed, limit);
        Ok(SimilarFound::NotYet(NotYet::NotAnalysed))
    }
}

/// What the album page's details column shows about the selected track beyond
/// what its row holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackFacts {
    /// Its composers, joined with `, `.
    pub composers: String,
    /// When it was last played, as a date and time.
    pub last_played: Option<String>,
    /// When it came into the library, as a date.
    pub added: Option<String>,
    /// The size of its file in bytes.
    pub file_size: Option<u64>,
    /// The track gain of its file's ReplayGain or R128 tags, in dB.
    pub track_gain_db: Option<f64>,
    /// Whether its file carries lyrics.
    pub has_lyrics: bool,
}

/// Where a listen in the play history came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PlayedFrom {
    /// Not known.
    #[default]
    Unknown,
    /// An album.
    Album,
    /// An artist's tracks.
    Artist,
    /// A genre's tracks.
    Genre,
    /// A playlist, by name.
    Playlist(String),
    /// A search, by its text.
    Search(String),
    /// A folder, by name.
    Folder(String),
    /// Queued from elsewhere.
    Queue,
    /// Loose files.
    Files,
}

impl PlayedFrom {
    /// Reads the context the history stores (`album:…`, `playlist:…`,
    /// `search:…`, `folder:…`, `queue`, `files`); `playlist` names a playlist
    /// by its number.
    pub fn parse(context: Option<&str>, playlist: impl Fn(u64) -> Option<String>) -> Self {
        let Some(context) = context else {
            return Self::Unknown;
        };
        let (kind, rest) = context.split_once(':').unwrap_or((context, ""));
        match kind {
            "album" => Self::Album,
            "artist" => Self::Artist,
            "genre" => Self::Genre,
            "playlist" => Self::Playlist(rest.parse().ok().and_then(playlist).unwrap_or_default()),
            "search" => Self::Search(rest.to_owned()),
            "folder" => Self::Folder(
                Path::new(rest)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| rest.to_owned()),
            ),
            "queue" => Self::Queue,
            "files" => Self::Files,
            _ => Self::Unknown,
        }
    }
}

/// One listen in the play history.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRow {
    /// When playback started.
    pub started_at: SystemTime,
    /// The time actually played.
    pub listened: Duration,
    /// Whether it counted as a play; otherwise it was skipped.
    pub counted: bool,
    /// Where it came from.
    pub from: PlayedFrom,
    /// The track; `missing` when it left the library.
    pub track: TrackRow,
}

/// The totals of the whole play history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryTotals {
    /// The listens that counted as plays.
    pub plays: u64,
    /// When the oldest listen started.
    pub first: Option<SystemTime>,
    /// A play counts after this share of the track, in percent …
    pub play_percent: u8,
    /// … or after this many minutes, whichever comes first.
    pub play_minutes: u32,
}

impl Default for HistoryTotals {
    fn default() -> Self {
        Self {
            plays: 0,
            first: None,
            play_percent: 50,
            play_minutes: 4,
        }
    }
}

/// The error of a backend that cannot do something.
fn unsupported() -> BackendError {
    BackendError(crate::i18n::tr("This is not available here."))
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

/// What endless play starts from, by the library's numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RadioSeed {
    /// A track.
    Track(u64),
    /// An album.
    Album(i64),
    /// An artist.
    Artist(i64),
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
    /// Play the tracks of a playlist, replacing the queue; the queue then
    /// comes from that playlist (for the history and MPRIS).
    PlayPlaylist {
        /// The playlist.
        key: PlaylistKey,
        /// Its tracks, in order.
        tracks: Vec<TrackKey>,
        /// The index of the first to play.
        start: usize,
        /// Its name, as the queue panel shows it.
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
    /// Play all tracks of a composer, album by album.
    PlayComposer {
        /// The composer.
        composer: ArtistKey,
        /// In random order.
        shuffle: bool,
    },
    /// Play all tracks of a span of release years.
    PlayYears {
        /// The years.
        years: YearRange,
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
    /// Adds files after the current track (`next`) or at the end of the queue.
    EnqueueFiles {
        /// The files.
        files: Vec<PathBuf>,
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
    /// Play on with the restored session if it was playing when the program
    /// ended; sent once at the start, unless something else is to play.
    ResumeSession,
    /// The desktop session is about to end: save the session now.
    SessionEnding,
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
    /// Moves the lyrics of the playing track by this many milliseconds
    /// (positive shows the lines sooner); kept for the track.
    AdjustLyricsOffset(i64),
    /// Reads the lyrics of the playing track again.
    ReloadLyrics,
    /// Shuffle off, or by tracks, albums, folders, groupings or within albums.
    SetShuffleMode(ShuffleMode),
    /// Repeat off, the track, the album or the whole queue.
    SetRepeatMode(RepeatMode),
    /// Stop after the track, the album or the queue, or not.
    SetStopAfterMode(StopAfter),
    /// "Remove tracks after they play" on or off.
    SetConsume(bool),
    /// "Play only": everything, this artist or this album.
    SetPlayScope(PlayScope),
    /// "Continue from the selected track" on or off.
    SetFollowCursor(bool),
    /// The queue entry selected in the queue panel, for "Continue from the
    /// selected track".
    SetCursor(Option<u64>),
    /// The first track of the next album in the queue.
    NextAlbum,
    /// The first track of the album before, or of this one when it is the first.
    PreviousAlbum,
    /// The first track of an album of the queue chosen at random.
    RandomAlbum,
    /// Intro scan on or off: only the first seconds of each track play.
    SetIntroScan(bool),
    /// "Revert": back to the track "Next" last moved on from, where it was.
    Revert,
    /// Play at another speed (0.5 to 2), keeping the pitch or not, until the
    /// program ends.
    SetSpeed {
        /// The speed, as a factor of the normal one.
        rate: f64,
        /// Whether the pitch stays (tempo) or follows the speed (tape).
        keep_pitch: bool,
    },
    /// "Radio Random": a random track of the queue, from a random point in it.
    RadioRandom,
    /// Starts endless play from a library track, album or artist.
    StartRadio(RadioSeed),
    /// Sets the start of the A-B loop at the playing position.
    LoopStartHere,
    /// Sets the end of the A-B loop at the playing position; the loop then plays.
    LoopEndHere,
    /// Sets both points of the A-B loop (start, end), or clears it with `None`.
    SetLoop(Option<(Duration, Duration)>),
    /// Starts the sleep timer, replacing a running one.
    StartSleepTimer {
        /// How long until the time is up.
        length: Duration,
        /// What happens then.
        when: SleepWhen,
    },
    /// Gives the running sleep timer more time.
    ExtendSleepTimer(Duration),
    /// Turns the sleep timer off.
    CancelSleepTimer,
    /// Changes the playback flags of a library track (the tag editor's Playback
    /// group); they apply at once.
    SetTrackFlags {
        /// The track.
        track: TrackKey,
        /// Its flags.
        flags: TrackFlags,
    },
}

/// When the sleep timer stops playback once its time is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SleepWhen {
    /// At once, after fading the sound out over the last 30 seconds.
    #[default]
    AtOnce,
    /// When the track playing then ends.
    AfterTrack,
}

/// A running sleep timer, as last reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SleepTimer {
    /// The time left when it was reported; zero once the time is up and the
    /// timer waits for the track to end.
    pub remaining: Duration,
    /// The length chosen, with what was added since.
    pub length: Duration,
    /// What happens when the time is up.
    pub when: SleepWhen,
}

/// The A-B loop of the playing track, as far as its points are set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoopPoints {
    /// The start (A).
    pub start: Option<Duration>,
    /// The end (B).
    pub end: Option<Duration>,
}

impl LoopPoints {
    /// The section that loops, once both points are set and A comes first.
    pub fn range(&self) -> Option<(Duration, Duration)> {
        let (start, end) = (self.start?, self.end?);
        (start < end).then_some((start, end))
    }
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

/// Where the lyrics shown came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricsOrigin {
    /// A `.lrc` file beside the track.
    LrcFile,
    /// A `.txt` file beside the track.
    TextFile,
    /// The tags of the track's file.
    Tags,
}

/// One line of lyrics as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricRow {
    /// When it is sung; `None` in lyrics that are not synced.
    pub time: Option<Duration>,
    /// The text.
    pub text: String,
    /// Whether it starts a verse.
    pub verse_start: bool,
}

/// The lyrics of the playing track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricsView {
    /// The lines.
    pub lines: Vec<LyricRow>,
    /// Whether the lines have times.
    pub synced: bool,
    /// Where they came from.
    pub origin: LyricsOrigin,
    /// The lyrics file, when they came from one.
    pub file: Option<PathBuf>,
    /// The offset of the track in milliseconds; positive shows the lines sooner.
    pub offset_ms: i64,
}

/// The playback speed: the speed itself and whether the pitch stays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackSpeed {
    /// The speed, as a factor of the normal one (0.5 to 2).
    pub rate: f64,
    /// Whether the pitch stays (tempo) or follows the speed (tape).
    pub keep_pitch: bool,
}

impl Default for PlaybackSpeed {
    fn default() -> Self {
        Self {
            rate: 1.0,
            keep_pitch: true,
        }
    }
}

impl PlaybackSpeed {
    /// Whether playback runs at the normal speed.
    pub fn is_normal(&self) -> bool {
        (self.rate - 1.0).abs() < 1e-6
    }
}

/// The equalizer of one track, chosen as a whole: it replaces the main
/// equalizer while the track plays.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackEqualizer {
    /// What it is called: a preset's name, "Copy of" one, or the name of an
    /// imported file.
    pub name: String,
    /// The equalizer.
    pub settings: helikodu_model::settings::EqualizerSettings,
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
    /// Whether "Revert" has a track of the queue to go back to.
    pub can_revert: bool,
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
    /// The track analysis made progress, or the background analysis changed
    /// what it does.
    Analysis(services::AnalysisProgress),
    /// The lyrics of the playing track: `None` when it has none or nothing
    /// plays.
    Lyrics(Option<Arc<LyricsView>>),
    /// The line of the lyrics sung now; `None` before the first.
    LyricsLine(Option<usize>),
    /// Another program asked the player for this (MPRIS `Play`, `PlayPause`,
    /// `OpenUri`). It is sent with [`Backend::send`], so it passes the same gate
    /// as the window's own requests.
    Request(PlayerRequest),
    /// Another program asked to show the window (MPRIS `Raise`).
    Raise,
    /// Another program asked to quit (MPRIS `Quit`).
    Quit,
    /// Every play mode: shuffle, repeat, consume, play only and "Continue from
    /// the selected track". Sent after [`Update::Modes`].
    PlayModes(PlayModes),
    /// Stop after the track, the album or the queue, or not. Sent after
    /// [`Update::StopAfter`].
    StopAfterMode(StopAfter),
    /// The A-B loop of the playing track changed; `None` when it ended.
    Loop(Option<LoopPoints>),
    /// The sleep timer started, changed or ended (`None`).
    SleepTimer(Option<SleepTimer>),
    /// The queue entry selected for "Continue from the selected track".
    Cursor(Option<u64>),
    /// A change of how copies are treated (Settings › Library) has been applied
    /// to the library: how many files were read for it, or why it failed.
    /// [`Update::LibraryChanged`] follows.
    DuplicatesApplied(Result<usize, String>),
    /// The playback flags of a library track changed.
    TrackFlags {
        /// The track.
        track: TrackKey,
        /// Its flags now.
        flags: TrackFlags,
    },
    /// The waveform peaks of the playing track (0 to 255), or `None` while it
    /// has none: the seek bar is then a flat band.
    Waveform(Option<Arc<[u8]>>),
    /// A measurement of the analyzer, while one is followed
    /// ([`Backend::follow_analyzer`]) and sound is played.
    Analyzer(Arc<AnalyzerFrame>),
    /// The playback speed changed (the settings', or one set with
    /// [`PlayerRequest::SetSpeed`]).
    Speed(PlaybackSpeed),
}

/// Takes requests on the main thread.
pub trait PlayerControl: fmt::Debug {
    /// Hands `request` to the player.
    fn send(&self, request: PlayerRequest);

    /// Whether the window is shown and wants position updates; while it is
    /// not, they are not sent, and the current position follows when it is
    /// shown again.
    fn follow_position(&self, _shown: bool) {}

    /// The playlists were changed in the window: desktop integrations that
    /// list them read them again.
    fn playlists_changed(&self) {}

    /// Whether a view shows the analyzer, and how many measurements a second
    /// it wants; `None` stops them.
    fn follow_analyzer(&self, _per_second: Option<u32>) {}

    /// Whether the restored session was playing when the program ended and
    /// waits for [`PlayerRequest::ResumeSession`].
    fn resume_pending(&self) -> bool {
        false
    }
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

    /// Tells desktop integrations that the playlists changed (see
    /// [`PlayerControl::playlists_changed`]).
    pub fn playlists_changed(&self) {
        self.player.playlists_changed();
    }

    /// Tells the player whether the window shows the position (see
    /// [`PlayerControl::follow_position`]).
    pub fn follow_position(&self, shown: bool) {
        self.player.follow_position(shown);
    }

    /// Starts or stops the analyzer's measurements (see
    /// [`PlayerControl::follow_analyzer`]).
    pub fn follow_analyzer(&self, per_second: Option<u32>) {
        self.player.follow_analyzer(per_second);
    }

    /// Sends a request to the player past the gate.
    pub fn send_ungated(&self, request: PlayerRequest) {
        self.player.send(request);
    }

    /// Plays on with the restored session if it was playing when the program
    /// ended (see [`PlayerControl::resume_pending`]); does nothing otherwise.
    pub fn resume_session(&self) {
        if self.player.resume_pending() {
            self.send(PlayerRequest::ResumeSession);
        }
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
