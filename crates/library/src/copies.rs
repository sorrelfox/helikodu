// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Copies: byte-identical files in the library (content and tags), and the
//! duplicate finder.
//!
//! - **Recognising copies.** Files that share their size and quick hash are hashed
//!   whole (SHA-256, stored with the file's stamp and computed again when the stamp
//!   changes); files with equal hashes form a group. The device and inode of each
//!   file tell two paths to one file (a hard link, a bind mount, a library folder
//!   reached through a symbolic link) from two copies: such paths are never offered
//!   for the trash. A scan confirms copies at its end.
//! - **One identity.** The tracks of one section of the files of a group share one
//!   identity: ratings, plays, history, resume positions and playlist entries belong
//!   to the music, and playing it plays the main copy. Groups found in a library
//!   whose copies had identities of their own are merged (`merge`), after a one-time
//!   backup of the database (`backup`).
//! - **The main copy** of a group is the copy that entered the library first (then
//!   the shorter path, then byte order), or the one the owner chose. When it goes
//!   missing the next copy takes over, and it does not take the role back when it
//!   returns.
//! - **Settings** (`[library.duplicates]`): with `on_scan = "skip"` copies get no
//!   tracks (they are recorded with their group); with `show_one` every query shows
//!   only the main copy's tracks.
//! - **Removing copies** moves them to the desktop trash after checking, right before
//!   each move, that the file is unchanged, still identical to the main copy and not
//!   the same file as another (`remove`).

pub(crate) mod backup;
mod finder;
pub(crate) mod groups;
mod hash;
mod list;
mod merge;
pub(crate) mod precheck;
pub(crate) mod reconcile;
mod remove;
pub(crate) mod settings;

use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

use helikodu_model::library::TrackId;
use helikodu_model::settings::DuplicateSettings;

pub use backup::{BACKUP_SUFFIX, backup_path};
pub use finder::{DuplicateCounts, DuplicateEntry, DuplicateGroup, KeepRule, MatchBy};

use crate::Library;
use crate::db::DbError;
use crate::maintenance::{Trash, Trashed};
use crate::scan::{CancelToken, ScanError, ScanEvent, ScanOptions, ScanSummary};

/// A file of the library, as groups of copies and the duplicate finder name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub(crate) i64);

impl FileId {
    /// The row id.
    pub fn get(self) -> i64 {
        self.0
    }

    /// The id with the row id `id`; it names a file only if the library gave it out.
    pub fn from_raw(id: i64) -> Self {
        Self(id)
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A group of byte-identical files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyGroup {
    /// The main copy, whose file plays.
    pub main: FileId,
    /// The files: the main copy first, then in the order of the main-copy rule.
    pub files: Vec<CopyFile>,
}

impl CopyGroup {
    /// The files that may be moved to the trash: not the main copy, present, and
    /// not the same file as another path of the group.
    pub fn removable(&self) -> Vec<FileId> {
        self.files
            .iter()
            .filter(|f| !f.is_main && !f.missing && f.same_file_as.is_none())
            .map(|f| f.file)
            .collect()
    }
}

/// One file of a [`CopyGroup`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyFile {
    /// The file.
    pub file: FileId,
    /// Its path, as the bytes the kernel returned.
    pub path: PathBuf,
    /// The path for display.
    pub display_path: String,
    /// The library folder it is in.
    pub root: PathBuf,
    /// The codec of its tracks (or of the main copy's, for a copy without tracks).
    pub format: Option<String>,
    /// The size in bytes.
    pub size: u64,
    /// When it entered the library.
    pub added: Option<SystemTime>,
    /// Whether it is the main copy.
    pub is_main: bool,
    /// Whether it is missing, or on a drive that is not connected.
    pub missing: bool,
    /// Another path of the group that is the same file (the same device and
    /// inode): then these are one file reached by two paths, not two copies, and
    /// neither is ever moved to the trash.
    pub same_file_as: Option<FileId>,
    /// Its track of the asked section, when it has tracks of its own.
    pub track: Option<TrackId>,
}

impl CopyFile {
    /// Whether another path of the group is the same file.
    pub fn same_inode(&self) -> bool {
        self.same_file_as.is_some()
    }
}

/// Why a file was not moved to the trash. It is still where it was.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TrashRefusal {
    /// The library has no such file.
    NotInLibrary,
    /// It is in no group of copies.
    NotACopy,
    /// It is the main copy of its group.
    MainCopy,
    /// It is missing.
    Missing,
    /// It is the same file as another path in the library (the same device and
    /// inode).
    SameFile,
    /// It changed since the library read it, or is no longer identical to the
    /// main copy.
    Changed,
    /// The main copy cannot be read to compare.
    MainUnavailable,
    /// The trash did not take it (no usable trash on its volume, no permission).
    Trash(String),
}

impl fmt::Display for TrashRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInLibrary => f.write_str("the file is not in the library"),
            Self::NotACopy => f.write_str("the file has no copies"),
            Self::MainCopy => f.write_str("the file is the main copy"),
            Self::Missing => f.write_str("the file is missing"),
            Self::SameFile => {
                f.write_str("the file is the same file as another path in the library")
            }
            Self::Changed => f.write_str("the file changed, or is no longer identical"),
            Self::MainUnavailable => f.write_str("the main copy cannot be read to compare"),
            Self::Trash(reason) => f.write_str(reason),
        }
    }
}

