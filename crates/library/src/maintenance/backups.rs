// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Backups of the library database, and putting one back.
//!
//! - **Making a backup** ([`Library::back_up`]) copies the database with SQLite's
//!   online backup API from a reading connection, so writes go on meanwhile. The
//!   copy is written under a temporary name, checked (`PRAGMA quick_check`),
//!   synced and only then renamed to `library-YYYYMMDD-HHMMSS.db` (UTC) in the
//!   backups folder. A copy that fails its check is thrown away, and the check is
//!   recorded as the result of the last check. Only after a good copy is in place
//!   are the oldest backups removed, keeping [`KEPT_BACKUPS`]: a full disk or a
//!   damaged database never costs a good backup.
//! - **Putting one back** happens in two steps, so that it never swaps the file
//!   under open connections. [`stage_restore`] checks the chosen backup and copies
//!   it beside the database as `library.db.restore`; the next time the program
//!   starts, [`apply_pending_restore`] first saves the current database in the
//!   backups folder (`…-before-restore.db`, or, when it is damaged, its raw files
//!   as `…-damaged.db`), then renames the staged copy over it. A crash at any
//!   point leaves either the old database or the restored one, and a staged copy
//!   that is still there is simply applied again.
//! - **A damaged database** found at start ([`DbError::is_damaged_database`]) is
//!   set aside and replaced with the newest backup that passes its check
//!   ([`recover_damaged`]).

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};
use thiserror::Error;

use super::localtime::{civil_from_days, days_from_civil};
use crate::Library;
use crate::db::unix_time::{from_unix_seconds, to_unix_seconds};
use crate::db::{DbError, LocaleCollation, SCHEMA_VERSION, current_version, register_functions};

/// The name of the backups folder beside the database.
pub const BACKUP_FOLDER: &str = "backups";
/// How many regular backups are kept.
pub const KEPT_BACKUPS: usize = 7;
/// How often a backup is made by itself.
pub const BACKUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

const PREFIX: &str = "library-";
const EXTENSION: &str = ".db";
const BEFORE_RESTORE: &str = "-before-restore";
const DAMAGED: &str = "-damaged";
const CHECK_FILE: &str = "last-check";
const RESTORE_SUFFIX: &str = ".restore";

/// One backup file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupFile {
    /// The file.
    pub path: PathBuf,
    /// When it was made, from its name.
    pub made: SystemTime,
    /// Its size in bytes.
    pub size: u64,
}

/// The result of the last check of the database, made with each backup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseCheck {
    /// When it was checked.
    pub at: SystemTime,
    /// What the check found wrong; `None` when nothing was.
    pub problem: Option<String>,
}

/// What putting a backup back did with the database it replaced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RestoreOutcome {
    /// The copy of the replaced database, in the backups folder.
    pub saved_copy: Option<PathBuf>,
    /// The replaced database's own files, set aside because it could not be
    /// copied (it was damaged).
    pub damaged_copy: Option<PathBuf>,
}

/// Why a backup could not be made or put back.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BackupError {
    /// A file could not be written, read, renamed or removed.
    #[error("{}: {source}", .path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        #[source]
        source: io::Error,
    },
    /// SQLite reported an error.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The copy failed its check, so the database or the backup is damaged.
    #[error("the database failed its check: {0}")]
    Damaged(String),
    /// The file is not a library database.
    #[error("{} is not a Helikodu library database", .0.display())]
    NotALibrary(PathBuf),
    /// The backup was made by a newer version.
    #[error(
        "the backup was made by a newer version of Helikodu (schema version {found}; \
         this version knows up to {supported})"
    )]
    Newer {
        /// The schema version of the backup.
        found: u32,
        /// The newest version this build knows.
        supported: u32,
    },
}

impl From<rusqlite::Error> for BackupError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(error.into())
    }
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> BackupError + '_ {
    move |source| BackupError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The backups folder of a database: the regular backups, the copies set aside
/// when a backup is put back, and the result of the last check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backups {
    folder: PathBuf,
    keep: usize,
}

