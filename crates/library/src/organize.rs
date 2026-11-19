// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The file organizer: moving and renaming library files by a naming pattern,
//! with a preview first and a journal that can undo a run and do it again.
//!
//! - **Preview** ([`Library::plan_organize`]): every file of the chosen tracks
//!   (or of the whole library) gets the path its tags give through the pattern,
//!   in its own library folder or in a chosen one, and what would happen to it:
//!   moved, renamed, already in place, or left alone because the name is taken
//!   (on the disk, by another library file, or by an earlier file of the same
//!   preview). Nothing changes.
//! - **A run** ([`Library::organize`]) moves the files the owner ticked, one by
//!   one. Every move is written to the journal before the file moves, and the
//!   library row moves in the same transaction that marks it done, so a run cut
//!   short by a crash is finished from what is on the disk the next time
//!   ([`Library::recover_file_runs`]). A file that changed place since the
//!   preview, a destination that was taken in the meantime, and a file the disk
//!   refuses stay where they are, and the run's report says why. Files are never
//!   replaced and never deleted; across file systems a move is a checked copy
//!   (see `fsops`). Covers and booklets beside the files can move along when a
//!   whole folder moves to one place. Folders a run leaves empty are removed.
//! - **Undo and redo** ([`Library::undo_file_run`], [`Library::redo_file_run`])
//!   move the files of the last run back, newest first, and forward again, with
//!   the same checks. The tracks keep their identities, statistics, ratings and
//!   places in playlists throughout, as they do for a move the watcher sees.
//!
//! The journal is only for file operations; it does not record tag edits.

mod fsops;
mod patterns;
mod plan;
mod run;

use std::path::PathBuf;
use std::sync::Mutex;

pub use helikodu_meta::naming::{
    CharacterSet, FieldMap, FieldValues, MAX_NAME_BYTES, NamingPattern, PatternError,
    PatternProblem, clean_file_name,
};
use helikodu_model::library::TrackId;

pub use patterns::SavedPattern;
pub(crate) use plan::naming_values_of;

use crate::Library;
use crate::copies::FileId;
use crate::db::DbError;
use crate::scan::CancelToken;

/// One file operation at a time, in the whole program.
static FILE_OPERATIONS: Mutex<()> = Mutex::new(());

/// Where organized files go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Each file stays in its own library folder.
    OwnLibraryFolder,
    /// Every file goes below this folder, which must be in a library folder.
    Folder(PathBuf),
}

/// What a preview asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizeOptions {
    /// The pattern.
    pub pattern: String,
    /// Where the files go.
    pub into: Destination,
    /// Which characters names may hold.
    pub characters: CharacterSet,
    /// The word written for a field that is empty outside square brackets.
    pub unknown: String,
}

/// What would happen to a file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Change {
    /// It moves to another folder under its own name.
    Move,
    /// It stays in its folder under a new name.
    Rename,
    /// It moves to another folder under a new name.
    MoveAndRename,
    /// It is where the pattern puts it.
    InPlace,
    /// Another file has that name (on the disk, in the library, or earlier in the
    /// same preview).
    NameTaken,
    /// The file is missing, so it cannot move.
    Missing,
    /// A CUE sheet refers to the file by its name, so it stays.
    CueSheet,
}

impl Change {
    /// Whether the file can be moved (and is ticked at first).
    pub fn moves(&self) -> bool {
        matches!(self, Self::Move | Self::Rename | Self::MoveAndRename)
    }
}

/// One file of a preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedMove {
    /// The file.
    pub file: FileId,
    /// A track of it, to play it from the preview.
    pub track: TrackId,
    /// Where it is.
    pub from: PathBuf,
    /// Where the pattern puts it.
    pub to: PathBuf,
    /// What would happen.
    pub change: Change,
}

/// A file a run is asked to move: a ticked row of the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveRequest {
    /// The file.
    pub file: FileId,
    /// Where it is, as the preview saw it.
    pub from: PathBuf,
    /// Where it goes.
    pub to: PathBuf,
}

/// A run in the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileRunId(pub(crate) i64);

impl FileRunId {
    /// The number of the run in the database.
    pub fn get(self) -> i64 {
        self.0
    }

    /// The id with the number `id`; it names a run only if the library gave it out.
    pub fn from_raw(id: i64) -> Self {
        Self(id)
    }
}

/// What kind of run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// The organizer.
    Organize,
    /// One file renamed by hand.
    Rename,
}

/// Why a file did not move (or move back).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MoveProblem {
    /// It is not where it was.
    Gone,
    /// The library has it elsewhere now, or no longer has it.
    Changed,
    /// Something is at the destination already.
    NameTaken,
    /// The destination is not in a library folder.
    OutsideLibrary,
    /// The run was stopped before this file.
    Stopped,
    /// The disk or the library refused; the text says why.
    Failed(String),
}

