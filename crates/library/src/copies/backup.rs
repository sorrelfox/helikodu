// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The one backup of the database before the statistics of copies are first merged:
//! `library.db.backup-before-copies` beside the database file.
//!
//! It is written once per database, by `VACUUM INTO` from a reading connection, to
//! a temporary name that is then renamed, so a half-written backup never carries
//! the final name. A backup file that is already there is kept: it is the older one.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::db::{Database, DbError, Location};

/// The suffix of the backup file's name.
pub const BACKUP_SUFFIX: &str = ".backup-before-copies";

/// The backup file of the database at `database`.
pub fn backup_path(database: &Path) -> PathBuf {
    let mut name = OsString::from(database.as_os_str());
    name.push(BACKUP_SUFFIX);
    PathBuf::from(name)
}

/// Makes sure the backup exists. A database in memory needs none.
pub(crate) fn ensure(db: &Database) -> Result<(), DbError> {
    let Location::File(path) = db.location() else {
        return Ok(());
    };
    let target = backup_path(path);
    if target.exists() {
        return Ok(());
    }
    let mut partial = OsString::from(target.as_os_str());
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let _ = fs::remove_file(&partial);
    if let Err(e) = db.copy_to(&partial) {
        let _ = fs::remove_file(&partial);
        return Err(e);
    }
    fs::rename(&partial, &target).map_err(|source| {
        let _ = fs::remove_file(&partial);
        DbError::Backup {
            path: target.clone(),
            source,
        }
    })
}
