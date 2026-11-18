// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Errors of the database layer.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use helikodu_model::library::TrackId;
use thiserror::Error;

use crate::catalog::RootId;

/// An error from the library database.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DbError {
    /// SQLite reported an error, or a write or read job returned one.
    #[error("database error: {0}")]
    Sqlite(#[from] SqliteError),

    /// The batch a write belonged to could not be committed, so the write was rolled
    /// back with the rest of the batch.
    #[error("the write batch could not be committed: {0}")]
    BatchFailed(Arc<SqliteError>),

    /// The database was written by a newer version of Helikodu.
    #[error(
        "the library database has schema version {found}, but this version of Helikodu \
         supports versions up to {supported}"
    )]
    NewerSchema {
        /// The version found in the database.
        found: u32,
        /// The newest version this build knows.
        supported: u32,
    },

    /// Applying a schema migration failed; the migration was rolled back.
    #[error("schema migration {version} ({name}) failed: {source}")]
    Migration {
        /// The migration's number.
        version: u32,
        /// The migration's name.
        name: &'static str,
        /// What went wrong.
        #[source]
        source: SqliteError,
    },

    /// The SQLite library is older than the oldest supported version.
    #[error("SQLite {found} is too old; version {required} or newer is needed")]
    SqliteTooOld {
        /// The version in use.
        found: &'static str,
        /// The oldest supported version.
        required: &'static str,
    },

    /// The SQLite library was built without a feature the library needs.
    #[error("SQLite was built without {0}")]
    MissingSqliteFeature(&'static str),

    /// The locale for the collation could not be parsed or has no collation data.
    #[error("cannot sort for the locale {locale:?}: {reason}")]
    Locale {
        /// The locale as given.
        locale: String,
        /// Why it was refused.
        reason: String,
    },

    /// The database was opened read-only, so it cannot be written.
    #[error("the library database is open read-only")]
    ReadOnly,

    /// The database file, its folder or the medium cannot be written, so the
    /// database can only be opened read-only.
    #[error("the library database cannot be written: the file or its folder is read-only")]
    NotWritable,

    /// The writer thread has stopped: the database was closed, or the thread panicked.
    #[error("the database writer has stopped")]
    WriterStopped,

    /// The track is not in the library.
    #[error("there is no track {0} in the library")]
    UnknownTrack(TrackId),

    /// The library folder is not in the library.
    #[error("there is no library folder {} in the library", .0.get())]
    UnknownRoot(RootId),

    /// No file at this path is in the library.
    #[error("there is no file {} in the library", .0.display())]
    UnknownPath(PathBuf),

    /// The path does not name a file: it has no file name or no parent folder.
    #[error("not a file path: {}", .0.display())]
    InvalidPath(PathBuf),

    /// The writer thread could not be started.
    #[error("could not start the database writer thread: {0}")]
    Thread(#[source] std::io::Error),

    /// The backup of the database could not be written.
    #[error("cannot write the backup {}: {source}", .path.display())]
    Backup {
        /// The backup file.
        path: PathBuf,
        /// Why.
        #[source]
        source: std::io::Error,
    },
}

impl DbError {
    /// Whether SQLite found the database file damaged or not a database at all.
    pub fn is_damaged_database(&self) -> bool {
        let sqlite = match self {
            Self::Sqlite(e) | Self::Migration { source: e, .. } => e,
            Self::BatchFailed(e) => e.as_ref(),
            _ => return false,
        };
        matches!(
            sqlite.0.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
        )
    }

    /// Whether the database could not be written because the file, its folder or
    /// the medium is read-only, or the file could not be opened for writing.
    pub fn is_read_only_medium(&self) -> bool {
        let sqlite = match self {
            Self::Sqlite(e) | Self::Migration { source: e, .. } => e,
            Self::BatchFailed(e) => e.as_ref(),
            Self::ReadOnly | Self::NotWritable => return true,
            _ => return false,
        };
        matches!(
            sqlite.0.sqlite_error_code(),
            Some(
                rusqlite::ErrorCode::ReadOnly
                    | rusqlite::ErrorCode::CannotOpen
                    | rusqlite::ErrorCode::PermissionDenied
            )
        )
    }

    /// Whether the disk was full.
    pub fn is_disk_full(&self) -> bool {
        let sqlite = match self {
            Self::Sqlite(e) | Self::Migration { source: e, .. } => e,
            Self::BatchFailed(e) => e.as_ref(),
            _ => return false,
        };
        sqlite.0.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull)
    }
}

impl From<rusqlite::Error> for DbError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(SqliteError(error))
    }
}

/// An error reported by SQLite.
///
/// The SQLite bindings stay out of the public API; [`raw`](crate::raw) gives access to
/// the error underneath for code that works with SQL directly.
pub struct SqliteError(pub(crate) rusqlite::Error);

impl SqliteError {
    /// Whether a constraint was violated: a unique key, a foreign key or a `CHECK`.
    pub fn is_constraint_violation(&self) -> bool {
        self.0.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation)
    }

    /// The error from the SQLite bindings.
    pub(crate) fn inner(&self) -> &rusqlite::Error {
        &self.0
    }
}

impl From<rusqlite::Error> for SqliteError {
    fn from(error: rusqlite::Error) -> Self {
        Self(error)
    }
}

impl fmt::Debug for SqliteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for SqliteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SqliteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}
