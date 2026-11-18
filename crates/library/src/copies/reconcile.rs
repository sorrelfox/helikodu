// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Confirming copies after a scan: files that share their size and quick hash are
//! hashed whole, byte-identical ones form groups, and the groups touched by the scan
//! are brought up to date.
//!
//! The hashing runs on the scan's thread (a few threads), never on the database
//! writer. A file is hashed only when another present file has its size and quick
//! hash, and again only when its stamp changed since.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::SystemTime;

use rusqlite::{Connection, params};

use super::groups::{self, Work, WorkResult};
use super::{backup, settings};
use crate::db::unix_time::to_unix_seconds;
use crate::db::{Database, DbError, path_from_blob};
use crate::scan::CancelToken;
use crate::scan::store::{STATE_COPY, STATE_READ};

/// How many groups one write refreshes.
const REFRESH_CHUNK: usize = 100;

/// A file that shares its size and quick hash with another.
#[derive(Debug, Clone)]
struct Candidate {
    id: i64,
    path: PathBuf,
    size: i64,
    quick_hash: Vec<u8>,
    /// The content hash, if it is valid for the file's stamp.
    hash: Option<Vec<u8>>,
    group: Option<i64>,
    group_hash: Option<Vec<u8>>,
}

/// The files of every size and quick hash that more than one present file has.
fn candidates(c: &Connection) -> rusqlite::Result<Vec<Candidate>> {
    let readable = format!("coalesce(f.scan_state, 0) IN ({STATE_READ}, {STATE_COPY})");
    let mut statement = c.prepare(&format!(
        "WITH sets AS (
             SELECT f.size, f.quick_hash FROM files f
             WHERE f.quick_hash IS NOT NULL AND length(f.quick_hash) > 0
               AND f.missing_since IS NULL AND {readable}
             GROUP BY f.size, f.quick_hash HAVING count(*) > 1)
         SELECT f.id, d.path, f.name, f.size, f.mtime_ns, f.quick_hash,
                CASE WHEN f.hashed_size IS f.size AND f.hashed_mtime_ns IS f.mtime_ns
                     THEN f.content_hash END,
                f.copy_group, g.content_hash
         FROM sets s
         JOIN files f ON f.size = s.size AND f.quick_hash = s.quick_hash
         JOIN dirs d ON d.id = f.dir_id
         LEFT JOIN copy_groups g ON g.id = f.copy_group
         WHERE f.missing_since IS NULL AND {readable}
         ORDER BY f.size, f.quick_hash, f.id"
    ))?;
    let rows = statement.query_map([], |row| {
        let dir = path_from_blob(row.get(1)?);
        let name = path_from_blob(row.get(2)?);
        Ok(Candidate {
            id: row.get(0)?,
            path: dir.join(name),
            size: row.get(3)?,
            quick_hash: row.get(5)?,
            hash: row.get(6)?,
            group: row.get(7)?,
            group_hash: row.get(8)?,
        })
    })?;
    rows.collect()
}

/// Whether the files of one size and quick hash are grouped as their hashes say:
/// all hashed, each hash shared by two files or more in one group of that hash, and
/// a file with a hash of its own in no group or in a group of its hash (whose other
/// files are missing).
fn settled(set: &[Candidate]) -> bool {
    let mut by_hash: BTreeMap<&[u8], Vec<&Candidate>> = BTreeMap::new();
    for f in set {
        let Some(hash) = f.hash.as_deref() else {
            return false;
        };
        if f.group.is_some() && f.group_hash.as_deref() != Some(hash) {
            return false;
        }
        by_hash.entry(hash).or_default().push(f);
    }
    by_hash.values().all(|files| {
        files.len() < 2
            || (files[0].group.is_some() && files.iter().all(|f| f.group == files[0].group))
    })
}

/// What [`reconcile`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Reconciled {
    /// Files that joined a group.
    pub(crate) joined: usize,
    /// Files whose tracks must now be read.
    pub(crate) needs_read: Vec<i64>,
    /// Why merging statistics had to wait, if it did: the backup failed.
    pub(crate) backup_failed: Option<String>,
}

