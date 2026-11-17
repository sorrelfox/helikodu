// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the scanner reads from and writes to the database, beside the tracks.
//!
//! `files.scan_state` tells what a file row is:
//!
//! | Value | Meaning |
//! |---|---|
//! | `NULL`, 0 | read; its tracks are in `tracks` |
//! | 1 | unreadable; `scan_error` says why. Tracks it had before stay |
//! | 2 | a CUE sheet; it has no tracks of its own |

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use super::walk::Stamp;
use crate::catalog::write::{ensure_dir, file_at, upsert_track};
use crate::catalog::{RootId, TrackChange, TrackRecord};
use crate::db::{display_text, path_from_blob, path_to_blob};

/// `files.scan_state` of a file that was read.
pub(crate) const STATE_READ: i64 = 0;
/// `files.scan_state` of a file that could not be read.
pub(crate) const STATE_UNREADABLE: i64 = 1;
/// `files.scan_state` of a CUE sheet.
pub(crate) const STATE_CUE_SHEET: i64 = 2;

/// A file row of the library, as the scanner compares it with the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KnownFile {
    pub(crate) id: i64,
    pub(crate) stamp: Option<Stamp>,
    pub(crate) state: i64,
    pub(crate) missing: bool,
    pub(crate) has_tracks: bool,
}

/// Every file row in the folders of `root`, by path.
pub(crate) fn known_files(
    c: &Connection,
    root: RootId,
) -> rusqlite::Result<HashMap<PathBuf, KnownFile>> {
    let mut statement = c.prepare(
        "SELECT f.id, d.path, f.name, f.size, f.mtime_ns, f.inode, f.scan_state,
                f.missing_since IS NOT NULL,
                EXISTS (SELECT 1 FROM tracks t WHERE t.file_id = f.id)
         FROM files f JOIN dirs d ON d.id = f.dir_id
         WHERE d.root_id = ?1",
    )?;
    let rows = statement.query_map([root.0], |row| {
        let dir = path_from_blob(row.get(1)?);
        let name = path_from_blob(row.get(2)?);
        let size: Option<i64> = row.get(3)?;
        let mtime_ns: Option<i64> = row.get(4)?;
        let inode: Option<i64> = row.get(5)?;
        let stamp = match (size, mtime_ns, inode) {
            (Some(size), Some(mtime_ns), Some(inode)) => Some(Stamp {
                size: u64::try_from(size).unwrap_or(0),
                mtime_ns,
                inode: inode.cast_unsigned(),
            }),
            _ => None,
        };
        let state: Option<i64> = row.get(6)?;
        Ok((
            dir.join(name),
            KnownFile {
                id: row.get(0)?,
                stamp,
                state: state.unwrap_or(STATE_READ),
                missing: row.get(7)?,
                has_tracks: row.get(8)?,
            },
        ))
    })?;
    rows.collect()
}

/// Writes the tracks read from one file and removes the file's tracks that are no
/// longer in it (a CUE sheet that lost tracks, or a whole-file track replaced by a
/// sheet's tracks). Marks the file as read.
pub(crate) fn write_file_tracks(
    c: &Connection,
    records: &[TrackRecord],
    now: i64,
) -> rusqlite::Result<Vec<TrackChange>> {
    let mut changes = Vec::with_capacity(records.len());
    for record in records {
        changes.push(upsert_track(c, record, now)?.change);
    }
    let Some(path) = records.first().map(|r| r.path.as_path()) else {
        return Ok(changes);
    };
    let Some(file) = file_at(c, path)? else {
        return Ok(changes);
    };
    let stale: Vec<(i64, u32)> = c
        .prepare_cached("SELECT id, sub_index FROM tracks WHERE file_id = ?1")?
        .query_map([file], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut delete = c.prepare_cached("DELETE FROM tracks WHERE id = ?1")?;
    for (track, sub_index) in stale {
        if !records.iter().any(|r| r.sub_index == sub_index) {
            delete.execute([track])?;
        }
    }
    c.prepare_cached("UPDATE files SET scan_state = ?2, scan_error = NULL WHERE id = ?1")?
        .execute(params![file, STATE_READ])?;
    Ok(changes)
}

/// Records the file at `path` in `state` (unreadable, with `error`, or a CUE
/// sheet) without touching its tracks.
pub(crate) fn write_file_state(
    c: &Connection,
    root: RootId,
    path: &Path,
    stamp: Stamp,
    state: i64,
    error: Option<&str>,
) -> rusqlite::Result<()> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(());
    };
    let dir_id = ensure_dir(c, root, dir)?;
    c.prepare_cached(
        "INSERT INTO files (dir_id, name, name_text, size, mtime_ns, inode, scan_state,
                            scan_error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (dir_id, name) DO UPDATE SET
             size = excluded.size, mtime_ns = excluded.mtime_ns, inode = excluded.inode,
             scan_state = excluded.scan_state, scan_error = excluded.scan_error,
             missing_since = NULL",
    )?
    .execute(params![
        dir_id,
        path_to_blob(Path::new(name)),
        display_text(name),
        i64::try_from(stamp.size).unwrap_or(i64::MAX),
        stamp.mtime_ns,
        stamp.inode.cast_signed(),
        state,
        error,
    ])?;
    Ok(())
}

/// Marks the files `ids` as missing since `since` (seconds since the epoch), unless
/// they are already. Their tracks stay until the grace period ends.
pub(crate) fn mark_missing(c: &Connection, ids: &[i64], since: i64) -> rusqlite::Result<()> {
    let mut update = c.prepare_cached(
        "UPDATE files SET missing_since = ?2 WHERE id = ?1 AND missing_since IS NULL",
    )?;
    for id in ids {
        update.execute(params![id, since])?;
    }
    Ok(())
}

/// Clears the missing mark of the files `ids`, which are back where they were.
pub(crate) fn mark_present(c: &Connection, ids: &[i64]) -> rusqlite::Result<()> {
    let mut update = c.prepare_cached("UPDATE files SET missing_since = NULL WHERE id = ?1")?;
    for id in ids {
        update.execute([id])?;
    }
    Ok(())
}

/// Removes the file rows `ids` that have no tracks (CUE sheets, unreadable files)
/// and are gone from the disk.
pub(crate) fn forget_files(c: &Connection, ids: &[i64]) -> rusqlite::Result<()> {
    let mut delete = c.prepare_cached(
        "DELETE FROM files WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM tracks WHERE file_id = ?1)",
    )?;
    for id in ids {
        delete.execute([id])?;
    }
    Ok(())
}

/// The files recorded as unreadable, with the reason, in path order.
pub(crate) fn unreadable_files(c: &Connection) -> rusqlite::Result<Vec<(PathBuf, String)>> {
    let mut statement = c.prepare(
        "SELECT d.path, f.name, f.scan_error FROM files f JOIN dirs d ON d.id = f.dir_id
         WHERE f.scan_state = ?1 ORDER BY d.path, f.name",
    )?;
    let rows = statement.query_map([STATE_UNREADABLE], |row| {
        let dir = path_from_blob(row.get(0)?);
        let name = path_from_blob(row.get(1)?);
        let error: Option<String> = row.get(2)?;
        Ok((dir.join(name), error.unwrap_or_default()))
    })?;
    rows.collect()
}