/// What happened to one file asked to go to the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashOutcome {
    /// The file.
    pub file: FileId,
    /// Its path, if the library knew it.
    pub path: Option<PathBuf>,
    /// Where it went, or why it stayed.
    pub result: Result<Trashed, TrashRefusal>,
}

/// Why the main copy could not be changed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MainCopyError {
    /// The file is in no group of copies.
    #[error("the file has no copies")]
    NotACopy,
    /// The file is missing, or on a drive that is not connected.
    #[error("the file is missing")]
    Missing,
    /// The database failed.
    #[error(transparent)]
    Database(#[from] DbError),
}

impl Library {
    /// How copies are treated, as stored in the database.
    pub fn duplicate_settings(&self) -> Result<DuplicateSettings, DbError> {
        self.database().read(settings::read)
    }

    /// Applies `wanted`: stores it, and when the scan setting changed brings the
    /// library in line at once. Skipping copies removes their tracks; adding them
    /// reads the copies with the workers of `options`, so this can take a while.
    /// Events go to `events`. Returns what was read (an empty summary when nothing
    /// had to be).
    pub fn apply_duplicate_settings(
        &self,
        wanted: DuplicateSettings,
        options: &ScanOptions,
        events: Option<&std::sync::mpsc::Sender<ScanEvent>>,
        cancel: &CancelToken,
    ) -> Result<ScanSummary, ScanError> {
        let before = self.duplicate_settings()?;
        self.database()
            .writer()?
            .write(move |c| settings::write(c, wanted))?;
        if before.on_scan == wanted.on_scan {
            return Ok(ScanSummary::default());
        }
        crate::scan::confirm_copies(self.database(), options, &[], true, events, cancel)
    }

    /// Confirms copies and brings every group up to date, reading files with the
    /// workers of `options` where tracks are missing. A scan does this at its end;
    /// this is for a library filled otherwise.
    pub fn confirm_copies(
        &self,
        options: &ScanOptions,
        cancel: &CancelToken,
    ) -> Result<ScanSummary, ScanError> {
        crate::scan::confirm_copies(self.database(), options, &[], true, None, cancel)
    }

    /// The group of copies of the file of `track`; `None` if it has no copies.
    pub fn copies_of(&self, track: TrackId) -> Result<Option<CopyGroup>, DbError> {
        self.database().read(|c| list::copies_of(c, track))
    }

    /// Every group of copies.
    pub fn copy_groups(&self) -> Result<Vec<CopyGroup>, DbError> {
        self.database().read(list::all_groups)
    }

    /// Makes `file` the main copy of its group. With copies skipped, the tracks
    /// move to it.
    pub fn set_main_copy(&self, file: FileId) -> Result<(), MainCopyError> {
        let now = crate::db::unix_time::to_unix_seconds(SystemTime::now());
        let id = file.0;
        self.database().writer()?.write(move |c| {
            let Some(facts) = list::file_facts(c, id)? else {
                return Ok(Err(MainCopyError::NotACopy));
            };
            let Some(group) = facts.group else {
                return Ok(Err(MainCopyError::NotACopy));
            };
            if facts.missing {
                return Ok(Err(MainCopyError::Missing));
            }
            c.prepare_cached("UPDATE copy_groups SET primary_file = ?2 WHERE id = ?1")?
                .execute(rusqlite::params![group, id])?;
            let mut work = groups::Work::new(c, now)?;
            groups::refresh(c, group, &mut work)?;
            work.finish(c)?;
            Ok(Ok(()))
        })?
    }

    /// Moves the copies `files` to `trash`, each after the checks of the module
    /// `remove`: never a main copy, never a file that changed or is no longer
    /// identical to the main copy, never a path that is the same file as another.
    /// The result of each file says where it went or why it stayed.
    pub fn trash_copies(
        &self,
        files: &[FileId],
        trash: &Trash,
    ) -> Result<Vec<TrashOutcome>, DbError> {
        remove::trash_files(self.database(), files, trash, remove::Kind::Copies)
    }

    /// Moves files that the duplicate finder found by another measure than identity
    /// to `trash`: each must be unchanged and must not be the same file as any other
    /// library path. Their tracks leave the library; their identities stay.
    pub fn trash_duplicates(
        &self,
        files: &[FileId],
        trash: &Trash,
    ) -> Result<Vec<TrashOutcome>, DbError> {
        remove::trash_files(self.database(), files, trash, remove::Kind::Duplicates)
    }

    /// The groups of duplicates by `match_by`, each with the files `keep` suggests
    /// moving to the trash.
    pub fn find_duplicates(
        &self,
        match_by: MatchBy,
        keep: KeepRule,
    ) -> Result<Vec<DuplicateGroup>, DbError> {
        self.database().read(|c| finder::find(c, match_by, keep))
    }

    /// How many groups each measure of the duplicate finder finds.
    pub fn duplicate_counts(&self) -> Result<DuplicateCounts, DbError> {
        self.database().read(finder::counts)
    }
}
