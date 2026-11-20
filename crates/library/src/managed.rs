// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The managed media folder: a folder in the library that files are imported
//! into, named by a naming pattern, and that the library's other files can be
//! consolidated into.
//!
//! - **Import** ([`Library::plan_managed_import`]): files and folders from outside
//!   the library (a downloads folder, a memory stick) are read by the scan workers
//!   for their tags, and each audio file gets the path the pattern gives it below
//!   the managed folder. Their sidecar files go with them: covers, booklets and rip
//!   logs of a folder to the folder that gets most of its audio files, lyrics
//!   (`.lrc`) beside their audio file under its new name, and CUE sheets beside the
//!   files they describe, with their FILE lines changed to the new names. Of a file
//!   given alone, only its lyrics and its CUE sheet go with it.
//! - **Consolidate** ([`Library::plan_consolidate`]): the library files outside the
//!   managed folder get the paths the pattern gives them inside it. A file keeps its
//!   library row (and so its identity: ratings, play counts, labels, history and
//!   places in playlists), which follows it to its new path. When the files are
//!   copied, the library follows the copies and the originals stay on the disk.
//! - **The plan** lists every file with its source and destination, what will
//!   happen to it, the space the files need and the space that is free. Nothing
//!   changes. A destination where an identical file already is means the file is
//!   skipped; one taken by another file gets a number, ` (2)`, ` (3)`, … A move
//!   whose source cannot be removed (a read-only folder or file system) becomes a
//!   copy.
//! - **A run** ([`Library::apply_managed`]) writes the whole plan into the journal
//!   first, then places one file after the other (see `transfer`): a copy goes to a
//!   temporary name, is flushed, renamed into place without replacing anything and
//!   compared with its source; only then is a moved file's source removed. The
//!   library learns the new files by reading only the folders they went to.
//! - **Cut short** (a crash, `kill -9`, or [`CancelToken`]): the run stays in the
//!   journal as interrupted ([`Library::interrupted_managed_run`]) and can be
//!   continued ([`Library::resume_managed_run`]) or rolled back
//!   ([`Library::roll_back_managed_run`]), both from what is on the disk. A new run
//!   cannot start while one is interrupted.

mod cue;
mod naming;
mod plan;
mod run;
mod transfer;

use std::path::{Path, PathBuf};

use crate::Library;
use crate::copies::FileId;
use crate::db::DbError;
use crate::organize::{CharacterSet, FILE_OPERATIONS, PatternError, Progress};
use crate::scan::{CancelToken, ScanError, ScanOptions};

/// Whether files are copied or moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transfer {
    /// The files are copied; the originals stay.
    Copy,
    /// The files are moved: copied (or renamed on one file system), checked, and
    /// only then removed from where they were.
    Move,
}

/// The managed folder and how files are named in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedOptions {
    /// The managed folder.
    pub folder: PathBuf,
    /// The library folder that holds it (or the folder itself), which the library
    /// gets if it does not have it yet.
    pub library_folder: PathBuf,
    /// The naming pattern below the folder.
    pub pattern: String,
    /// Which characters names may hold.
    pub characters: CharacterSet,
    /// The word written for a field that is empty outside square brackets.
    pub unknown: String,
    /// Whether the files are copied or moved.
    pub transfer: Transfer,
}

/// What is planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ManagedOperation {
    /// Files from outside the library come into the managed folder.
    Import,
    /// Library files outside the managed folder go into it.
    Consolidate,
}

/// What a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    /// An audio file.
    Audio,
    /// A picture beside the audio files.
    Cover,
    /// A CUE sheet.
    CueSheet,
    /// Lyrics (`.lrc`).
    Lyrics,
    /// A rip log.
    Log,
    /// A booklet (PDF).
    Booklet,
}

impl ItemKind {
    pub(crate) fn number(self) -> i64 {
        match self {
            Self::Audio => 0,
            Self::Cover => 1,
            Self::CueSheet => 2,
            Self::Lyrics => 3,
            Self::Log => 4,
            Self::Booklet => 5,
        }
    }

    pub(crate) fn from_number(n: i64) -> Self {
        match n {
            1 => Self::Cover,
            2 => Self::CueSheet,
            3 => Self::Lyrics,
            4 => Self::Log,
            5 => Self::Booklet,
            _ => Self::Audio,
        }
    }
}

/// Why a file is left where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
    /// An identical file is at the destination already.
    Identical,
    /// The file could not be read; the text says why.
    Unreadable(String),
    /// The file is in a library folder already: consolidate it instead.
    InLibrary,
    /// The file is in the managed folder already.
    InManagedFolder,
    /// The library file is missing.
    Missing,
    /// The file is not where it was.
    Gone,
    /// The library has the file elsewhere now, or no longer has it.
    Changed,
    /// The disk refused; the text says why.
    Failed(String),
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identical => write!(f, "an identical file is already there"),
            Self::Unreadable(reason) => write!(f, "the file cannot be read: {reason}"),
            Self::InLibrary => write!(f, "the file is in the library already"),
            Self::InManagedFolder => write!(f, "the file is in the managed folder already"),
            Self::Missing => write!(f, "the file is missing"),
            Self::Gone => write!(f, "the file is not where it was"),
            Self::Changed => write!(f, "the library has the file elsewhere now"),
            Self::Failed(reason) => write!(f, "{reason}"),
        }
    }
}

