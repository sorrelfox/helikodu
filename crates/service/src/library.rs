// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The music library for the front ends.
//!
//! - [`MusicLibrary`] is the database itself, with typed queries: browsing (albums,
//!   album detail, artists, genres, tracks), search, one file or track, totals,
//!   statistics and ratings. Its calls block on the database, so tools use it
//!   directly and the window uses it only through:
//! - [`LibraryService`], which runs the same queries on its own thread and hands
//!   each result to a callback, makes cover thumbnails through the artwork pool, and
//!   runs scans and the folder watcher, announcing their progress as
//!   [`LibraryEvent`]s on the event bus.
//!
//! ```no_run
//! use helikodu_service::library::{MusicLibrary, default_database_path};
//!
//! let library = MusicLibrary::open(&default_database_path()?)?;
//! let page = library.search("love", 0, 20)?;
//! for found in &page.hits {
//!     println!("{} {:?}", found.hit.track, found.hit.title);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod analysis;
mod artist_photos;
pub mod backups;
mod duplicates;
mod files;
mod grouping;
mod history_export;
mod info;
mod insights;
mod kinds;
mod labels;
mod location;
mod managed;
mod playlists;
mod query;
mod service;
mod sorting;
mod source;
mod stats_import;
mod stats_writes;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use std::time::Duration;

use helikodu_library::tag_editing::{BatchItem, BatchProgress};
use helikodu_library::{
    DatabaseConfig, DbError, Library, ScanError, ScanOptions, SearchRequest, SymlinkPolicy,
    TagWriteSettings,
};
use helikodu_model::library::TrackId;
use helikodu_model::player::TrackFlags;
use helikodu_model::settings::{AdvancedSettings, LibrarySettings};
use std::collections::HashMap;

/// Held while tags are written to files: by a tag edit or its undo in the
/// front end, and by the background writer of play counts and ratings. A file
/// is replaced as a whole when its tags are written, so two writers at once
/// could lose one's change.
static TAG_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Takes the lock that keeps tag writes to files one after the other.
pub fn lock_tag_writes() -> std::sync::MutexGuard<'static, ()> {
    TAG_WRITES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub use self::analysis::{AnalysisProgress, AnalysisState, BackgroundAnalysisOptions, PauseReason};
