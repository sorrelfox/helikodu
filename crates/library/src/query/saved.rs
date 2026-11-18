// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Saved searches: named queries kept in the database, in the user's order.

use std::fmt;
use std::time::SystemTime;

use helikodu_model::query::Query;
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::parser::parse_lenient;
use super::syntax_error::SyntaxError;
use crate::db::unix_time::from_unix_seconds;

/// A saved search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SavedSearchId(pub(crate) i64);

impl SavedSearchId {
    /// The number of the saved search in the database.
    pub fn get(self) -> i64 {
        self.0
    }

    /// The saved search numbered `id`. It names a saved search only if the number came
    /// from this library.
    pub fn from_raw(id: i64) -> Self {
        Self(id)
    }
}

impl fmt::Display for SavedSearchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A saved search as the side column lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSearch {
    /// The saved search.
    pub id: SavedSearchId,
    /// Its name.
    pub name: String,
    /// The query text as stored.
    pub text: String,
    /// The query. If the text cannot be read (written by a later version), a plain
    /// search of its words, and `error` says why.
    pub query: Query,
    /// Why the stored text could not be read, if it could not.
    pub error: Option<SyntaxError>,
    /// Its place in the list, from 0.
    pub position: usize,
    /// When it was saved.
    pub created: SystemTime,
    /// When its name or query last changed.
    pub modified: SystemTime,
}

const COLUMNS: &str = "id, name, query, position, created_at, modified_at";

fn saved_search(row: &Row<'_>) -> rusqlite::Result<SavedSearch> {
    let text: String = row.get(2)?;
    let parsed = parse_lenient(&text);
    Ok(SavedSearch {
        id: SavedSearchId(row.get(0)?),
        name: row.get(1)?,
        text,
        query: parsed.query,
        error: parsed.error,
        position: usize::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
        created: from_unix_seconds(row.get(4)?),
        modified: from_unix_seconds(row.get(5)?),
    })
}

/// Every saved search, in order.
pub(crate) fn list(c: &Connection) -> rusqlite::Result<Vec<SavedSearch>> {
    c.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM saved_searches ORDER BY position, id"
    ))?
    .query_map([], saved_search)?
    .collect()
}

/// The saved search `id`, if there is one.
pub(crate) fn get(c: &Connection, id: SavedSearchId) -> rusqlite::Result<Option<SavedSearch>> {
    c.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM saved_searches WHERE id = ?1"
    ))?
    .query_row([id.0], saved_search)
    .optional()
}

/// Saves `text` as `name`, after the others.
pub(crate) fn create(
    c: &Connection,
    name: &str,
    text: &str,
    now: i64,
) -> rusqlite::Result<SavedSearchId> {
    c.prepare_cached(
        "INSERT INTO saved_searches (name, query, position, created_at, modified_at)
         VALUES (?1, ?2, (SELECT count(*) FROM saved_searches), ?3, ?3)",
    )?
    .execute(params![name, text, now])?;
    Ok(SavedSearchId(c.last_insert_rowid()))
}

/// Renames `id`; whether it exists.
pub(crate) fn rename(
    c: &Connection,
    id: SavedSearchId,
    name: &str,
    now: i64,
) -> rusqlite::Result<bool> {
    Ok(
        c.prepare_cached("UPDATE saved_searches SET name = ?2, modified_at = ?3 WHERE id = ?1")?
            .execute(params![id.0, name, now])?
            > 0,
    )
}

/// Gives `id` the query `text`; whether it exists.
pub(crate) fn set_query(
    c: &Connection,
    id: SavedSearchId,
    text: &str,
    now: i64,
) -> rusqlite::Result<bool> {
    Ok(
        c.prepare_cached("UPDATE saved_searches SET query = ?2, modified_at = ?3 WHERE id = ?1")?
            .execute(params![id.0, text, now])?
            > 0,
    )
}

fn position(c: &Connection, id: SavedSearchId) -> rusqlite::Result<Option<i64>> {
    c.prepare_cached("SELECT position FROM saved_searches WHERE id = ?1")?
        .query_row([id.0], |row| row.get(0))
        .optional()
}

/// Deletes `id` and closes the gap; whether it existed.
pub(crate) fn delete(c: &Connection, id: SavedSearchId) -> rusqlite::Result<bool> {
    let Some(old) = position(c, id)? else {
        return Ok(false);
    };
    c.prepare_cached("DELETE FROM saved_searches WHERE id = ?1")?
        .execute([id.0])?;
    c.prepare_cached("UPDATE saved_searches SET position = position - 1 WHERE position > ?1")?
        .execute([old])?;
    Ok(true)
}

/// Moves `id` to `to` (or the end), shifting the others; whether it exists.
pub(crate) fn move_to(c: &Connection, id: SavedSearchId, to: usize) -> rusqlite::Result<bool> {
    let Some(old) = position(c, id)? else {
        return Ok(false);
    };
    let count: i64 = c
        .prepare_cached("SELECT count(*) FROM saved_searches")?
        .query_row([], |row| row.get(0))?;
    let new = i64::try_from(to).unwrap_or(i64::MAX).min(count - 1);
    if new < old {
        c.prepare_cached(
            "UPDATE saved_searches SET position = position + 1
             WHERE position >= ?1 AND position < ?2",
        )?
        .execute([new, old])?;
    } else if new > old {
        c.prepare_cached(
            "UPDATE saved_searches SET position = position - 1
             WHERE position > ?1 AND position <= ?2",
        )?
        .execute([old, new])?;
    }
    c.prepare_cached("UPDATE saved_searches SET position = ?2 WHERE id = ?1")?
        .execute([id.0, new])?;
    Ok(true)
}