impl Backups {
    /// The backups in `folder`, keeping [`KEPT_BACKUPS`].
    pub fn new(folder: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
            keep: KEPT_BACKUPS,
        }
    }

    /// The backups folder beside the database at `database`.
    pub fn beside(database: &Path) -> Self {
        Self::new(
            database
                .parent()
                .unwrap_or(Path::new("."))
                .join(BACKUP_FOLDER),
        )
    }

    /// The same folder, keeping `keep` regular backups (at least one).
    #[must_use]
    pub fn keeping(mut self, keep: usize) -> Self {
        self.keep = keep.max(1);
        self
    }

    /// The folder.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// How many regular backups are kept.
    pub fn kept(&self) -> usize {
        self.keep
    }

    /// The regular backups, newest first. A missing folder has none.
    pub fn list(&self) -> Vec<BackupFile> {
        let mut found = self.files("");
        found.sort_by(|a, b| b.made.cmp(&a.made).then_with(|| b.path.cmp(&a.path)));
        found
    }

    /// The newest regular backup.
    pub fn latest(&self) -> Option<BackupFile> {
        self.list().into_iter().next()
    }

    /// Whether a backup is due at `now`: there is none, or the newest is older
    /// than [`BACKUP_INTERVAL`] (or from the future, after the clock was set back).
    pub fn is_due(&self, now: SystemTime) -> bool {
        self.next_due(now) <= now
    }

    /// When the next backup is due.
    pub fn next_due(&self, now: SystemTime) -> SystemTime {
        match self.latest() {
            Some(latest) if latest.made <= now => latest.made + BACKUP_INTERVAL,
            _ => now,
        }
    }

    /// The result of the last check of the database.
    pub fn last_check(&self) -> Option<DatabaseCheck> {
        let text = fs::read_to_string(self.folder.join(CHECK_FILE)).ok()?;
        let (at, rest) = text.split_once('\n').unwrap_or((&text, ""));
        let at = from_unix_seconds(at.trim().parse().ok()?);
        let rest = rest.trim();
        Some(DatabaseCheck {
            at,
            problem: (rest != "ok").then(|| rest.to_owned()),
        })
    }

    fn record_check(&self, at: SystemTime, problem: Option<&str>) {
        let text = format!("{}\n{}\n", to_unix_seconds(at), problem.unwrap_or("ok"));
        // The record is a convenience; a backup does not fail for it.
        let _ = write_atomic(&self.folder.join(CHECK_FILE), text.as_bytes());
    }

    /// The files named `library-<stamp><tag>.db`.
    fn files(&self, tag: &str) -> Vec<BackupFile> {
        let Ok(entries) = fs::read_dir(&self.folder) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let made = parse_name(name.to_str()?, tag)?;
                let meta = entry.metadata().ok().filter(fs::Metadata::is_file)?;
                Some(BackupFile {
                    path: entry.path(),
                    made,
                    size: meta.len(),
                })
            })
            .collect()
    }

    /// Removes the regular backups beyond the kept number, and all but the newest
    /// copy set aside by a restore of each kind.
    fn rotate(&self) {
        for stale in self.list().into_iter().skip(self.keep) {
            let _ = fs::remove_file(&stale.path);
        }
        for tag in [BEFORE_RESTORE, DAMAGED] {
            let mut files = self.files(tag);
            files.sort_by_key(|a| std::cmp::Reverse(a.made));
            for stale in files.into_iter().skip(1) {
                let _ = fs::remove_file(&stale.path);
                let _ = fs::remove_file(sibling(&stale.path, "-wal"));
            }
        }
    }

    fn name_for(&self, made: SystemTime, tag: &str) -> PathBuf {
        self.folder
            .join(format!("{PREFIX}{}{tag}{EXTENSION}", stamp(made)))
    }
}

