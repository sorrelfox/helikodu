// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Another player's SQLite database, read from a copy.
//!
//! The player may be running and hold the database open: the file and its
//! write-ahead log are copied to a new temporary folder, and only the copy is
//! opened. The original is never opened, so it cannot be locked or changed.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, OpenFlags};

use super::ImportError;

/// A temporary folder, removed with what is in it when dropped.
#[derive(Debug)]
struct TempFolder(PathBuf);

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A copy of a database in a temporary folder, removed when dropped.
#[derive(Debug)]
pub(super) struct DatabaseCopy {
    // Fields are dropped in order: the connection closes before the folder goes.
    connection: Connection,
    _folder: TempFolder,
}

impl DatabaseCopy {
    /// Copies the database at `path` (with its `-wal` file, if any) and opens the
    /// copy.
    pub(super) fn open(path: &Path) -> Result<Self, ImportError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let io = |source| ImportError::Io {
            path: path.to_path_buf(),
            source,
        };
        if !path.is_file() {
            return Err(io(std::io::Error::from(std::io::ErrorKind::NotFound)));
        }
        let folder = TempFolder(std::env::temp_dir().join(format!(
            "helikodu-import-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )));
        fs::create_dir_all(&folder.0).map_err(io)?;
        let target = folder.0.join("library.db");
        fs::copy(path, &target).map_err(io)?;
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        let wal = PathBuf::from(wal);
        if wal.is_file() {
            fs::copy(&wal, folder.0.join("library.db-wal")).map_err(io)?;
        }
        let connection = Connection::open_with_flags(
            &target,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| format_error(path, &e))?;
        // Reading the schema tells a database from another file at once.
        connection
            .query_row("SELECT count(*) FROM sqlite_master", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|e| format_error(path, &e))?;
        Ok(Self {
            connection,
            _folder: folder,
        })
    }

    /// The connection to the copy.
    pub(super) fn connection(&self) -> &Connection {
        &self.connection
    }
}

/// A database error as a format error of `path`.
pub(super) fn format_error(path: &Path, error: &rusqlite::Error) -> ImportError {
    ImportError::Format {
        path: path.to_path_buf(),
        reason: format!("cannot read the database: {error}"),
    }
}

/// The columns of `table`, in lower case; empty if there is no such table.
pub(super) fn columns(c: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = c.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let names = statement.query_map([table], |row| row.get::<_, String>(0))?;
    names.map(|n| n.map(|n| n.to_ascii_lowercase())).collect()
}
