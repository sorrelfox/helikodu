// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The state of the library folders: how they are watched, and whether they are
//! masked.
//!
//! A library folder that is gone (an unmounted or unplugged device) is masked by the
//! watcher. While it is masked and `mask_when_missing` is set (the default), the
//! browsing queries leave out its tracks; they stay in the library and come back when
//! the folder does.

use std::path::PathBuf;

use rusqlite::{Connection, params};

use super::ids::RootId;
use crate::db::path_from_blob;

/// A library folder and its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootState {
    /// The folder.
    pub id: RootId,
    /// Its path.
    pub path: PathBuf,
    /// Whether it is gone and masked.
    pub masked: bool,
    /// When it was masked, in seconds since the Unix epoch.
    pub masked_since: Option<i64>,
    /// Whether its tracks are hidden while it is masked. On by default.
    pub mask_when_missing: bool,
    /// Whether it is rescanned periodically rather than watched with inotify, as
    /// the watcher last decided.
    pub polled: bool,
}

/// The library folders whose tracks are hidden now.
fn hidden_roots(c: &Connection) -> rusqlite::Result<Vec<i64>> {
    c.prepare_cached("SELECT id FROM roots WHERE masked = 1 AND mask_when_missing = 1")?
        .query_map([], |row| row.get(0))?
        .collect()
}

/// The condition on the tracks `t` for a query that hides the tracks of masked
/// folders unless `include_masked`, and the hidden copies when the queries show one
/// copy of each group: `1` when nothing is hidden.
///
/// It looks up each track's folder (two lookups by primary key), which costs less
/// than a list of every hidden file when a large folder is masked.
pub(crate) fn visibility(c: &Connection, include_masked: bool) -> rusqlite::Result<String> {
    let copies = crate::copies::settings::hidden_copies_condition(c)?;
    let hidden = if include_masked {
        Vec::new()
    } else {
        hidden_roots(c)?
    };
    let masked = (!hidden.is_empty()).then(|| {
        let list: Vec<String> = hidden.iter().map(i64::to_string).collect();
        format!(
            "(SELECT d.root_id FROM files f JOIN dirs d ON d.id = f.dir_id WHERE f.id = t.file_id)
         NOT IN ({})",
            list.join(", ")
        )
    });
    Ok(match (masked, copies) {
        (None, None) => "1".to_owned(),
        (Some(masked), None) => masked,
        (None, Some(copies)) => copies.to_owned(),
        (Some(masked), Some(copies)) => format!("{masked} AND {copies}"),
    })
}

/// Masks the library folder `root` (since `now`), or unmasks it. Returns whether it
/// exists.
pub(crate) fn set_masked(
    c: &Connection,
    root: RootId,
    masked: bool,
    now: i64,
) -> rusqlite::Result<bool> {
    let changed = c
        .prepare_cached(
            "UPDATE roots SET masked = ?2,
                 masked_since = CASE WHEN ?2 THEN coalesce(masked_since, ?3) END
             WHERE id = ?1",
        )?
        .execute(params![root.0, masked, now])?;
    Ok(changed > 0)
}

/// Sets whether the tracks of `root` are hidden while it is masked. Returns whether
/// it exists.
pub(crate) fn set_mask_when_missing(
    c: &Connection,
    root: RootId,
    hide: bool,
) -> rusqlite::Result<bool> {
    let changed = c
        .prepare_cached("UPDATE roots SET mask_when_missing = ?2 WHERE id = ?1")?
        .execute(params![root.0, hide])?;
    Ok(changed > 0)
}

/// Records how `root` is watched: polled, or with inotify.
pub(crate) fn set_polled(c: &Connection, root: RootId, polled: bool) -> rusqlite::Result<()> {
    c.prepare_cached("UPDATE roots SET watch_mode = ?2 WHERE id = ?1")?
        .execute(params![root.0, i64::from(polled)])?;
    Ok(())
}

/// Every library folder, in path order.
pub(crate) fn root_states(c: &Connection) -> rusqlite::Result<Vec<RootState>> {
    let mut statement = c.prepare_cached(
        "SELECT id, path, masked, masked_since, mask_when_missing, watch_mode
         FROM roots ORDER BY path",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(RootState {
            id: RootId(row.get(0)?),
            path: path_from_blob(row.get(1)?),
            masked: row.get(2)?,
            masked_since: row.get(3)?,
            mask_when_missing: row.get(4)?,
            polled: row.get::<_, i64>(5)? != 0,
        })
    })?;
    rows.collect()
}
