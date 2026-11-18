// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The library database: connections, the schema and its migrations, the collation and
//! the SQL functions.
//!
//! One writer thread owns the only writing connection and batches writes in
//! transactions ([`Writer`]). A small pool of read-only connections serves queries
//! ([`Database::read`]). The database file is in WAL mode, so readers never wait for the
//! writer.

mod collation;
mod connection;
mod error;
mod functions;
mod migrations;
mod path_bytes;
mod read_pool;
pub(crate) mod unix_time;
mod writer;

use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension};

pub use collation::LocaleCollation;
pub use connection::{Location, MemoryName};
pub use error::{DbError, SqliteError};
pub use migrations::SCHEMA_VERSION;
pub use path_bytes::{display_text, path_from_blob, path_to_blob};
pub use writer::{WriteTicket, Writer};

use connection::Role;
use read_pool::ReadPool;
use writer::WriterThread;

/// The `db_meta` key of the locale the `HK_LOCALE` indexes were built for.
const COLLATION_LOCALE_KEY: &str = "collation_locale";

/// How to open a [`Database`].
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// Where the database lives.
    pub location: Location,
    /// The sort order of the `HK_LOCALE` collation: the interface language.
    pub collation: LocaleCollation,
    /// The number of read-only connections.
    pub read_connections: usize,
    /// The most writes committed in one transaction.
    pub max_batch: usize,
    /// The most writes waiting for the writer before [`Writer::submit`] blocks.
    pub queue_capacity: usize,
}

impl DatabaseConfig {
    /// A database file at `path`, created if it does not exist.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::at(Location::File(path.into()))
    }

    /// A new, empty database in memory, for tests.
    pub fn in_memory() -> Self {
        Self::at(Location::in_memory())
    }

    fn at(location: Location) -> Self {
        Self {
            location,
            collation: LocaleCollation::root(),
            read_connections: 4,
            max_batch: 512,
            queue_capacity: 4096,
        }
    }

    /// The same configuration with the sort order of `collation`.
    #[must_use]
    pub fn with_collation(mut self, collation: LocaleCollation) -> Self {
        self.collation = collation;
        self
    }
}

/// An open library database, with direct SQL access.
///
/// This is the layer below [`Library`](crate::Library), reachable through
/// [`raw`](crate::raw): its read and write callbacks receive SQLite connections. Other
/// crates use the typed operations of `Library`.
///
/// Dropping it commits the writes already queued and stops the writer thread.
#[derive(Debug)]
pub struct Database {
    // Declared before the pool, so the writer finishes first and the last connection to
    // close is a reader, which keeps an in-memory database alive until the end.
    writer: Option<WriterThread>,
    readers: ReadPool,
    schema_version: u32,
    location: Location,
}

impl Database {
    /// Opens the database, creating it or bringing its schema up to date as needed.
    ///
    /// Refuses a database written by a newer version ([`DbError::NewerSchema`]); such a
    /// database can still be opened with [`open_read_only`](Self::open_read_only).
    /// If the database's sort indexes were built for another locale, they are rebuilt.
    pub fn open(config: &DatabaseConfig) -> Result<Self, DbError> {
        let mut connection = connection::open(&config.location, Role::Writer, &config.collation)?;
        let schema_version = migrations::migrate(&mut connection)?;
        apply_collation_locale(&mut connection, &config.collation)?;
        // The writer's connection stays open while the readers open, which keeps an
        // in-memory database alive.
        let readers = ReadPool::open(&config.location, config.read_connections, &config.collation)?;
        let writer = WriterThread::start(connection, config.queue_capacity, config.max_batch)?;
        Ok(Self {
            writer: Some(writer),
            readers,
            schema_version,
            location: config.location.clone(),
        })
    }

    /// Opens an existing database for reading only, without migrating it.
    ///
    /// This is for a database written by a newer version: its data can be shown, but
    /// writes fail with [`DbError::ReadOnly`]. Queries may still fail if the newer schema
    /// changed what they use. The sort indexes are not rebuilt, so if they were built
    /// for another locale, sorting through them may be out of order.
    pub fn open_read_only(config: &DatabaseConfig) -> Result<Self, DbError> {
        let readers = ReadPool::open(&config.location, config.read_connections, &config.collation)?;
        let schema_version = readers.read(migrations::current_version)?;
        Ok(Self {
            writer: None,
            readers,
            schema_version,
            location: config.location.clone(),
        })
    }

    /// The schema version of the database. Greater than [`SCHEMA_VERSION`] only for a
    /// database opened with [`open_read_only`](Self::open_read_only).
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// The handle for writes.
    pub fn writer(&self) -> Result<&Writer, DbError> {
        self.writer
            .as_ref()
            .map(WriterThread::writer)
            .ok_or(DbError::ReadOnly)
    }

    /// Runs `read` on a read-only connection, inside a read transaction so that all its
    /// queries see the same snapshot. Waits while every connection is in use.
    pub fn read<T>(
        &self,
        read: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, DbError> {
        self.readers.read(read)
    }

    /// Where the database lives.
    pub(crate) fn location(&self) -> &Location {
        &self.location
    }

    /// Writes a consistent copy of the database to the new file `target`
    /// (`VACUUM INTO`), from a reading connection, so that writes go on meanwhile.
    pub(crate) fn copy_to(&self, target: &std::path::Path) -> Result<(), DbError> {
        let Some(target) = target.to_str() else {
            return Err(rusqlite::Error::InvalidPath(target.to_path_buf()).into());
        };
        self.readers.outside_transaction(|c| {
            // The connection is opened read-only, so the database itself cannot
            // change; `query_only` would refuse the statement.
            c.pragma_update(None, "query_only", false)?;
            let copied = c.execute("VACUUM INTO ?1", [target]).map(drop);
            c.pragma_update(None, "query_only", true)?;
            copied
        })
    }

    /// Switches the sort order to `collation` and rebuilds the indexes that use it.
    ///
    /// Waits until the rebuild is done. Queries running at the same time may still sort
    /// in the old order.
    pub fn set_collation(&self, collation: LocaleCollation) -> Result<(), DbError> {
        let writer = self.writer()?;
        let for_writer = collation.clone();
        writer.exclusive(move |connection| {
            for_writer.register(connection)?;
            apply_collation_locale(connection, &for_writer)
        })?;
        self.readers.set_collation(&collation);
        Ok(())
    }
}

/// Rebuilds the `HK_LOCALE` indexes if they were built for another locale, and records
/// the locale.
fn apply_collation_locale(
    connection: &mut Connection,
    collation: &LocaleCollation,
) -> Result<(), DbError> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let built_for: Option<String> = transaction
        .query_row(
            "SELECT value FROM db_meta WHERE key = ?1",
            [COLLATION_LOCALE_KEY],
            |row| row.get(0),
        )
        .optional()?;
    if built_for.as_deref() != Some(collation.locale()) {
        if built_for.is_some() {
            transaction.execute_batch(&format!("REINDEX {}", collation::COLLATION_NAME))?;
        }
        transaction.execute(
            "INSERT INTO db_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            (COLLATION_LOCALE_KEY, collation.locale()),
        )?;
    }
    transaction.commit()?;
    Ok(())
}
