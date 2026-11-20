// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library interchange: statistics, ratings and playlists from other players and
//! from an export of Helikodu's own user data; statistics carried from a replaced
//! file to its replacement; play counts and ratings written into the files.
//!
//! - **Readers** ([`read_source`]) read another player's library without changing
//!   it: XML files are only read, and SQLite databases are copied to a temporary
//!   folder first (with their write-ahead log), so that a running player keeps its
//!   lock and its data. Rhythmbox (`rhythmdb.xml`, `playlists.xml`), Strawberry and
//!   Clementine (their SQLite database), Lollypop (`lollypop.db`, `playlists.db`)
//!   and an iTunes or Music `Library.xml` are read. [`default_sources`] lists the
//!   files found at the players' usual places, Flatpak folders included.
//! - **Matching** takes each entry to a library track by its path first (a `file://`
//!   URI decoded, with [`PathMapping`]s for a library that moved, such as
//!   `C:\Music` to `/home/me/Music`), then by its tags: the same artist and title,
//!   and the album, the track number and a length within 2 s where both sides
//!   have them. An entry that fits tracks of more than one identity is ambiguous
//!   and left out.
//! - **A plan, then the import.** [`Library::plan_import`](crate::Library::plan_import) says what would change
//!   (see [`ImportPlan`]); [`Library::apply_import`](crate::Library::apply_import) makes the change in one
//!   transaction and records it as an import run, which [`Library::undo_import`](crate::Library::undo_import)
//!   undoes. Merge rules:
//!   - play and skip counts are **added**; the counts each source gave are kept
//!     per identity, so importing the same source again adds only what was played
//!     there since (and nothing when its counts went down). An export of
//!     Helikodu's own data is merged by taking the **higher** count instead, so
//!     that importing it into the library it came from adds nothing;
//!   - a rating is taken only for a track that has none, unless
//!     [`MergeOptions::replace_ratings`] asks for it;
//!   - the earliest first play and the latest last play (and last skip) win; a
//!     track's date added becomes the earlier one;
//!   - playlists are created beside the others, a playlist made by an earlier
//!     import of the same source is not made again; automatic playlists become
//!     smart playlists where their rules have a counterpart in the query language.
//! - **Undo.** Imports are not on the Ctrl+Z history of tag edits and file runs:
//!   that history undoes changes to files, and an import changes only the
//!   database, often for thousands of tracks at once. An import run is undone on
//!   its own, newest first: counts lose what the run added (plays made since
//!   stay), other values go back where they still hold what the run set, and the
//!   playlists and saved searches it made are deleted, as are the labels it gave
//!   and the fields it defined (a field changed since, or used by another
//!   field, stays).

mod apply;
mod carry;
mod clementine;
mod foreign;
mod itunes;
mod labels_fields;
mod lollypop;
mod matching;
mod merge;
mod plan;
mod rhythmbox;
mod sqlite_copy;
mod to_files;
mod uri;
mod user_data;
mod xml_tree;

use std::path::PathBuf;

pub use carry::CarryCandidate;
pub use foreign::{
    ForeignAlbum, ForeignItem, ForeignLabel, ForeignLibrary, ForeignListen, ForeignPlaylist,
    ForeignPlaylistKind, ForeignTrack, ImportSource, Player, default_sources, default_sources_in,
    read_source,
};
pub use labels_fields::FieldConflict;
pub use plan::{
    ImportPlan, MatchCounts, MergeOptions, PlanTotals, PlannedPlaylist, PlannedPlaylistKind,
};
pub(crate) use to_files::record_rating;
pub use to_files::{StatsToFiles, StatsWriteReport};
pub use uri::PathMapping;
pub use user_data::{USER_DATA_FORMAT, USER_DATA_VERSION, UserDataExport};

/// An applied import, or statistics carried to a replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImportRunId(pub(crate) i64);

impl ImportRunId {
    /// The number of the run in the database.
    pub fn get(self) -> i64 {
        self.0
    }
}

/// What an import run did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImportRunKind {
    /// Data of another player.
    Player,
    /// An export of Helikodu's own user data.
    UserData,
    /// Statistics carried from a replaced file to its replacement.
    Carry,
}

/// An import run, as [`Library::import_runs`](crate::Library::import_runs) lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRun {
    /// The run.
    pub id: ImportRunId,
    /// What it imported.
    pub kind: ImportRunKind,
    /// Where from: the player and the path of its file, the export file, or the
    /// two files.
    pub source: String,
    /// When it was applied.
    pub at: std::time::SystemTime,
    /// Whether it was undone.
    pub undone: bool,
    /// The tracks (identities) whose statistics it changed.
    pub tracks: usize,
    /// The playlists, saved searches, history entries, favourites, labels given
    /// and fields defined it added.
    pub rows: usize,
}