/// Confirms copies and refreshes groups: the groups of `touched` files, and every
/// group with `all`. Hashes with up to `threads` threads.
pub(crate) fn reconcile(
    db: &Database,
    touched: &[i64],
    all: bool,
    threads: usize,
    cancel: &CancelToken,
) -> Result<Reconciled, DbError> {
    let found = db.read(candidates)?;
    let mut sets: Vec<Vec<Candidate>> = Vec::new();
    for f in found {
        match sets.last_mut() {
            Some(set) if set[0].size == f.size && set[0].quick_hash == f.quick_hash => {
                set.push(f);
            }
            _ => sets.push(vec![f]),
        }
    }
    sets.retain(|set| !settled(set));
    if sets.is_empty() && touched.is_empty() && !all {
        return Ok(Reconciled::default());
    }

    let to_hash: Vec<(i64, PathBuf)> = sets
        .iter()
        .flatten()
        .filter(|f| f.hash.is_none())
        .map(|f| (f.id, f.path.clone()))
        .collect();
    let paths: Vec<PathBuf> = to_hash.iter().map(|(_, p)| p.clone()).collect();
    let hashed = super::hash::hash_files(&paths, threads, cancel);
    if cancel.is_cancelled() {
        return Ok(Reconciled::default());
    }
    let hashes: Vec<(i64, super::hash::Hashed)> = to_hash
        .iter()
        .zip(hashed)
        .filter_map(|((id, _), h)| h.ok().map(|h| (*id, h)))
        .collect();
    let set_keys: Vec<(i64, Vec<u8>)> = sets
        .iter()
        .map(|s| (s[0].size, s[0].quick_hash.clone()))
        .collect();
    let touched = touched.to_vec();
    let now = to_unix_seconds(SystemTime::now());
    let writer = db.writer()?;
    let (joined, mut result, affected) = writer.write(move |c| {
        let mut work = Work::new(c, now)?;
        let (joined, affected) = regroup(c, &hashes, &set_keys, &touched, all, &mut work)?;
        Ok((joined, work.finish(c)?, affected))
    })?;
    // Groups are refreshed a bounded number per write: one transaction that removes
    // or moves the tracks of thousands of copies grows a statement journal that
    // makes every statement slower.
    for chunk in affected.chunks(REFRESH_CHUNK) {
        let chunk = chunk.to_vec();
        let part: WorkResult = writer.write(move |c| {
            let mut work = Work::new(c, now)?;
            for group in chunk {
                groups::refresh(c, group, &mut work)?;
            }
            work.finish(c)
        })?;
        result.needs_read.extend(part.needs_read);
        result.blocked.extend(part.blocked);
        result.merge_blocked |= part.merge_blocked;
    }
    let mut reconciled = Reconciled {
        joined,
        needs_read: result.needs_read.clone(),
        backup_failed: None,
    };
    if result.merge_blocked {
        match backup::ensure(db) {
            Ok(()) => {
                writer.write(move |c| {
                    settings::set_value(c, settings::BACKUP_KEY, &now.to_string())
                })?;
                let mut blocked = result.blocked;
                blocked.sort_unstable();
                blocked.dedup();
                for chunk in blocked.chunks(REFRESH_CHUNK) {
                    let chunk = chunk.to_vec();
                    let again: WorkResult = writer.write(move |c| {
                        let mut work = Work::new(c, now)?;
                        for group in chunk {
                            groups::refresh(c, group, &mut work)?;
                        }
                        work.finish(c)
                    })?;
                    reconciled.needs_read.extend(again.needs_read);
                }
            }
            Err(e) => reconciled.backup_failed = Some(e.to_string()),
        }
    }
    reconciled.needs_read.sort_unstable();
    reconciled.needs_read.dedup();
    Ok(reconciled)
}

/// Stores `hashes` and forms the groups of the sets `set_keys`; then refreshes the
/// groups of `touched` files, every group with `all`, and the groups left with
/// fewer than two files. Returns how many files joined a group, and the groups to
/// refresh.
fn regroup(
    c: &Connection,
    hashes: &[(i64, super::hash::Hashed)],
    set_keys: &[(i64, Vec<u8>)],
    touched: &[i64],
    all: bool,
    work: &mut Work,
) -> rusqlite::Result<(usize, Vec<i64>)> {
    let mut store = c.prepare_cached(
        "UPDATE files SET content_hash = ?2, hashed_size = ?3, hashed_mtime_ns = ?4,
                          dev = coalesce(dev, ?5)
         WHERE id = ?1 AND size = ?3 AND mtime_ns IS ?4",
    )?;
    for (id, h) in hashes {
        store.execute(params![
            id,
            h.hash,
            i64::try_from(h.size).unwrap_or(i64::MAX),
            h.mtime_ns,
            h.dev.cast_signed()
        ])?;
    }
    let mut affected: BTreeSet<i64> = BTreeSet::new();
    let mut joined = 0;
    let mut members = c.prepare_cached(&format!(
        "SELECT id, content_hash, copy_group FROM files
         WHERE size = ?1 AND quick_hash = ?2 AND missing_since IS NULL
           AND coalesce(scan_state, 0) IN ({STATE_READ}, {STATE_COPY})
           AND content_hash IS NOT NULL
           AND hashed_size IS size AND hashed_mtime_ns IS mtime_ns
         ORDER BY id"
    ))?;
    for (size, quick_hash) in set_keys {
        let files: Vec<(i64, Vec<u8>, Option<i64>)> = members
            .query_map(params![size, quick_hash], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut by_hash: BTreeMap<Vec<u8>, Vec<(i64, Option<i64>)>> = BTreeMap::new();
        for (id, hash, group) in files {
            by_hash.entry(hash).or_default().push((id, group));
        }
        for (hash, files) in by_hash {
            if files.len() < 2 {
                // Alone: it leaves a group of other content.
                for (id, group) in files {
                    if let Some(group) = group {
                        let key: Option<Vec<u8>> = c
                            .prepare_cached("SELECT content_hash FROM copy_groups WHERE id = ?1")?
                            .query_row([group], |row| row.get(0))?;
                        if key.as_deref() != Some(hash.as_slice()) {
                            groups::leave(c, id, work)?;
                        }
                    }
                }
                continue;
            }
            let group = groups::ensure_group(c, *size, &hash)?;
            for (id, current) in files {
                if current == Some(group) {
                    continue;
                }
                if current.is_some() {
                    groups::leave(c, id, work)?;
                }
                groups::join(c, id, group)?;
                joined += 1;
            }
            affected.insert(group);
        }
    }
    for &file in touched {
        if let Some(group) = groups::group_of(c, file)? {
            affected.insert(group);
        }
    }
    let small: Vec<i64> = if all {
        c.prepare_cached("SELECT id FROM copy_groups")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    } else {
        c.prepare_cached(
            "SELECT g.id FROM copy_groups g
             WHERE (SELECT count(*) FROM files f WHERE f.copy_group = g.id) < 2",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?
    };
    affected.extend(small);
    Ok((joined, affected.into_iter().collect()))
}