/// What will happen to a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemAction {
    /// It is copied.
    Copy,
    /// It is moved.
    Move,
    /// It should move, but its source cannot be removed: it is copied and stays.
    CopyKeepingSource,
    /// It stays where it is.
    Skip(SkipReason),
}

/// One file of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedItem {
    /// What it is.
    pub kind: ItemKind,
    /// The library file, for a consolidation.
    pub file: Option<FileId>,
    /// Where it is.
    pub from: PathBuf,
    /// Where it goes (for an identical file, the file that is there).
    pub to: PathBuf,
    /// Its size in bytes.
    pub size: u64,
    /// What happens to it.
    pub action: ItemAction,
    /// Whether the destination got a number because its name was taken.
    pub renamed: bool,
    /// Whether the file is written with changes (a CUE sheet's FILE lines).
    pub rewritten: bool,
}

impl ManagedItem {
    /// Whether the file is copied or moved.
    pub fn transfers(&self) -> bool {
        !matches!(self.action, ItemAction::Skip(_))
    }
}

/// A plan: what an import or a consolidation would do. Nothing has changed yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPlan {
    /// What is planned.
    pub operation: ManagedOperation,
    /// The managed folder and how files are named in it.
    pub options: ManagedOptions,
    /// Every file, audio files first.
    pub items: Vec<ManagedItem>,
    /// The bytes the files take in the managed folder: every copy, and every move
    /// across file systems.
    pub bytes_needed: u64,
    /// The bytes free on the managed folder's file system, when known.
    pub free_bytes: Option<u64>,
    /// The folders and files given to an import, up to which a move tidies the
    /// folders it empties.
    pub(crate) bases: Vec<PathBuf>,
}

impl ManagedPlan {
    /// How many files are copied or moved.
    pub fn to_transfer(&self) -> usize {
        self.items.iter().filter(|i| i.transfers()).count()
    }

    /// How many files get a number because their name was taken.
    pub fn renamed(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.transfers() && i.renamed)
            .count()
    }

    /// How many files stay where they are.
    pub fn skipped(&self) -> usize {
        self.items.len() - self.to_transfer()
    }

    /// The bytes of the files that are copied or moved.
    pub fn total_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| i.transfers())
            .map(|i| i.size)
            .sum()
    }

    /// Whether the files fit in the free space (true when it is not known).
    pub fn fits(&self) -> bool {
        self.free_bytes.is_none_or(|free| self.bytes_needed <= free)
    }

    /// A plan made of these parts, for a front end that shows invented plans (its
    /// sample content). The library's own plans come from
    /// [`Library::plan_managed_import`] and [`Library::plan_consolidate`]; a run of
    /// a plan made here tidies no emptied folders.
    pub fn from_parts(
        operation: ManagedOperation,
        options: ManagedOptions,
        items: Vec<ManagedItem>,
        bytes_needed: u64,
        free_bytes: Option<u64>,
    ) -> Self {
        Self {
            operation,
            options,
            items,
            bytes_needed,
            free_bytes,
            bases: Vec::new(),
        }
    }
}

/// A run in the journal of the managed folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ManagedRunId(pub(crate) i64);

impl ManagedRunId {
    /// The number of the run in the database.
    pub fn get(self) -> i64 {
        self.0
    }

    /// The id with the number `id`; it names a run only if the library gave it out.
    pub fn from_raw(id: i64) -> Self {
        Self(id)
    }
}

/// What became of one file of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ItemResult {
    /// It was copied.
    Copied,
    /// It was moved.
    Moved,
    /// It was copied, but its source could not be removed and stays; the text
    /// says why.
    SourceKept(String),
    /// It stayed where it was.
    Skipped(SkipReason),
    /// The run stopped before it; it waits for the run to go on.
    Waiting,
    /// A rollback put it back as it was.
    RolledBack,
    /// A rollback could not put it back; the text says why.
    NotRolledBack(String),
}

/// One file of a run's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedOutcome {
    /// What it is.
    pub kind: ItemKind,
    /// The library file, for a consolidation.
    pub file: Option<FileId>,
    /// Where it was.
    pub from: PathBuf,
    /// Where it went, or would have.
    pub to: PathBuf,
    /// What became of it.
    pub result: ItemResult,
}

/// The report of a run, a continuation or a rollback.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManagedReport {
    /// The run in the journal; `None` when nothing was to be done.
    pub run: Option<ManagedRunId>,
    /// Each file of the run, in order.
    pub outcomes: Vec<ManagedOutcome>,
    /// Whether the run is finished (or rolled back); `false` when it was stopped
    /// and waits to go on or to be rolled back.
    pub finished: bool,
    /// Why the library could not read the new files, if it could not; the watcher
    /// or the next scan finds them then.
    pub library_error: Option<String>,
}

