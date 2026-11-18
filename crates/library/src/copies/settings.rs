// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! How the library treats copies, kept in `db_meta` so that every connection (the
//! scan's writes, the queries, other programs reading the database) follows it.

use helikodu_model::settings::{DuplicateScan, DuplicateSettings};
use rusqlite::{Connection, OptionalExtension, params};

const ON_SCAN_KEY: &str = "copies.on_scan";
const SHOW_ONE_KEY: &str = "copies.show_one";
/// Set once the backup before the first merge of copies' statistics exists.
pub(crate) const BACKUP_KEY: &str = "copies.backup";

fn value(c: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    c.prepare_cached("SELECT value FROM db_meta WHERE key = ?1")?
        .query_row([key], |row| row.get(0))
        .optional()
}

pub(crate) fn set_value(c: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO db_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )?
    .execute(params![key, value])?;
    Ok(())
}

/// What a scan does with copies.
pub(crate) fn on_scan(c: &Connection) -> rusqlite::Result<DuplicateScan> {
    Ok(match value(c, ON_SCAN_KEY)?.as_deref() {
        Some("skip") => DuplicateScan::Skip,
        _ => DuplicateScan::Add,
    })
}

/// Whether the queries show only the main copy of each group.
pub(crate) fn show_one(c: &Connection) -> rusqlite::Result<bool> {
    Ok(value(c, SHOW_ONE_KEY)?.as_deref() == Some("1"))
}

/// Both settings.
pub(crate) fn read(c: &Connection) -> rusqlite::Result<DuplicateSettings> {
    Ok(DuplicateSettings {
        on_scan: on_scan(c)?,
        show_one: show_one(c)?,
    })
}

/// Stores `settings`.
pub(crate) fn write(c: &Connection, settings: DuplicateSettings) -> rusqlite::Result<()> {
    let on_scan = match settings.on_scan {
        DuplicateScan::Skip => "skip",
        _ => "add",
    };
    set_value(c, ON_SCAN_KEY, on_scan)?;
    set_value(c, SHOW_ONE_KEY, if settings.show_one { "1" } else { "0" })
}

/// Whether the backup before the first merge has been made.
pub(crate) fn backup_made(c: &Connection) -> rusqlite::Result<bool> {
    Ok(value(c, BACKUP_KEY)?.is_some())
}

/// The condition on the tracks `t` that leaves out hidden copies when the queries
/// show one copy and the library has any; `None` when nothing is left out.
pub(crate) fn hidden_copies_condition(c: &Connection) -> rusqlite::Result<Option<&'static str>> {
    if !show_one(c)? {
        return Ok(None);
    }
    let any: bool = c
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM tracks WHERE is_copy = 1)")?
        .query_row([], |row| row.get(0))?;
    Ok(any.then_some("t.is_copy = 0"))
}