pub use self::artist_photos::ARTIST_PHOTO_FOLDER;
pub use self::history_export::{ExportedListen, HistoryFormat, write_history};
pub use self::info::{LibraryEntry, TagOrigin, TrackInfo, read_file_info};
pub use self::location::{DATA_FOLDER, DATABASE_FILE, default_database_path};
pub use self::managed::{managed_options, set_managed_folder, turn_off_managed_folder};
pub use self::playlists::{CleanupCounts, FoundReport, Pin, PinTarget, PlaylistNode, PlaylistType};
pub use self::query::{
    ParsedQuery, QueryError, SavedSearchEntry, SyntaxError, SyntaxErrorKind, parse_query,
    parse_query_lenient, print_query,
};
pub use self::service::{
    LibraryEvent, LibraryService, LibraryServiceOptions, QueueItemInfo, WatchSettings,
};
pub use self::source::TrackSource;
pub use self::stats_import::{STATS_BATCH, stats_to_files};
pub use self::stats_writes::StatsWriteOptions;
pub use helikodu_library::PlayKeys;
/// Imports from other players and of exported user data, statistics carried to a
/// replacement, and statistics written into the files.
pub use helikodu_library::interchange;
/// What a tag editor needs: the files as they are opened for editing, the values a
/// selection shares, and the changes written by [`MusicLibrary::write_tags`] and
/// [`LibraryService::write_tags`].
pub use helikodu_library::tag_editing;
/// The tag tools: values from file names, numbering, capital letters, find and
/// replace, garbled text and sort names, worked out without touching a file.
pub use helikodu_library::tag_tools;
pub use helikodu_library::{
    AlbumDetail, AlbumFilter, AlbumId, AlbumQuery, AlbumSort, AlbumSummary, ArtistQuery, ArtistRef,
    ArtistRole, ArtistSummary, AudioProperties, CoverHash, Disc, ExcludeRule, FailureKind,
    FileFailure, GenreId, GenreQuery, GenreRef, GenreSummary, HistoryEntry, HistoryQuery,
    HistoryTotals, IdentityId, LibraryTotals, Listen, ListenEnd, ListenOutcome, MAX_PAGE,
    MatchRank, Page, Paged, PersonId, PlayRules, PlayedTrack, PollReason, Rating, RescanReason,
    RootState, ScanEvent, ScanHandle, ScanSummary, SearchHit, Thumbnail, ThumbnailSize,
    TrackFilter, TrackKind, TrackOrder, TrackQuery, TrackSection, TrackSortKey, TrackStats,
    TrackSummary, TrackTags, WatchEvent, WatchMode,
};
pub use helikodu_library::{
    AlbumFormat, AlbumQuality, DeadItem, DeadReference, ExportOptions, ExportReport, ImportOptions,
    ImportReport, ItemContent, PlaylistError, PlaylistId, PlaylistItem, PlaylistItemId,
    PlaylistKind, PlaylistSummary, TagWriteReport, TrackDetails,
};
pub use helikodu_library::{
    AlbumMeasurement, CancelToken, LoudnessCoverage, LoudnessSelection, LoudnessTarget,
    MeasuredLoudness, StoredLoudness,
};
pub use helikodu_library::{AnalysisCoverage, AnalysisOutcome, StoredAnalysis};
/// Artist photos chosen in the app or found in the artists' folders.
pub use helikodu_library::{ArtistPhoto, ArtistPhotoError};
pub use helikodu_library::{
    Change, CharacterSet, Destination, FileRunId, FileRunSummary, FoundFile, MissingFile,
    MoveOutcome, MoveProblem, MoveRequest, NamingPattern, OrganizeError, OrganizeOptions,
    PatternError, PatternProblem, PlannedMove, Progress, RelinkOutcome, RelinkProblem, RunKind,
    RunReport, SavedPattern,
};
pub use helikodu_library::{
    CopyFile, CopyGroup, DuplicateCounts, DuplicateEntry, DuplicateGroup, FileId, KeepRule,
    MainCopyError, MatchBy, Trash, TrashError, TrashOutcome, TrashRefusal, Trashed, backup_path,
};
/// The fields the user defines: computed fields and custom fields.
pub use helikodu_library::{
    DefinitionError, DefinitionPart, DefinitionProblem, FieldDefinition, FieldError, UserField,
};
pub use helikodu_library::{FacetChoice, FacetField, FacetKey, FacetList, FacetValue};
pub use helikodu_library::{FileSection, YearSpan, YearSummary};
pub use helikodu_library::{GroupBy, TrackGroup};
/// One undo history over tag edits and file runs, newest first: what Ctrl+Z and
/// Ctrl+Shift+Z step through outside an open playlist.
pub use helikodu_library::{HistoryAction, HistoryError, HistoryReport, UndoHistory};
/// The managed media folder: plans, runs and their journal.
pub use helikodu_library::{
    ItemAction, ItemKind, ItemResult, ManagedError, ManagedItem, ManagedOperation, ManagedOptions,
    ManagedOutcome, ManagedPlan, ManagedReport, ManagedRunId, ManagedRunSummary, SkipReason,
    Transfer,
};
/// Labels of tracks, albums and artists, kept in the library.
pub use helikodu_library::{
    ItemLabels, LabelChange, LabelError, LabelId, LabelItems, LabelMark, LabelMarks, LabelSummary,
    LabelTarget, LabelUndo, MAX_LABEL_CHARS,
};
pub use helikodu_library::{LibraryStatistics, PlayedAlbum, PlayedArtist, StatisticsRequest};
/// The undo journal of tag edits: [`MusicLibrary::edit_tags`] records each
/// save, and [`MusicLibrary::step_tags`] undoes them or does them again.
pub use helikodu_library::{
    RatingOutcome, StepProblem, TagEditReport, TagHistory, TagStep, TagStepId, TagStepReport,
};
pub use helikodu_library::{SavedSearch, SavedSearchId};