impl std::fmt::Display for MoveProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gone => write!(f, "the file is not where it was"),
            Self::Changed => write!(f, "the library has the file elsewhere now"),
            Self::NameTaken => write!(f, "a file with that name is already there"),
            Self::OutsideLibrary => write!(f, "the destination is not in a library folder"),
            Self::Stopped => write!(f, "stopped before this file"),
            Self::Failed(reason) => write!(f, "{reason}"),
        }
    }
}

/// What happened to one file of a run, an undo or a redo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveOutcome {
    /// The library file; `None` for a cover or booklet that moved with one.
    pub file: Option<FileId>,
    /// Where it was before this step.
    pub from: PathBuf,
    /// Where this step put it, or would have.
    pub to: PathBuf,
    /// Why it stayed, or `None` when it moved.
    pub problem: Option<MoveProblem>,
}

/// The report of a run, an undo or a redo.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunReport {
    /// The run in the journal; `None` when nothing moved (nothing to undo).
    pub run: Option<FileRunId>,
    /// Each file, in the order of the step.
    pub outcomes: Vec<MoveOutcome>,
}

impl RunReport {
    /// How many files moved (covers and booklets included).
    pub fn moved(&self) -> usize {
        self.outcomes.iter().filter(|o| o.problem.is_none()).count()
    }

    /// How many library files moved.
    pub fn moved_files(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.problem.is_none() && o.file.is_some())
            .count()
    }

    /// The outcomes with a problem.
    pub fn problems(&self) -> impl Iterator<Item = &MoveOutcome> {
        self.outcomes.iter().filter(|o| o.problem.is_some())
    }
}

/// The newest run of the journal and what can be done with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRunSummary {
    /// The run.
    pub run: FileRunId,
    /// What kind of run.
    pub kind: RunKind,
    /// When it started, in seconds since the Unix epoch.
    pub started_at: i64,
    /// The pattern it used, for the organizer.
    pub pattern: Option<String>,
    /// The files that are where the run put them: undo moves them back.
    pub moved: usize,
    /// The files that are back where they were: redo moves them again.
    pub undone: usize,
    /// The files that could not be moved.
    pub failed: usize,
}

impl FileRunSummary {
    /// Whether the run can be undone.
    pub fn can_undo(&self) -> bool {
        self.moved > 0
    }

    /// Whether the run can be done again.
    pub fn can_redo(&self) -> bool {
        self.undone > 0
    }
}

/// Why a preview or a run could not start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OrganizeError {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The pattern cannot be read.
    #[error("the pattern cannot be read: {0}")]
    Pattern(#[from] PatternError),
    /// The chosen folder is not in a library folder.
    #[error("{} is not in a library folder", .0.display())]
    OutsideLibrary(PathBuf),
    /// The run asked for is not the newest, or has nothing to undo or redo.
    #[error("there is nothing to undo or redo in that run")]
    NothingToDo,
}

/// What a run, an undo or a redo reports as it goes: files done and files in all.
pub type Progress<'a> = &'a (dyn Fn(usize, usize) + Sync);

impl Library {
    /// Previews moving the files of `tracks` (every library file when `None`) by
    /// `options`. Changes nothing.
    pub fn plan_organize(
        &self,
        tracks: Option<&[TrackId]>,
        options: &OrganizeOptions,
    ) -> Result<Vec<PlannedMove>, OrganizeError> {
        let pattern = NamingPattern::parse(&options.pattern)?;
        plan::plan(self.database(), tracks, &pattern, options)
    }

    /// Moves `moves` (the ticked rows of a preview made with `pattern`), recording
    /// each in the journal first; with `companions`, the covers and booklets of a
    /// folder whose files all move to one folder move along. Stops between files
    /// when `cancel` is set.
    pub fn organize(
        &self,
        moves: &[MoveRequest],
        pattern: Option<&str>,
        companions: bool,
        progress: Progress<'_>,
        cancel: &CancelToken,
    ) -> Result<RunReport, OrganizeError> {
        let _guard = FILE_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        run::recover(self.database())?;
        let kind = if pattern.is_some() {
            RunKind::Organize
        } else {
            RunKind::Rename
        };
        run::organize(
            self.database(),
            moves,
            kind,
            pattern,
            companions,
            fsops::Method::Auto,
            progress,
            cancel,
        )
    }

    /// Renames the file `file` at `from` to the name `name` in the same folder,
    /// as one run of the journal.
    pub fn rename_file(
        &self,
        file: FileId,
        from: PathBuf,
        name: &std::ffi::OsStr,
    ) -> Result<RunReport, OrganizeError> {
        let to = from.with_file_name(name);
        self.organize(
            &[MoveRequest { file, from, to }],
            None,
            false,
            &|_, _| {},
            &CancelToken::new(),
        )
    }