impl ManagedReport {
    /// How many files were copied or moved.
    pub fn transferred(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| {
                matches!(
                    o.result,
                    ItemResult::Copied | ItemResult::Moved | ItemResult::SourceKept(_)
                )
            })
            .count()
    }

    /// The outcomes with a problem: files that failed, or could not be rolled back.
    pub fn problems(&self) -> impl Iterator<Item = &ManagedOutcome> {
        self.outcomes.iter().filter(|o| {
            matches!(
                o.result,
                ItemResult::Skipped(SkipReason::Failed(_)) | ItemResult::NotRolledBack(_)
            )
        })
    }
}

/// A run of the journal that was cut short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedRunSummary {
    /// The run.
    pub run: ManagedRunId,
    /// What it does.
    pub operation: ManagedOperation,
    /// Whether it copies or moves.
    pub transfer: Transfer,
    /// When it started, in seconds since the Unix epoch.
    pub started_at: i64,
    /// The managed folder.
    pub folder: PathBuf,
    /// The files it finished.
    pub done: usize,
    /// The files it has yet to do.
    pub waiting: usize,
    /// The files it skipped.
    pub skipped: usize,
    /// The bytes of the files it finished.
    pub done_bytes: u64,
    /// The bytes of the files it has yet to do.
    pub waiting_bytes: u64,
}

/// Why a plan or a run could not start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ManagedError {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The pattern cannot be read.
    #[error("the pattern cannot be read: {0}")]
    Pattern(#[from] PatternError),
    /// The files could not be read.
    #[error(transparent)]
    Scan(#[from] ScanError),
    /// The managed folder is not in the library folder given with it.
    #[error("{} is not in the library folder {}", .folder.display(), .library_folder.display())]
    OutsideLibrary {
        /// The managed folder.
        folder: PathBuf,
        /// The library folder.
        library_folder: PathBuf,
    },
    /// The files do not fit on the managed folder's file system.
    #[error("the files need {needed} bytes, but only {free} are free")]
    NoSpace {
        /// The bytes needed.
        needed: u64,
        /// The bytes free.
        free: u64,
    },
    /// An earlier run was cut short: continue it or roll it back first.
    #[error(
        "an earlier run of the managed folder was cut short; continue it or roll it back first"
    )]
    Interrupted(ManagedRunId),
    /// Every numbered form of a name is taken.
    #[error("there is no free name for {}", .0.display())]
    NoFreeName(PathBuf),
    /// The run asked for is not an interrupted run.
    #[error("that run is not waiting to go on")]
    NotInterrupted,
}

impl Library {
    /// Plans importing `paths` (files and folders outside the library) into the
    /// managed folder. The audio files are read by the scan workers of `scan`.
    /// Changes nothing.
    pub fn plan_managed_import(
        &self,
        paths: &[PathBuf],
        options: &ManagedOptions,
        scan: &ScanOptions,
        cancel: &CancelToken,
    ) -> Result<ManagedPlan, ManagedError> {
        plan::import(self.database(), paths, options, scan, cancel)
    }

    /// How many library files that are present are outside `folder`, the managed
    /// folder: the files a consolidation would look at. Reads only the database.
    pub fn managed_files_outside(&self, folder: &Path) -> Result<usize, ManagedError> {
        Ok(plan::outside(self.database(), folder)?)
    }

    /// Plans consolidating the library files outside the managed folder into it.
    /// Changes nothing.
    pub fn plan_consolidate(&self, options: &ManagedOptions) -> Result<ManagedPlan, ManagedError> {
        plan::consolidate(self.database(), options)
    }

    /// Carries out `plan`, after checking the free space again. The library reads
    /// the new files with `scan`. Stops between files when `cancel` is set; the
    /// run then waits to go on or to be rolled back.
    pub fn apply_managed(
        &self,
        plan: &ManagedPlan,
        scan: &ScanOptions,
        progress: Progress<'_>,
        cancel: &CancelToken,
    ) -> Result<ManagedReport, ManagedError> {
        let _guard = FILE_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        run::apply(self.database(), plan, scan, progress, cancel)
    }

    /// The run that was cut short, if there is one.
    pub fn interrupted_managed_run(&self) -> Result<Option<ManagedRunSummary>, ManagedError> {
        let _guard = FILE_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(run::interrupted(self.database())?)
    }

    /// Continues the run `run`, which was cut short, from what is on the disk.
    pub fn resume_managed_run(
        &self,
        run: ManagedRunId,
        scan: &ScanOptions,
        progress: Progress<'_>,
        cancel: &CancelToken,
    ) -> Result<ManagedReport, ManagedError> {
        let _guard = FILE_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        run::resume(self.database(), run, scan, progress, cancel)
    }

    /// Rolls back the run `run`, which was cut short: removes the copies it made,
    /// moves the files it moved back, and puts the library rows back. A file that
    /// cannot go back stays, and its outcome says why; rolling back again tries it
    /// again.
    pub fn roll_back_managed_run(
        &self,
        run: ManagedRunId,
        progress: Progress<'_>,
    ) -> Result<ManagedReport, ManagedError> {
        let _guard = FILE_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        run::roll_back(self.database(), run, progress)
    }
}