/// Why a library operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LibraryError {
    /// The database failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// A scan could not run.
    #[error(transparent)]
    Scan(#[from] ScanError),
    /// There is no library database at the path yet.
    #[error("there is no library at {} yet; scan a folder first", .0.display())]
    NoLibrary(PathBuf),
    /// Neither `XDG_DATA_HOME` nor `HOME` names a folder for the database.
    #[error("cannot find the data folder: neither XDG_DATA_HOME nor HOME is set")]
    NoDataFolder,
    /// The library service has stopped.
    #[error("the library service has stopped")]
    Gone,
    /// A rating outside 0 to 5 stars.
    #[error("{0} is not a rating from 0 to 5 stars")]
    InvalidRating(u8),
    /// A playlist could not be read, changed, imported or exported.
    #[error(transparent)]
    Playlist(#[from] PlaylistError),
    /// A query could not run, or a saved search could not be changed.
    #[error(transparent)]
    Query(#[from] QueryError),
    /// The main copy of a group of copies could not be changed.
    #[error(transparent)]
    MainCopy(#[from] MainCopyError),
    /// Files could not be organized, or a run could not be undone or redone.
    #[error(transparent)]
    Organize(#[from] OrganizeError),
    /// Files could not be imported into the managed folder or consolidated, or a
    /// run of it could not go on or be rolled back.
    #[error(transparent)]
    Managed(#[from] helikodu_library::ManagedError),
    /// The managed folder is off: no folder is set.
    #[error("the managed folder is off; set a folder first")]
    ManagedFolderOff,
    /// The managed folder setting cannot be used; the text says why.
    #[error("{0}")]
    ManagedFolder(String),
    /// A backup could not be made, checked or staged.
    #[error(transparent)]
    Backup(#[from] helikodu_library::backups::BackupError),
    /// A scan was asked for without folders, and the library has none.
    #[error("the library has no folders yet; name the folders to scan")]
    NoFolders,
    /// An import, an export of the user data or an undo of an import failed.
    #[error(transparent)]
    Import(#[from] helikodu_library::interchange::ImportError),
    /// A label could not be given, taken, renamed or merged.
    #[error(transparent)]
    Label(#[from] LabelError),
    /// A field could not be defined or removed.
    #[error(transparent)]
    Field(#[from] FieldError),
    /// An artist photo could not be set or removed.
    #[error(transparent)]
    ArtistPhoto(#[from] helikodu_library::ArtistPhotoError),
    /// A file's tags could not be read.
    #[error("cannot read {}: {reason}", .path.display())]
    ReadFile {
        /// The file.
        path: PathBuf,
        /// Why.
        reason: String,
    },
    /// The folder of the database could not be created, or a scan thread could not
    /// be started.
    #[error("{context}: {source}")]
    Io {
        /// What was being done.
        context: String,
        /// The error.
        #[source]
        source: io::Error,
    },
}

/// What to scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanRequest {
    /// The folders, added to the library if they are not in it yet. When empty, every
    /// folder already in the library is scanned again.
    pub folders: Vec<PathBuf>,
    /// What to leave out. A pattern with a `/` is a path (a folder and everything in
    /// it, or one file), relative to the current folder unless it is absolute; any
    /// other pattern is matched against every name, with `*` for any run of
    /// characters and `?` for any one.
    pub exclude: Vec<String>,
    /// The number of worker processes; 0 picks one per processor, at most 8.
    pub workers: usize,
    /// The worker program, when it is not `helikodu-scan-worker` beside the running
    /// program.
    pub worker_program: Option<PathBuf>,
}

/// One page of search results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchPage {
    /// The matching tracks on this page, best first.
    pub hits: Vec<FoundTrack>,
    /// The number of matching tracks on all pages.
    pub total: usize,
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundTrack {
    /// The track, its rank and its title, artists, album and length.
    pub hit: SearchHit,
    /// Its file, as the bytes the kernel returned.
    pub path: Option<PathBuf>,
}

/// The library as a whole: its folders, totals and most played tracks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryOverview {
    /// The library folders, in path order.
    pub roots: Vec<PathBuf>,
    /// The totals.
    pub totals: LibraryTotals,
    /// The tracks played most often, most played first.
    pub most_played: Vec<PlayedTrack>,
}

/// An open library database. Clones share it.
#[derive(Debug, Clone)]
pub struct MusicLibrary {
    library: Arc<Library>,
    path: PathBuf,
}

impl MusicLibrary {
    /// Opens the library database at `path`, creating it and its folder if needed.
    /// The sort order follows the locale of the environment (`LC_ALL`, `LC_COLLATE`,
    /// `LANG`).
    pub fn open(path: &Path) -> Result<Self, LibraryError> {
        Self::open_sorted_for(path, "")
    }

    /// Opens the library database at `path` as [`open`](Self::open) does, sorted
    /// for the interface language `language` (the `[appearance] language`
    /// setting; empty for the environment's locale).
    pub fn open_sorted_for(path: &Path, language: &str) -> Result<Self, LibraryError> {
        if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(folder).map_err(|source| LibraryError::Io {
                context: format!("cannot create {}", folder.display()),
                source,
            })?;
        }
        let library = Library::open(&config(path, language))?;
        Ok(Self {
            library: Arc::new(library),
            path: path.to_path_buf(),
        })
    }

    /// Opens the library database at `path` if it exists. A database written by a
    /// newer version is opened read-only.
    pub fn open_existing(path: &Path) -> Result<Self, LibraryError> {
        Self::open_existing_sorted_for(path, "")
    }

    /// Opens the library database at `path` if it exists, as
    /// [`open_existing`](Self::open_existing) does, sorted for the interface
    /// language `language` (empty for the environment's locale).
    pub fn open_existing_sorted_for(path: &Path, language: &str) -> Result<Self, LibraryError> {
        if !path.is_file() {
            return Err(LibraryError::NoLibrary(path.to_path_buf()));
        }
        let config = config(path, language);
        let library = match Library::open(&config) {
            Err(DbError::NewerSchema { .. }) => Library::open_read_only(&config)?,
            other => other?,
        };
        Ok(Self {
            library: Arc::new(library),
            path: path.to_path_buf(),
        })
    }

    /// A new, empty library in memory, gone when the last clone is dropped. For
    /// tests and trying things out.
    pub fn in_memory() -> Result<Self, LibraryError> {
        Ok(Self {
            library: Arc::new(Library::open(&DatabaseConfig::in_memory())?),
            path: PathBuf::from(":memory:"),
        })
    }

    /// A library already opened through the library crate, for tests that fill it
    /// directly. `path` is only reported.
    pub fn from_library(library: Arc<Library>, path: impl Into<PathBuf>) -> Self {
        Self {
            library,
            path: path.into(),
        }
    }

    /// The database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn inner(&self) -> &Arc<Library> {
        &self.library
    }

    /// Starts a scan on its own thread. Its events tell the progress; joining the
    /// handle gives the summary.
    pub fn scan(&self, request: ScanRequest) -> Result<ScanHandle, LibraryError> {
        let folders = if request.folders.is_empty() {
            let roots = self.library.roots()?;
            if roots.is_empty() {
                return Err(LibraryError::NoFolders);
            }
            roots
        } else {
            request.folders
        };
        let mut options = helikodu_library::ScanOptions::new(folders);
        options.exclude = request.exclude.iter().map(|p| exclude_rule(p)).collect();
        options.workers.count = request.workers;
        options.workers.program = Some(helikodu_library::find_worker_program(
            request.worker_program.as_deref(),
        )?);
        ScanHandle::spawn(Arc::clone(&self.library), options).map_err(|source| LibraryError::Io {
            context: "cannot start the scan".into(),
            source,
        })
    }

    /// The tracks matching `text`, best first: `limit` of them after the first
    /// `offset`. At most 1000 are returned at once.
    pub fn search(
        &self,
        text: &str,
        offset: usize,
        limit: usize,
    ) -> Result<SearchPage, LibraryError> {
        let results = self
            .library
            .search(&SearchRequest::new(text).page(offset, limit))?;
        let hits = results
            .hits
            .into_iter()
            .map(|hit| {
                let path = self
                    .library
                    .track_details(hit.track)?
                    .map(|details| details.path);
                Ok(FoundTrack { hit, path })
            })
            .collect::<Result<_, DbError>>()?;
        Ok(SearchPage {
            hits,
            total: results.total,
        })
    }

    /// What the library holds about `track`, with the loudness tags of its file if
    /// it can be read; `None` if there is no such track.
    pub fn track_info(&self, track: TrackId) -> Result<Option<TrackInfo>, LibraryError> {
        info::track_info(&self.library, track)
    }

    /// The tags of the file at `path`, read from the file, with what the library
    /// holds about it if it is in the library. A library file that cannot be read
    /// any more is described from the library.
    pub fn file_info(&self, path: &Path) -> Result<TrackInfo, LibraryError> {
        info::file_info(Some(self.library.as_ref()), path)
    }

    /// One page of albums.
    pub fn albums(&self, query: &AlbumQuery) -> Result<Paged<AlbumSummary>, LibraryError> {
        Ok(self.library.albums(query)?)
    }

    /// Every track matching `text`, best first, as ids, in one query.
    pub fn search_ids(&self, text: &str) -> Result<Vec<TrackId>, LibraryError> {
        Ok(self.library.search_ids(text)?)
    }

    /// The album and the number of other copies of each of `tracks`, in one query
    /// for many tracks; for lists that fetch their rows by id.
    pub fn albums_and_copies(
        &self,
        tracks: &[TrackId],
    ) -> Result<std::collections::HashMap<TrackId, (Option<AlbumId>, u32)>, LibraryError> {
        Ok(self.library.albums_and_copies(tracks)?)
    }

    /// Every album of `query`, whatever its page, in one query.
    pub fn all_albums(&self, query: &AlbumQuery) -> Result<Vec<AlbumSummary>, LibraryError> {
        Ok(self.library.all_albums(query)?)
    }

    /// An album with its discs and tracks; `None` if there is no such album or all
    /// its tracks are on drives that are not connected.
    pub fn album(&self, album: AlbumId) -> Result<Option<AlbumDetail>, LibraryError> {
        Ok(self.library.album(album)?)
    }

    /// One page of artists in one role.
    pub fn artists(&self, query: &ArtistQuery) -> Result<Paged<ArtistSummary>, LibraryError> {
        Ok(self.library.artists(query)?)
    }

    /// One page of genres.
    pub fn genres(&self, query: &GenreQuery) -> Result<Paged<GenreSummary>, LibraryError> {
        Ok(self.library.genres(query)?)
    }

    /// One page of tracks.
    pub fn tracks(&self, query: &TrackQuery) -> Result<Paged<TrackSummary>, LibraryError> {
        Ok(self.library.tracks(query)?)
    }

    /// Every track `filter` selects, in its order (page after page).
    pub fn all_tracks(&self, filter: TrackFilter) -> Result<Vec<TrackSummary>, LibraryError> {
        self.sorted_tracks(filter, &[])
    }

    /// Every track `filter` selects, sorted by `sort` (most significant key first)
    /// and then in the filter's own order.
    pub fn sorted_tracks(
        &self,
        filter: TrackFilter,
        sort: &[TrackOrder],
    ) -> Result<Vec<TrackSummary>, LibraryError> {
        let mut tracks = Vec::new();
        loop {
            let page = self.library.tracks(&TrackQuery {
                filter,
                page: Page::new(tracks.len(), MAX_PAGE),
                include_masked: false,
                sort: sort.to_vec(),
            })?;
            let done = page.items.is_empty() || tracks.len() + page.items.len() >= page.total;
            tracks.extend(page.items);
            if done {
                return Ok(tracks);
            }
        }
    }

    /// The ids and albums of every track `filter` selects, sorted by `sort` and
    /// then in the filter's own order, in one query; for lists that fetch their
    /// rows later.
    pub fn sorted_track_ids(
        &self,
        filter: TrackFilter,
        sort: &[TrackOrder],
    ) -> Result<Vec<(TrackId, Option<AlbumId>)>, LibraryError> {
        Ok(self.library.track_order(&TrackQuery {
            filter,
            page: Page::new(0, MAX_PAGE),
            include_masked: false,
            sort: sort.to_vec(),
        })?)
    }

    /// The source to play for `track`: its file, and the part of it for a CUE track
    /// or chapter. `None` if there is no such track.
    pub fn track_source(&self, track: TrackId) -> Result<Option<TrackSource>, LibraryError> {
        Ok(source::track_source(&self.library, track)?)
    }

    /// The library track of the whole file at `path`, if it is one.
    pub fn track_at(&self, path: &Path) -> Result<Option<TrackId>, LibraryError> {
        Ok(self.library.track_at(path, 0)?)
    }

    /// The playback flags of `track` (the tag editor's Playback group).
    pub fn track_flags(&self, track: TrackId) -> Result<TrackFlags, LibraryError> {
        Ok(self.library.track_flags(track)?)
    }

    /// Stores the playback flags of `track`. While a player runs, change them with
    /// its `SetTrackFlags` command instead, so that its queue follows at once.
    pub fn set_track_flags(&self, track: TrackId, flags: TrackFlags) -> Result<(), LibraryError> {
        Ok(self.library.set_track_flags(track, flags)?)
    }

    /// The album, folder, grouping, artist and flags of `tracks`, for the play modes.
    pub fn play_keys(
        &self,
        tracks: &[TrackId],
    ) -> Result<HashMap<TrackId, PlayKeys>, LibraryError> {
        Ok(self.library.play_keys(tracks)?)
    }

    /// The statistics of `track`.
    pub fn stats(&self, track: TrackId) -> Result<TrackStats, LibraryError> {
        Ok(self.library.stats(track)?)
    }

    /// Where listening to `track` last stopped, if it keeps that (long tracks and
    /// audiobooks).
    pub fn resume_position(&self, track: TrackId) -> Result<Option<Duration>, LibraryError> {
        Ok(self.library.stats(track)?.resume_position)
    }

    /// Records a listen to `track` under `rules` and returns what it counted as.
    pub fn record_listen(
        &self,
        track: TrackId,
        listen: Listen,
        rules: PlayRules,
    ) -> Result<ListenOutcome, LibraryError> {
        Ok(self.library.record_listen(track, listen, rules)?)
    }

    /// Rates `track` in whole stars (0 to 5), or clears its rating with `None`.
    /// The database keeps half stars; the interface sets whole ones.
    pub fn rate(&self, track: TrackId, stars: Option<u8>) -> Result<(), LibraryError> {
        let rating = match stars {
            None => None,
            Some(stars) => {
                Some(Rating::from_stars(stars).ok_or(LibraryError::InvalidRating(stars))?)
            }
        };
        Ok(self.library.set_rating(track, rating)?)
    }

    /// The listening history, newest first.
    pub fn history(&self, query: &HistoryQuery) -> Result<Vec<HistoryEntry>, LibraryError> {
        Ok(self.library.history(query)?)
    }

    /// The totals of the whole listening history.
    pub fn history_totals(&self) -> Result<HistoryTotals, LibraryError> {
        Ok(self.library.history_totals()?)
    }

    /// What the library holds about `track` without reading its file: its path, tags
    /// and audio properties; `None` if there is no such track.
    pub fn track_details(&self, track: TrackId) -> Result<Option<TrackDetails>, LibraryError> {
        Ok(self.library.track_details(track)?)
    }

    /// Every static playlist, in the order they were created.
    pub fn playlists(&self) -> Result<Vec<PlaylistSummary>, LibraryError> {
        let mut all = Vec::new();
        loop {
            let page = self.library.playlists(Page::new(all.len(), MAX_PAGE))?;
            let done = page.items.is_empty() || all.len() + page.items.len() >= page.total;
            all.extend(page.items);
            if done {
                return Ok(all);
            }
        }
    }

    /// The static playlist `playlist`, if there is one.
    pub fn playlist(&self, playlist: PlaylistId) -> Result<Option<PlaylistSummary>, LibraryError> {
        Ok(self.library.playlist(playlist)?)
    }

    /// Every item of the static playlist `playlist`, in order.
    pub fn playlist_items(&self, playlist: PlaylistId) -> Result<Vec<PlaylistItem>, LibraryError> {
        let mut all = Vec::new();
        loop {
            let page = self
                .library
                .playlist_items(playlist, Page::new(all.len(), MAX_PAGE))?;
            let done = page.items.is_empty() || all.len() + page.items.len() >= page.total;
            all.extend(page.items);
            if done {
                return Ok(all);
            }
        }
    }

    /// Creates an empty static playlist named `name`.
    pub fn create_playlist(&self, name: &str) -> Result<PlaylistId, LibraryError> {
        Ok(self.library.create_playlist(name)?)
    }

    /// Renames `playlist`.
    pub fn rename_playlist(&self, playlist: PlaylistId, name: &str) -> Result<(), LibraryError> {
        Ok(self.library.rename_playlist(playlist, name)?)
    }

    /// Deletes `playlist` with its items.
    pub fn delete_playlist(&self, playlist: PlaylistId) -> Result<(), LibraryError> {
        Ok(self.library.delete_playlist(playlist)?)
    }

    /// Inserts `tracks` into `playlist` at `position`, or adds them at its end.
    pub fn add_to_playlist(
        &self,
        playlist: PlaylistId,
        tracks: Vec<TrackId>,
        position: Option<usize>,
    ) -> Result<(), LibraryError> {
        match position {
            Some(position) => self
                .library
                .insert_into_playlist(playlist, position, tracks)?,
            None => {
                self.library.add_to_playlist(playlist, tracks)?;
            }
        }
        Ok(())
    }

    /// Moves the items at `positions` of `playlist` before the item at `to`, keeping
    /// their order; returns the new position of the first.
    pub fn move_playlist_items(
        &self,
        playlist: PlaylistId,
        positions: Vec<usize>,
        to: usize,
    ) -> Result<usize, LibraryError> {
        Ok(self.library.move_playlist_items(playlist, positions, to)?)
    }

    /// Removes the items at `positions` from `playlist`; returns how many went.
    pub fn remove_from_playlist(
        &self,
        playlist: PlaylistId,
        positions: Vec<usize>,
    ) -> Result<usize, LibraryError> {
        Ok(self.library.remove_from_playlist(playlist, positions)?)
    }

    /// Imports the playlist file at `path` (M3U, M3U8, PLS or XSPF) as a new playlist.
    pub fn import_playlist(&self, path: &Path) -> Result<ImportReport, LibraryError> {
        Ok(self
            .library
            .import_playlist(path, &ImportOptions::default())?)
    }

    /// Exports `playlist` to `path` with relative paths, replacing the file
    /// atomically. The extension of `path` picks the format: `.m3u`, `.pls` and
    /// `.xspf` write M3U, PLS and XSPF; any other writes M3U8.
    pub fn export_playlist(
        &self,
        playlist: PlaylistId,
        path: &Path,
    ) -> Result<ExportReport, LibraryError> {
        let kind = PlaylistKind::from_path(path).unwrap_or(PlaylistKind::M3u8);
        self.export_playlist_as(playlist, path, kind)
    }

    /// Exports `playlist` to `path` as `kind` with relative paths, replacing the
    /// file atomically.
    pub fn export_playlist_as(
        &self,
        playlist: PlaylistId,
        path: &Path,
        kind: PlaylistKind,
    ) -> Result<ExportReport, LibraryError> {
        let options = ExportOptions {
            kind,
            ..ExportOptions::default()
        };
        Ok(self.library.export_playlist(playlist, path, options)?)
    }

    /// Writes each item's changes into its file, one file after the other, and then
    /// reads the files written again so that the library has their new tags at once.
    ///
    /// `advanced` gives the ID3v2 version of new tags and the size above which files
    /// are changed in place; `library` the rules the re-read follows, as a scan does;
    /// `worker_program` the scan worker when it is not beside the running program.
    /// A file that fails does not stop the others: each file's result is in the
    /// report. `progress` is called after each file. Blocks until all are done.
    pub fn write_tags(
        &self,
        items: &[BatchItem],
        library: &LibrarySettings,
        advanced: &AdvancedSettings,
        worker_program: Option<&Path>,
        progress: &mut dyn FnMut(&BatchProgress),
    ) -> Result<TagWriteReport, LibraryError> {
        let scan = scan_options(library, worker_program)?;
        Ok(self.library.write_tags(
            items,
            &TagWriteSettings::from_settings(advanced),
            &scan,
            progress,
            &CancelToken::new(),
        )?)
    }

    /// Writes tags as [`write_tags`](Self::write_tags) does and records the
    /// change in the undo journal of tag edits as one step.
    pub fn edit_tags(
        &self,
        items: &[BatchItem],
        library: &LibrarySettings,
        advanced: &AdvancedSettings,
        worker_program: Option<&Path>,
        progress: &mut dyn FnMut(&BatchProgress),
    ) -> Result<TagEditReport, LibraryError> {
        let scan = scan_options(library, worker_program)?;
        Ok(self.library.edit_tags(
            items,
            &TagWriteSettings::from_settings(advanced),
            &scan,
            progress,
            &CancelToken::new(),
        )?)
    }

    /// What the undo journal of tag edits can undo and do again.
    pub fn tag_history(&self) -> Result<TagHistory, LibraryError> {
        Ok(self.library.tag_history()?)
    }

    /// Undoes the newest tag edit (`redo` false), or does the newest undone one
    /// again; `None` when there is none. Files changed since are left alone and
    /// reported.
    pub fn step_tags(
        &self,
        redo: bool,
        library: &LibrarySettings,
        advanced: &AdvancedSettings,
        worker_program: Option<&Path>,
    ) -> Result<Option<TagStepReport>, LibraryError> {
        let scan = scan_options(library, worker_program)?;
        let settings = TagWriteSettings::from_settings(advanced);
        Ok(if redo {
            self.library.redo_tags(&settings, &scan)?
        } else {
            self.library.undo_tags(&settings, &scan)?
        })
    }

    /// Undoes the newest tag edit or file run, whichever came last (`redo`
    /// false), or does the one undone last again; `None` when there is none.
    pub fn step_history(
        &self,
        redo: bool,
        library: &LibrarySettings,
        advanced: &AdvancedSettings,
        worker_program: Option<&Path>,
        progress: Progress<'_>,
    ) -> Result<Option<HistoryReport>, LibraryError> {
        let scan = scan_options(library, worker_program)?;
        let settings = TagWriteSettings::from_settings(advanced);
        let report = if redo {
            self.library.redo_last(&settings, &scan, progress)
        } else {
            self.library.undo_last(&settings, &scan, progress)
        };
        report.map_err(|e| match e {
            HistoryError::Tags(e) => LibraryError::Scan(e),
            HistoryError::Files(e) => LibraryError::Organize(e),
            HistoryError::Db(e) => LibraryError::Database(e),
        })
    }

    /// Rates `track` like [`rate`](Self::rate), and with the `[advanced]`
    /// setting to write ratings to files, also into the file of a whole-file
    /// track, which is then read again.
    pub fn rate_into_file(
        &self,
        track: TrackId,
        stars: Option<u8>,
        library: &LibrarySettings,
        advanced: &AdvancedSettings,
        worker_program: Option<&Path>,
    ) -> Result<RatingOutcome, LibraryError> {
        let settings = TagWriteSettings::from_settings(advanced);
        if !settings.write_ratings_to_tags {
            self.rate(track, stars)?;
            return Ok(RatingOutcome {
                rating: stars.and_then(Rating::from_stars),
                file: None,
                refresh: None,
            });
        }
        let scan = scan_options(library, worker_program)?;
        self.library
            .rate_track(track, stars, &settings, &scan)
            .map_err(|e| match e {
                helikodu_library::RateError::InvalidStars(s) => LibraryError::InvalidRating(s),
                helikodu_library::RateError::Db(e) => LibraryError::Database(e),
            })
    }

    /// The library folders and whether each is masked or polled.
    pub fn root_states(&self) -> Result<Vec<RootState>, LibraryError> {
        Ok(self.library.root_states()?)
    }

    /// The library folders, the totals and the `top` most played tracks.
    pub fn overview(&self, top: usize) -> Result<LibraryOverview, LibraryError> {
        Ok(LibraryOverview {
            roots: self.library.roots()?,
            totals: self.library.totals()?,
            most_played: self.library.most_played(top)?,
        })
    }

    /// The loudness measurement of `track`, current or not; `None` if it was never
    /// measured.
    pub fn measured_loudness(
        &self,
        track: TrackId,
    ) -> Result<Option<StoredLoudness>, LibraryError> {
        Ok(self.library.measured_loudness(track)?)
    }

    /// The track analysis of `track`, current or not; `None` if it was never
    /// analysed.
    pub fn track_analysis(&self, track: TrackId) -> Result<Option<StoredAnalysis>, LibraryError> {
        Ok(self.library.track_analysis(track)?)
    }

    /// How many tracks have a current track analysis.
    pub fn analysis_coverage(&self) -> Result<AnalysisCoverage, LibraryError> {
        crate::jobs::analysis::coverage(self)
    }

    /// How many tracks have loudness values from their tags or measured.
    pub fn loudness_coverage(&self) -> Result<LoudnessCoverage, LibraryError> {
        Ok(self.library.loudness_coverage()?)
    }

    /// The tracks a loudness measurement of `selection` would measure.
    pub fn loudness_targets(
        &self,
        selection: &LoudnessSelection,
    ) -> Result<Vec<LoudnessTarget>, LibraryError> {
        Ok(self.library.loudness_targets(selection)?)
    }
}

/// How to open the database at `path`, sorted for the interface language
/// `language` (empty for the environment's locale).
fn config(path: &Path, language: &str) -> DatabaseConfig {
    DatabaseConfig::file(path).with_collation(location::collation(language))
}

/// How a scan of the folders of `settings` reads them, with the worker program.
pub(crate) fn scan_options(
    settings: &LibrarySettings,
    worker_program: Option<&Path>,
) -> Result<ScanOptions, LibraryError> {
    let mut scan = ScanOptions::new(settings.folders.clone());
    scan.exclude = settings
        .skip
        .iter()
        .map(|p| {
            if p.contains('/') {
                ExcludeRule::Path(PathBuf::from(p))
            } else {
                ExcludeRule::Name(p.clone())
            }
        })
        .collect();
    scan.include_video = settings.include_video;
    scan.symlinks = if settings.follow_symlinks {
        SymlinkPolicy::Follow
    } else {
        SymlinkPolicy::WithinRoots
    };
    scan.workers.program = Some(helikodu_library::find_worker_program(worker_program)?);
    Ok(scan)
}

/// The rule for an `--exclude` pattern (see [`ScanRequest::exclude`]).
fn exclude_rule(pattern: &str) -> ExcludeRule {
    if pattern.contains('/') {
        let path = Path::new(pattern);
        ExcludeRule::Path(std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
    } else {
        ExcludeRule::Name(pattern.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclude_patterns_are_names_or_paths() {
        assert_eq!(exclude_rule("*.part"), ExcludeRule::Name("*.part".into()));
        assert_eq!(
            exclude_rule("/music/Podcasts"),
            ExcludeRule::Path("/music/Podcasts".into())
        );
        let ExcludeRule::Path(relative) = exclude_rule("Podcasts/") else {
            panic!("a pattern with a slash is a path");
        };
        assert!(relative.is_absolute());
    }

    #[test]
    fn missing_library_is_reported() {
        let error = MusicLibrary::open_existing(Path::new("/nonexistent/library.db"));
        assert!(matches!(error, Err(LibraryError::NoLibrary(_))));
    }
}