/// `YYYYMMDD-HHMMSS` in UTC.
fn stamp(at: SystemTime) -> String {
    let unix = to_unix_seconds(at);
    let (y, m, d) = civil_from_days(unix.div_euclid(86_400));
    let secs = unix.rem_euclid(86_400);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// The time in a name `library-YYYYMMDD-HHMMSS<tag>.db`.
fn parse_name(name: &str, tag: &str) -> Option<SystemTime> {
    let stamp = name
        .strip_prefix(PREFIX)?
        .strip_suffix(EXTENSION)?
        .strip_suffix(tag)?;
    let (date, time) = stamp.split_once('-')?;
    if date.len() != 8
        || time.len() != 6
        || !(date.chars().chain(time.chars())).all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let number = |s: &str| s.parse::<i64>().ok();
    let (y, m, d) = (
        number(&date[..4])?,
        number(&date[4..6])?,
        number(&date[6..])?,
    );
    let (h, min, s) = (
        number(&time[..2])?,
        number(&time[2..4])?,
        number(&time[4..])?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || min > 59 || s > 60 {
        return None;
    }
    Some(from_unix_seconds(
        days_from_civil(y, m, d) * 86_400 + h * 3600 + min * 60 + s,
    ))
}

/// `path` with `suffix` added to its name.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

/// Where a backup chosen to be put back waits for the next start.
pub fn restore_path(database: &Path) -> PathBuf {
    sibling(database, RESTORE_SUFFIX)
}

/// The backup waiting to be put back at the next start, if any.
pub fn pending_restore(database: &Path) -> Option<PathBuf> {
    let staged = restore_path(database);
    staged.is_file().then_some(staged)
}

/// Drops the backup waiting to be put back.
pub fn cancel_restore(database: &Path) -> Result<(), BackupError> {
    let staged = restore_path(database);
    match fs::remove_file(&staged) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io_error(&staged)(e)),
        _ => Ok(()),
    }
}

impl Library {
    /// Makes a backup in `backups`, dated `now`, and removes the oldest beyond the
    /// kept number. See the [module documentation](self).
    pub fn back_up(&self, backups: &Backups, now: SystemTime) -> Result<BackupFile, BackupError> {
        fs::create_dir_all(backups.folder()).map_err(io_error(backups.folder()))?;
        let target = backups.name_for(now, "");
        let copied = self
            .database()
            .with_reader(|source| copy_database(source, &target))?;
        match copied {
            Ok(()) => backups.record_check(now, None),
            Err(BackupError::Damaged(problem)) => {
                backups.record_check(now, Some(&problem));
                return Err(BackupError::Damaged(problem));
            }
            Err(e) => return Err(e),
        }
        backups.rotate();
        let size = fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        Ok(BackupFile {
            path: target,
            made: from_unix_seconds(to_unix_seconds(now)),
            size,
        })
    }
}

