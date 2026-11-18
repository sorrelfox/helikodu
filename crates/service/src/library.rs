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

mod info;
mod location;
mod service;
mod source;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use std::time::Duration;

use helikodu_library::{DatabaseConfig, DbError, Library, ScanError, SearchRequest};
use helikodu_model::library::TrackId;

pub use self::info::{LibraryEntry, TagOrigin, TrackInfo, read_file_info};
pub use self::location::{DATA_FOLDER, DATABASE_FILE, default_database_path};
pub use self::service::{
    LibraryEvent, LibraryService, LibraryServiceOptions, QueueItemInfo, WatchSettings,
};
pub use self::source::TrackSource;
pub use helikodu_library::{
    AlbumDetail, AlbumFilter, AlbumId, AlbumQuery, AlbumSort, AlbumSummary, ArtistQuery, ArtistRef,
    ArtistRole, ArtistSummary, AudioProperties, CoverHash, Disc, ExcludeRule, FailureKind,
    FileFailure, GenreId, GenreQuery, GenreRef, GenreSummary, HistoryEntry, HistoryQuery,
    IdentityId, LibraryTotals, Listen, ListenEnd, ListenOutcome, MAX_PAGE, MatchRank, Page, Paged,
    PersonId, PlayRules, PlayedTrack, PollReason, Rating, RescanReason, RootState, ScanEvent,
    ScanHandle, ScanSummary, SearchHit, Thumbnail, ThumbnailSize, TrackFilter, TrackKind,
    TrackQuery, TrackSection, TrackStats, TrackSummary, TrackTags, WatchEvent, WatchMode,
};

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
    /// A scan was asked for without folders, and the library has none.
    #[error("the library has no folders yet; name the folders to scan")]
    NoFolders,
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
        if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(folder).map_err(|source| LibraryError::Io {
                context: format!("cannot create {}", folder.display()),
                source,
            })?;
        }
        let library = Library::open(&config(path))?;
        Ok(Self {
            library: Arc::new(library),
            path: path.to_path_buf(),
        })
    }

    /// Opens the library database at `path` if it exists. A database written by a
    /// newer version is opened read-only.
    pub fn open_existing(path: &Path) -> Result<Self, LibraryError> {
        if !path.is_file() {
            return Err(LibraryError::NoLibrary(path.to_path_buf()));
        }
        let config = config(path);
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
        let mut tracks = Vec::new();
        loop {
            let page = self.library.tracks(&TrackQuery {
                filter,
                page: Page::new(tracks.len(), MAX_PAGE),
                include_masked: false,
            })?;
            let done = page.items.is_empty() || tracks.len() + page.items.len() >= page.total;
            tracks.extend(page.items);
            if done {
                return Ok(tracks);
            }
        }
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
}

fn config(path: &Path) -> DatabaseConfig {
    DatabaseConfig::file(path).with_collation(location::collation())
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
