// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening connections: where the database lives, the pragmas, and what every
//! connection registers.

use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::collation::LocaleCollation;
use super::{DbError, functions};

/// The oldest SQLite with everything the schema needs: FTS5 with the trigram tokenizer
/// and contentless tables with `contentless_delete=1`.
const MIN_SQLITE_VERSION: i32 = 3_043_000;
const MIN_SQLITE_VERSION_TEXT: &str = "3.43.0";

/// How long a connection waits for a lock held by another connection.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Where a database lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// A database file, created if it does not exist.
    File(PathBuf),
    /// A database in memory, shared by the connections of one [`Database`] and gone when
    /// it closes. For tests.
    ///
    /// [`Database`]: super::Database
    Memory(MemoryName),
}

impl Location {
    /// A new, empty database in memory.
    pub fn in_memory() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        Self::Memory(MemoryName(format!("/helikodu-{}-{number}", process::id())))
    }
}

/// The name that ties the connections of one in-memory database together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryName(String);

/// What a connection is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// The one connection that writes.
    Writer,
    /// A read-only connection of the pool.
    Reader,
}

/// Opens a connection with the pragmas for its role, the `HK_LOCALE` collation and the
/// SQL functions.
pub(crate) fn open(
    location: &Location,
    role: Role,
    collation: &LocaleCollation,
) -> Result<Connection, DbError> {
    check_sqlite_version()?;
    let access = match role {
        Role::Writer => OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        Role::Reader => OpenFlags::SQLITE_OPEN_READ_ONLY,
    };
    let flags = access | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI;
    let connection = match location {
        Location::File(path) => Connection::open_with_flags(path, flags)?,
        // The memdb VFS shares a database between the connections that open the same
        // name starting with `/`, and handles locking like a file.
        Location::Memory(MemoryName(name)) => {
            Connection::open_with_flags(format!("file:{name}?vfs=memdb"), flags)?
        }
    };
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "temp_store", "MEMORY")?;
    match role {
        Role::Writer => {
            if let Location::File(_) = location {
                // Readers do not block the writer, nor the writer the readers.
                let mode: String =
                    connection
                        .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
                if !mode.eq_ignore_ascii_case("wal") {
                    return Err(DbError::MissingSqliteFeature("write-ahead logging"));
                }
            }
            // A committed batch survives a power loss, not only a crash. Writes are
            // batched, so the extra sync per commit is cheap.
            connection.pragma_update(None, "synchronous", "FULL")?;
            connection.pragma_update(None, "foreign_keys", true)?;
            connection.pragma_update(None, "cache_size", -16_384)?;
            connection.pragma_update(None, "journal_size_limit", 64 * 1024 * 1024)?;
            check_fts5(&connection)?;
        }
        Role::Reader => {
            connection.pragma_update(None, "query_only", true)?;
            connection.pragma_update(None, "cache_size", -8_192)?;
        }
    }
    collation.register(&connection)?;
    functions::register(&connection)?;
    Ok(connection)
}

fn check_sqlite_version() -> Result<(), DbError> {
    if rusqlite::version_number() < MIN_SQLITE_VERSION {
        return Err(DbError::SqliteTooOld {
            found: rusqlite::version(),
            required: MIN_SQLITE_VERSION_TEXT,
        });
    }
    Ok(())
}

fn check_fts5(connection: &Connection) -> Result<(), DbError> {
    let fts5: bool = connection.query_row(
        "SELECT sqlite_compileoption_used('ENABLE_FTS5')",
        [],
        |row| row.get(0),
    )?;
    if fts5 {
        Ok(())
    } else {
        Err(DbError::MissingSqliteFeature("FTS5"))
    }
}
