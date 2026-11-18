// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The schema migrations in `migrations/` and the runner that applies them.
//!
//! Each migration is one numbered SQL file. A migration that has been released is never
//! edited; a change to the schema is a new file, listed in [`MIGRATIONS`].
//!
//! The `schema_migrations` table records the migrations applied to a database. Each
//! migration runs in its own transaction together with its record, so it is applied
//! completely or not at all. Foreign keys are off while a migration runs, so that a
//! table can be rebuilt, and are checked before the transaction commits.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use super::DbError;

/// One schema change.
#[derive(Debug)]
pub(crate) struct Migration {
    pub(crate) version: u32,
    pub(crate) name: &'static str,
    pub(crate) sql: &'static str,
}

macro_rules! migration {
    ($version:literal, $number:literal, $name:literal) => {
        Migration {
            version: $version,
            name: $name,
            sql: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/migrations/",
                $number,
                "_",
                $name,
                ".sql"
            )),
        }
    };
}

/// Every migration, in order. The version of the last one is the schema version.
pub(crate) const MIGRATIONS: &[Migration] = &[
    migration!(1, "0001", "library_schema"),
    migration!(2, "0002", "search_index"),
    migration!(3, "0003", "identity_fingerprint"),
    migration!(4, "0004", "search_support"),
    migration!(5, "0005", "album_model"),
    migration!(6, "0006", "covers"),
    migration!(7, "0007", "static_playlists"),
    migration!(8, "0008", "loudness_measurements"),
    migration!(9, "0009", "saved_searches"),
    migration!(10, "0010", "copies"),
];

/// The schema version this build creates and understands.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

const CREATE_TABLE: &str = "CREATE TABLE IF NOT EXISTS schema_migrations (
    version    INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    applied_at INTEGER NOT NULL
) STRICT";

/// The schema version of the database, 0 for an empty one.
pub(crate) fn current_version(connection: &Connection) -> rusqlite::Result<u32> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok(0);
    }
    connection.query_row(
        "SELECT coalesce(max(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )
}

/// Brings the database up to [`SCHEMA_VERSION`] and returns the version.
///
/// Refuses a database whose version is newer than this build knows.
pub(crate) fn migrate(connection: &mut Connection) -> Result<u32, DbError> {
    let found = current_version(connection)?;
    if found > SCHEMA_VERSION {
        return Err(DbError::NewerSchema {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    for migration in MIGRATIONS.iter().filter(|m| m.version > found) {
        apply(connection, migration)?;
    }
    Ok(current_version(connection)?.max(found))
}

fn apply(connection: &mut Connection, migration: &Migration) -> Result<(), DbError> {
    let failed = |source: rusqlite::Error| DbError::Migration {
        version: migration.version,
        name: migration.name,
        source: source.into(),
    };
    // `foreign_keys` cannot change inside a transaction.
    connection
        .pragma_update(None, "foreign_keys", false)
        .map_err(failed)?;
    let result = apply_in_transaction(connection, migration).map_err(failed);
    let restored = connection.pragma_update(None, "foreign_keys", true);
    result?;
    restored.map_err(DbError::from)
}

fn apply_in_transaction(
    connection: &mut Connection,
    migration: &Migration,
) -> rusqlite::Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(CREATE_TABLE)?;
    // Another process may have applied it while this one waited for the lock.
    if current_version(&transaction)? >= migration.version {
        return Ok(());
    }
    transaction.execute_batch(migration.sql)?;
    let violation = transaction
        .query_row(
            "SELECT \"table\" FROM pragma_foreign_key_check",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(table) = violation {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY),
            Some(format!("foreign key violation in table {table}")),
        ));
    }
    let applied_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        });
    transaction.execute(
        "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
        (migration.version, migration.name, applied_at),
    )?;
    transaction.commit()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use super::*;

    #[test]
    fn every_file_is_listed_once_in_order() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations");
        let files: BTreeSet<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".sql"))
            .collect();
        let listed: BTreeSet<String> = MIGRATIONS
            .iter()
            .map(|m| format!("{:04}_{}.sql", m.version, m.name))
            .collect();
        assert_eq!(files, listed);
        for (index, migration) in MIGRATIONS.iter().enumerate() {
            assert_eq!(migration.version as usize, index + 1);
        }
    }
}
