// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the Import window, the "Replaced" section of Missing files and the
//! export of the user data need from the core: the other players' libraries
//! found on this computer, the plan of an import with what it would change,
//! applying and undoing it, the earlier imports, the export file, and the
//! missing tracks whose recording is in the library again as another file.
//!
//! [`InterchangeServices`] is implemented over the running service
//! (`service/interchange.rs`) and over invented content
//! (`sample/interchange.rs`). Methods marked "blocking" are called on worker
//! threads only.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use helikodu_service::library::interchange::{
    FieldConflict, ImportRunKind, ImportSummary, MatchCounts, MergeOptions, PathMapping,
    PlanTotals, PlannedPlaylist, PlannedPlaylistKind, Player, SavedBy,
};

use super::{BackendError, TrackKey};

/// Where an import reads from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ImportFrom {
    /// A player's library file.
    Player {
        /// The player.
        player: Player,
        /// Its library file.
        path: PathBuf,
    },
    /// A file made by "Export Your Data…".
    DataFile(PathBuf),
}

impl ImportFrom {
    /// The file read.
    pub fn path(&self) -> &Path {
        match self {
            Self::Player { path, .. } | Self::DataFile(path) => path,
        }
    }

    /// The player, for a player's library.
    pub fn player(&self) -> Option<Player> {
        match self {
            Self::Player { player, .. } => Some(*player),
            Self::DataFile(_) => None,
        }
    }
}

/// A player whose library can be imported, and where it was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSource {
    /// The player.
    pub player: Player,
    /// Its library file at one of the player's usual places (a Flatpak folder
    /// included), or `None` when it is not on this computer.
    pub found: Option<PathBuf>,
}

/// What an import would do, as the Import window shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportPreview {
    /// Where it reads from.
    pub from: ImportFrom,
    /// The names of the files read.
    pub files: Vec<String>,
    /// For a data file: who saved it, when, and how many tracks it lists.
    pub saved: Option<SavedBy>,
    /// How the entries matched the library's tracks.
    pub matching: MatchCounts,
    /// What would change.
    pub totals: PlanTotals,
    /// The playlists of the source and what each becomes.
    pub playlists: Vec<PlannedPlaylist>,
    /// The first entries that fit more than one track, in words.
    pub ambiguous: Vec<String>,
    /// The first entries that match no track, in words.
    pub unmatched: Vec<String>,
    /// The fields defined differently here: the definition here is kept.
    pub field_conflicts: Vec<FieldConflict>,
    /// What the reader left out, in words.
    pub notes: Vec<String>,
    /// When this source was imported before (the newest import that is not
    /// undone), for the playlists it made then.
    pub earlier: Option<SystemTime>,
    /// Whether importing would change nothing.
    pub empty: bool,
    /// The plan's number, for [`InterchangeServices::apply_import`].
    pub plan: u64,
}

/// Why an import cannot be planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportProblem {
    /// The file is not the player's library, or not a data file: the cause.
    Unreadable(String),
    /// A newer version of Helikodu saved the data file.
    Newer,
    /// Something else failed, in words.
    Failed(String),
}

/// An earlier import, or statistics carried to replacement files.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportRunRow {
    /// The run's number.
    pub id: i64,
    /// What it imported.
    pub kind: ImportRunKind,
    /// The player, for an import from another player.
    pub player: Option<Player>,
    /// The file it read, for an import.
    pub path: Option<PathBuf>,
    /// When it was made.
    pub at: SystemTime,
    /// Whether it was undone.
    pub undone: bool,
    /// When it was undone, where that is known.
    pub undone_at: Option<SystemTime>,
    /// The tracks it changed.
    pub tracks: usize,
    /// What it did in figures, where that is known.
    pub summary: Option<ImportSummary>,
}

/// A missing track whose recording is in the library again as another file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplacedTrack {
    /// The missing track.
    pub old: TrackKey,
    /// Its file, which is gone.
    pub old_path: PathBuf,
    /// Its title.
    pub title: Option<String>,
    /// Its artist.
    pub artist: Option<String>,
    /// Its plays.
    pub plays: u64,
    /// Its rating in half stars, 1 to 10.
    pub rating: Option<u8>,
    /// Its history entries.
    pub history: usize,
    /// The playlists it is in.
    pub playlists: usize,
    /// The tracks of the same recording that could take its statistics, with
    /// their files; at least one.
    pub new: Vec<(TrackKey, PathBuf)>,
}

