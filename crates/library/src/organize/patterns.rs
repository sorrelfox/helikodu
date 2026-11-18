// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The organizer's saved naming patterns.

use rusqlite::{Connection, params};

/// A saved naming pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPattern {
    /// Its row.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// The pattern.
    pub pattern: String,
}

pub(super) fn list(c: &Connection) -> rusqlite::Result<Vec<SavedPattern>> {
    c.prepare_cached("SELECT id, name, pattern FROM naming_patterns ORDER BY position, id")?
        .query_map([], |row| {
            Ok(SavedPattern {
                id: row.get(0)?,
                name: row.get(1)?,
                pattern: row.get(2)?,
            })
        })?
        .collect()
}

pub(super) fn save(
    c: &Connection,
    id: Option<i64>,
    name: &str,
    pattern: &str,
) -> rusqlite::Result<i64> {
    match id {
        Some(id) => {
            c.prepare_cached("UPDATE naming_patterns SET name = ?2, pattern = ?3 WHERE id = ?1")?
                .execute(params![id, name, pattern])?;
            Ok(id)
        }
        None => c
            .prepare_cached(
                "INSERT INTO naming_patterns (name, pattern, position)
                 VALUES (?1, ?2, coalesce((SELECT max(position) + 1 FROM naming_patterns), 0))
                 RETURNING id",
            )?
            .query_row(params![name, pattern], |row| row.get(0)),
    }
}

pub(super) fn delete(c: &Connection, id: i64) -> rusqlite::Result<()> {
    c.prepare_cached("DELETE FROM naming_patterns WHERE id = ?1")?
        .execute([id])?;
    Ok(())
}