/// Why an import, an export or an undo failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ImportError {
    /// A file could not be read or written.
    #[error("{}: {source}", .path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// The error.
        #[source]
        source: std::io::Error,
    },
    /// The file is not what the player writes.
    #[error("{}: {reason}", .path.display())]
    Format {
        /// The file.
        path: PathBuf,
        /// What is wrong.
        reason: String,
    },
    /// The library database failed.
    #[error(transparent)]
    Db(#[from] crate::db::DbError),
    /// Only the newest run that is not undone can be undone.
    #[error("only the newest import can be undone")]
    NotNewest,
    /// There is no such import run, or it was undone already.
    #[error("there is no such import to undo")]
    UnknownRun,
    /// There is no such track in the library.
    #[error("there is no such track in the library")]
    UnknownTrack,
}

fn now() -> i64 {
    crate::db::unix_time::to_unix_seconds(std::time::SystemTime::now())
}

impl crate::Library {
    /// Reads `source` (changing nothing there) and works out what importing it
    /// would do. Nothing in the library changes until
    /// [`apply_import`](Self::apply_import).
    pub fn plan_import(
        &self,
        source: &ImportSource,
        options: &MergeOptions,
    ) -> Result<ImportPlan, ImportError> {
        let library = read_source(source)?;
        self.plan_foreign_import(source.player, &source.key(), &library, options)
    }

    /// Works out what importing `library`, read from `player`'s file named by
    /// `source`, would do.
    pub fn plan_foreign_import(
        &self,
        player: Player,
        source: &str,
        library: &ForeignLibrary,
        options: &MergeOptions,
    ) -> Result<ImportPlan, ImportError> {
        let input = plan::PlanInput {
            kind: ImportRunKind::Player,
            player: Some(player),
            source: source.to_owned(),
            library,
            options,
        };
        Ok(self.database().read(|c| plan::make(c, &input))?)
    }

    /// Reads the user data file at `path` (see [`export_user_data`](Self::export_user_data))
    /// and works out what importing it would do.
    pub fn plan_user_data_import(
        &self,
        path: &std::path::Path,
        options: &MergeOptions,
    ) -> Result<ImportPlan, ImportError> {
        let library = user_data::read(path)?;
        let source = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let input = plan::PlanInput {
            kind: ImportRunKind::UserData,
            player: None,
            source: format!("user-data:{}", source.to_string_lossy()),
            library: &library,
            options,
        };
        Ok(self.database().read(|c| plan::make(c, &input))?)
    }

    /// Carries out `plan` in one transaction, as an import run that
    /// [`undo_import`](Self::undo_import) can undo. The merge rules are applied to
    /// the values in the library at this moment, so plays made since the plan
    /// count too.
    pub fn apply_import(&self, plan: &ImportPlan) -> Result<ImportRunId, ImportError> {
        let plan = plan.clone();
        let now = now();
        let fields = !plan.work.fields.is_empty();
        let run = self
            .database()
            .writer()?
            .write(move |c| apply::apply(c, &plan, now))?;
        if fields {
            self.install_user_fields();
        }
        Ok(ImportRunId(run))
    }

    /// Undoes the import run `run`, which must be the newest one that is not
    /// undone: counts lose what it added (plays made since stay), ratings, times
    /// and dates go back where they still hold what it set, and the playlists,
    /// saved searches, history entries, favourites and labels it added and the
    /// fields it defined are removed.
    pub fn undo_import(&self, run: ImportRunId) -> Result<(), ImportError> {
        let now = now();
        self.database()
            .writer()?
            .write(move |c| apply::undo(c, run.0, now))??;
        self.install_user_fields();
        Ok(())
    }

    /// Every import run, newest first.
    pub fn import_runs(&self) -> Result<Vec<ImportRun>, ImportError> {
        Ok(self.database().read(apply::runs)?)
    }

    /// The user data of the library: ratings, play counts, history, playlists and
    /// smart playlists, Favourites, saved searches, labels and the user's fields.
    pub fn export_user_data(&self) -> Result<UserDataExport, ImportError> {
        let now = now();
        Ok(self.database().read(|c| user_data::export(c, now))?)
    }

    /// Writes the user data of the library to `path` as JSON (see
    /// [`export_user_data`](Self::export_user_data)).
    pub fn export_user_data_to(
        &self,
        path: &std::path::Path,
    ) -> Result<UserDataExport, ImportError> {
        let export = self.export_user_data()?;
        user_data::write(&export, path)?;
        Ok(export)
    }
}