/// The core's side of imports, the export of the user data and the carrying of
/// statistics to replacement files.
pub trait InterchangeServices: fmt::Debug + Send + Sync {
    /// Every player that can be imported, in the order they are offered, with
    /// its library where it was found. Quick: only looks for files.
    fn players(&self) -> Vec<PlayerSource>;
    /// Reads `from` (changing nothing there; a file read before is not read
    /// again) and works out what importing it with `options` would do.
    /// Blocking.
    fn plan_import(
        &self,
        from: &ImportFrom,
        options: &MergeOptions,
    ) -> Result<ImportPreview, ImportProblem>;
    /// Carries out the plan numbered `plan`, the newest made; returns the run.
    /// Blocking.
    fn apply_import(&self, plan: u64) -> Result<i64, BackendError>;
    /// Every import run, newest first. Blocking.
    fn import_runs(&self) -> Result<Vec<ImportRunRow>, BackendError>;
    /// Undoes `run`, which must be the newest that is not undone. Blocking.
    fn undo_import(&self, run: i64) -> Result<(), BackendError>;
    /// Writes the user data to `path`. Blocking.
    fn export_user_data(&self, path: &Path) -> Result<(), BackendError>;
    /// The missing tracks whose recording is in the library again as another
    /// file. Blocking.
    fn replaced_tracks(&self) -> Result<Vec<ReplacedTrack>, BackendError>;
    /// Moves the statistics, history and playlist places of each missing track
    /// to its new track, as one run that is undone like an import; returns the
    /// run. Blocking.
    fn carry_stats(&self, pairs: &[(TrackKey, TrackKey)]) -> Result<i64, BackendError>;
    /// Drops the source read last and its plan, once the Import window closes.
    fn forget(&self) {}
}

/// A core without a library: nothing to import.
#[derive(Debug, Default)]
pub struct NoInterchange;

fn no_library() -> BackendError {
    BackendError(crate::i18n::tr("Helikodu runs without a library."))
}

impl InterchangeServices for NoInterchange {
    fn players(&self) -> Vec<PlayerSource> {
        Player::ALL
            .into_iter()
            .map(|player| PlayerSource {
                player,
                found: None,
            })
            .collect()
    }

    fn plan_import(
        &self,
        _from: &ImportFrom,
        _options: &MergeOptions,
    ) -> Result<ImportPreview, ImportProblem> {
        Err(ImportProblem::Failed(no_library().0))
    }

    fn apply_import(&self, _plan: u64) -> Result<i64, BackendError> {
        Err(no_library())
    }

    fn import_runs(&self) -> Result<Vec<ImportRunRow>, BackendError> {
        Ok(Vec::new())
    }

    fn undo_import(&self, _run: i64) -> Result<(), BackendError> {
        Err(no_library())
    }

    fn export_user_data(&self, _path: &Path) -> Result<(), BackendError> {
        Err(no_library())
    }

    fn replaced_tracks(&self) -> Result<Vec<ReplacedTrack>, BackendError> {
        Ok(Vec::new())
    }

    fn carry_stats(&self, _pairs: &[(TrackKey, TrackKey)]) -> Result<i64, BackendError> {
        Err(no_library())
    }
}

/// The source of a run as the library records it (`rhythmbox:/path`,
/// `user-data:/path`) split into the player and the file.
pub fn run_source(kind: ImportRunKind, source: &str) -> (Option<Player>, Option<PathBuf>) {
    match kind {
        ImportRunKind::Carry => (None, None),
        ImportRunKind::UserData => (None, source.strip_prefix("user-data:").map(PathBuf::from)),
        ImportRunKind::Player => match source.split_once(':') {
            Some((id, path)) => (Player::from_id(id), Some(PathBuf::from(path))),
            None => (None, None),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_sources_are_split() {
        assert_eq!(
            run_source(ImportRunKind::Player, "rhythmbox:/h/rhythmdb.xml"),
            (
                Some(Player::Rhythmbox),
                Some(PathBuf::from("/h/rhythmdb.xml"))
            )
        );
        assert_eq!(
            run_source(ImportRunKind::UserData, "user-data:/h/data.json"),
            (None, Some(PathBuf::from("/h/data.json")))
        );
        assert_eq!(run_source(ImportRunKind::Carry, "/a → /b"), (None, None));
    }
}