    /// The values of the fields and tags of each of `tracks` for a naming
    /// pattern, as the organizer names files by them; tracks the library does not
    /// have are left out. The converter and folder sync name their files by
    /// these, so that a pattern gives one name in every tool.
    pub fn naming_values(&self, tracks: &[TrackId]) -> Result<Vec<(TrackId, FieldMap)>, DbError> {
        let tracks = tracks.to_vec();
        self.database()
            .read(move |c| plan::naming_values(c, &tracks))
    }

    /// The newest run of the journal, after finishing any run a crash cut short.
    pub fn last_file_run(&self) -> Result<Option<FileRunSummary>, OrganizeError> {
        let _guard = FILE_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        run::recover(self.database())?;
        Ok(run::last_run(self.database())?)
    }

    /// Moves the files of `run`, the newest run, back where they were, newest
    /// first. A file that cannot go back stays, and its outcome says why; undoing
    /// again tries it again.
    pub fn undo_file_run(
        &self,
        run: FileRunId,
        progress: Progress<'_>,
    ) -> Result<RunReport, OrganizeError> {
        let _guard = FILE_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        run::recover(self.database())?;
        run::step(self.database(), run, false, fsops::Method::Auto, progress)
    }

    /// Moves the files of `run`, the newest run, that an undo moved back, to
    /// where the run had put them.
    pub fn redo_file_run(
        &self,
        run: FileRunId,
        progress: Progress<'_>,
    ) -> Result<RunReport, OrganizeError> {
        let _guard = FILE_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        run::recover(self.database())?;
        run::step(self.database(), run, true, fsops::Method::Auto, progress)
    }

    /// Finishes the runs a crash cut short, from what is on the disk: a file
    /// found at its destination counts as moved (and its library row follows), a
    /// file still where it was counts as not moved. Returns how many moves were
    /// settled.
    pub fn recover_file_runs(&self) -> Result<usize, OrganizeError> {
        let _guard = FILE_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(run::recover(self.database())?)
    }

    /// Removes the runs of the journal that started before `before` (seconds
    /// since the Unix epoch). Returns how many went.
    pub fn prune_file_runs(&self, before: i64) -> Result<usize, DbError> {
        self.database().writer()?.write(move |c| {
            c.prepare_cached("DELETE FROM file_runs WHERE started_at < ?1 AND state <> 0")?
                .execute([before])
        })
    }

    /// The library file at `path`, if there is one.
    pub fn file_of_path(&self, path: &std::path::Path) -> Result<Option<FileId>, DbError> {
        let path = path.to_path_buf();
        Ok(self
            .database()
            .read(move |c| crate::catalog::write::file_at(c, &path))?
            .map(FileId))
    }

    /// The paths of the files of `tracks` (every library file when `None`) that
    /// are present, in library folders that are not masked, in path order.
    pub fn present_files(&self, tracks: Option<&[TrackId]>) -> Result<Vec<PathBuf>, DbError> {
        let tracks = tracks.map(<[TrackId]>::to_vec);
        self.database()
            .read(move |c| plan::present_files(c, tracks.as_deref()))
    }

    /// The organizer's saved patterns, in the order of its list.
    pub fn naming_patterns(&self) -> Result<Vec<SavedPattern>, DbError> {
        self.database().read(patterns::list)
    }

    /// Saves a pattern: a new one when `id` is `None` (at the end of the list),
    /// otherwise the one with that id. Returns its id.
    pub fn save_naming_pattern(
        &self,
        id: Option<i64>,
        name: &str,
        pattern: &str,
    ) -> Result<i64, DbError> {
        let (name, pattern) = (name.to_owned(), pattern.to_owned());
        self.database()
            .writer()?
            .write(move |c| patterns::save(c, id, &name, &pattern))
    }

    /// Removes the saved pattern `id`.
    pub fn delete_naming_pattern(&self, id: i64) -> Result<(), DbError> {
        self.database()
            .writer()?
            .write(move |c| patterns::delete(c, id))
    }

    #[doc(hidden)]
    /// [`organize`](Self::organize) that copies across one file system, for tests
    /// of moves between file systems.
    pub fn organize_by_copying(
        &self,
        moves: &[MoveRequest],
        pattern: Option<&str>,
    ) -> Result<RunReport, OrganizeError> {
        let _guard = FILE_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        run::organize(
            self.database(),
            moves,
            RunKind::Organize,
            pattern,
            false,
            fsops::Method::Copy,
            &|_, _| {},
            &CancelToken::new(),
        )
    }
}