/// Checks the backup at `backup` and copies it beside `database`, to be put
/// back at the next start ([`apply_pending_restore`]).
pub fn stage_restore(database: &Path, backup: &Path) -> Result<(), BackupError> {
    let source = Connection::open_with_flags(
        backup,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| BackupError::NotALibrary(backup.to_path_buf()))?;
    let version = current_version(&source)
        .ok()
        .filter(|&v| v > 0)
        .ok_or_else(|| BackupError::NotALibrary(backup.to_path_buf()))?;
    if version > SCHEMA_VERSION {
        return Err(BackupError::Newer {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    copy_database(&source, &restore_path(database))
}

/// Puts back the backup staged by [`stage_restore`], if there is one. Call it
/// before the database is opened.
pub fn apply_pending_restore(
    database: &Path,
    backups: &Backups,
    now: SystemTime,
) -> Result<Option<RestoreOutcome>, BackupError> {
    let Some(staged) = pending_restore(database) else {
        return Ok(None);
    };
    fs::create_dir_all(backups.folder()).map_err(io_error(backups.folder()))?;
    let mut outcome = RestoreOutcome::default();
    if database.exists() {
        let copy = backups.name_for(now, BEFORE_RESTORE);
        let saved = Connection::open_with_flags(
            database,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(BackupError::from)
        .and_then(|current| {
            let copied = copy_database(&current, &copy);
            // Closing the last connection folds the write-ahead log into the
            // database and removes it.
            drop(current);
            copied
        });
        if saved.is_ok() {
            outcome.saved_copy = Some(copy);
        } else {
            // The database cannot be read whole: keep its own files instead.
            let aside = backups.name_for(now, DAMAGED);
            let wal = sibling(database, "-wal");
            if wal.exists() {
                fs::rename(&wal, sibling(&aside, "-wal")).map_err(io_error(&wal))?;
            }
            fs::rename(database, &aside).map_err(io_error(database))?;
            outcome.damaged_copy = Some(aside);
        }
    }
    // A log left beside the database belongs to the old file and must never be
    // read into the restored one.
    for suffix in ["-wal", "-shm", "-journal"] {
        let file = sibling(database, suffix);
        match fs::remove_file(&file) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(io_error(&file)(e)),
            _ => {}
        }
    }
    fs::rename(&staged, database).map_err(io_error(database))?;
    sync_folder(database);
    backups.rotate();
    Ok(Some(outcome))
}

/// Replaces the damaged database at `database` with the newest backup that
/// passes its check. Returns the backup used, or `None` when there was none.
pub fn recover_damaged(
    database: &Path,
    backups: &Backups,
    now: SystemTime,
) -> Result<Option<(BackupFile, RestoreOutcome)>, BackupError> {
    for backup in backups.list() {
        if stage_restore(database, &backup.path).is_err() {
            let _ = cancel_restore(database);
            continue;
        }
        if let Some(outcome) = apply_pending_restore(database, backups, now)? {
            return Ok(Some((backup, outcome)));
        }
    }
    Ok(None)
}

/// Copies the database open on `source` to the new file `target`: written under
/// a temporary name, checked, synced, then renamed.
fn copy_database(source: &Connection, target: &Path) -> Result<(), BackupError> {
    let partial = sibling(target, ".partial");
    let _ = fs::remove_file(&partial);
    let copied = (|| {
        let mut copy = Connection::open(&partial)?;
        {
            let backup = Backup::new(source, &mut copy)?;
            loop {
                match backup.step(-1)? {
                    StepResult::Done => break,
                    StepResult::More => {}
                    _ => thread::sleep(Duration::from_millis(20)),
                }
            }
        }
        // One self-contained file, whatever mode the source was in.
        let _: String = copy.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
        check(&copy)?;
        drop(copy);
        File::open(&partial)
            .and_then(|f| f.sync_all())
            .map_err(io_error(&partial))?;
        fs::rename(&partial, target).map_err(io_error(target))?;
        sync_folder(target);
        Ok(())
    })();
    if copied.is_err() {
        let _ = fs::remove_file(&partial);
        let _ = fs::remove_file(sibling(&partial, "-journal"));
    }
    copied
}

/// `PRAGMA quick_check` on `connection`: the pages, records and indexes are
/// well-formed.
fn check(connection: &Connection) -> Result<(), BackupError> {
    // The indexes and triggers name the collation and the SQL functions.
    LocaleCollation::root().register(connection)?;
    register_functions(connection)?;
    let mut statement = connection.prepare("PRAGMA quick_check(10)")?;
    let rows: Vec<String> = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if rows.len() == 1 && rows[0] == "ok" {
        Ok(())
    } else {
        Err(BackupError::Damaged(rows.join("; ")))
    }
}

fn sync_folder(file: &Path) {
    if let Some(folder) = file.parent()
        && let Ok(dir) = File::open(folder)
    {
        let _ = dir.sync_all();
    }
}

/// Replaces `path` with `contents` through a synced temporary file.
fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary = sibling(path, ".tmp");
    let written = (|| {
        let mut file = File::create(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        let at = from_unix_seconds(1_795_000_000);
        let name = format!("{PREFIX}{}{EXTENSION}", stamp(at));
        assert_eq!(name, "library-20261118-110640.db");
        assert_eq!(parse_name(&name, ""), Some(at));
        assert_eq!(
            parse_name("library-20261118-110640-before-restore.db", ""),
            None
        );
        assert_eq!(
            parse_name("library-20261118-110640-before-restore.db", BEFORE_RESTORE),
            Some(at)
        );
        assert_eq!(parse_name("library-2026101-085320.db", ""), None);
        assert_eq!(parse_name("library-20261318-085320.db", ""), None);
        assert_eq!(parse_name("library.db", ""), None);
    }
}
