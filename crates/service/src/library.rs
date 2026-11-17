// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The music library for tools that use it directly: opening the database, scanning,
//! searching, looking at one file or track, and totals.
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

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use helikodu_library::{DatabaseConfig, DbError, Library, ScanError, SearchRequest};
use helikodu_model::library::TrackId;

pub use self::info::{LibraryEntry, TagOrigin, TrackInfo, read_file_info};
pub use self::location::{DATA_FOLDER, DATABASE_FILE, default_database_path};
pub use helikodu_library::{
    AudioProperties, ExcludeRule, FailureKind, FileFailure, IdentityId, LibraryTotals, MatchRank,
    PlayedTrack, Rating, ScanEvent, ScanHandle, ScanSummary, SearchHit, TrackKind, TrackStats,
    TrackTags,
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

/// An open library database.
#[derive(Debug)]
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

    /// The database file.
    pub fn path(&self) -> &Path {
        &self.path
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
