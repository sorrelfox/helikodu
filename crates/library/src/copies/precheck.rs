// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! While copies are skipped, new files that are copies of a known file, or of
//! another new file, are recognised before the workers read them: they share the
//! size and quick hash of another file and their whole bytes hash the same. Only one
//! file of each such set is read; the others are recorded as copies without tracks,
//! after the reads, and the copies pass groups them.
//!
//! A file whose size and quick hash match a missing file is left to the workers, so
//! that a moved file keeps its tracks.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use rusqlite::{Connection, params};

use super::hash::{self, Hashed};
use crate::catalog::RootId;
use crate::catalog::write::ensure_dir;
use crate::db::{Database, DbError, display_text, path_to_blob};
use crate::scan::CancelToken;
use crate::scan::compare::ToRead;
use crate::scan::filter::FileClass;
use crate::scan::store::{STATE_COPY, STATE_READ};

/// A new file recognised as a copy, to record without tracks.
#[derive(Debug, Clone)]
pub(crate) struct SetAside {
    root: RootId,
    path: PathBuf,
    quick_hash: Vec<u8>,
    hashed: Hashed,
}

/// Takes the new files that are copies out of `to_read` and returns them.
pub(crate) fn set_aside(
    db: &Database,
    to_read: &mut Vec<ToRead>,
    threads: usize,
    cancel: &CancelToken,
) -> Result<Vec<SetAside>, DbError> {
    let known_sizes: HashSet<i64> = db.read(|c| {
        c.prepare(&format!(
            "SELECT DISTINCT size FROM files WHERE quick_hash IS NOT NULL
             AND missing_since IS NULL AND coalesce(scan_state, 0) IN ({STATE_READ}, {STATE_COPY})"
        ))?
        .query_map([], |row| row.get(0))?
        .collect()
    })?;
    let size = |item: &ToRead| i64::try_from(item.found.stamp.size).unwrap_or(i64::MAX);
    let mut new_sizes: HashMap<i64, usize> = HashMap::new();
    for item in to_read.iter().filter(|i| !i.known) {
        *new_sizes.entry(size(item)).or_insert(0) += 1;
    }
    let interest: Vec<usize> = to_read
        .iter()
        .enumerate()
        .filter(|(_, i)| !i.known && i.found.class != FileClass::CueSheet)
        .filter(|(_, i)| {
            let s = size(i);
            known_sizes.contains(&s) || new_sizes.get(&s).copied().unwrap_or(0) > 1
        })
        .map(|(n, _)| n)
        .collect();
    if interest.is_empty() {
        return Ok(Vec::new());
    }
    let paths: Vec<PathBuf> = interest
        .iter()
        .map(|&n| to_read[n].found.path.clone())
        .collect();
    let quick = parallel(&paths, threads, cancel, |path| {
        let mut file = File::open(path).ok()?;
        let size = file.metadata().ok()?.len();
        helikodu_meta::worker::quick_hash(&mut file, size).ok()
    });
    let mut by_key: BTreeMap<(i64, Vec<u8>), Vec<usize>> = BTreeMap::new();
    for (&n, q) in interest.iter().zip(quick) {
        if let Some(q) = q {
            by_key.entry((size(&to_read[n]), q)).or_default().push(n);
        }
    }
    // For each key, the known files that have it: present ones with their paths, and
    // whether a missing one has it (then it is a move, left to the workers).
    let mut known: HashMap<(i64, Vec<u8>), Vec<PathBuf>> = HashMap::new();
    let keys: Vec<(i64, Vec<u8>)> = by_key.keys().cloned().collect();
    let moved: HashSet<(i64, Vec<u8>)> = db.read(|c| {
        let mut moved = HashSet::new();
        for key in &keys {
            if !known_sizes.contains(&key.0) {
                continue;
            }
            let (present, missing) = known_files(c, key.0, &key.1)?;
            if missing {
                moved.insert(key.clone());
            }
            if !present.is_empty() {
                known.insert(key.clone(), present);
            }
        }
        Ok(moved)
    })?;
    by_key
        .retain(|key, items| !moved.contains(key) && (items.len() > 1 || known.contains_key(key)));
    if by_key.is_empty() || cancel.is_cancelled() {
        return Ok(Vec::new());
    }
    // Hash the new files of each key and the known ones that are compared with them.
    let mut to_hash: Vec<PathBuf> = Vec::new();
    for (key, items) in &by_key {
        to_hash.extend(items.iter().map(|&n| to_read[n].found.path.clone()));
        if let Some(paths) = known.get(key) {
            to_hash.extend(paths.iter().cloned());
        }
    }
    let hashed: HashMap<PathBuf, Hashed> = to_hash
        .iter()
        .cloned()
        .zip(hash::hash_files(&to_hash, threads, cancel))
        .filter_map(|(p, h)| h.ok().map(|h| (p, h)))
        .collect();
    if cancel.is_cancelled() {
        return Ok(Vec::new());
    }
    let mut aside: Vec<usize> = Vec::new();
    let mut out = Vec::new();
    for (key, items) in &by_key {
        let known_hashes: HashSet<&[u8]> = known
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(|p| hashed.get(p).map(|h| h.hash.as_slice()))
            .collect();
        let mut by_hash: BTreeMap<&[u8], Vec<usize>> = BTreeMap::new();
        for &n in items {
            if let Some(h) = hashed.get(&to_read[n].found.path) {
                by_hash.entry(h.hash.as_slice()).or_default().push(n);
            }
        }
        for (hash, mut set) in by_hash {
            if !known_hashes.contains(hash) {
                if set.len() < 2 {
                    continue;
                }
                // The one the main-copy rule picks among files added together is read.
                set.sort_by(|&a, &b| {
                    let (a, b) = (&to_read[a].found.path, &to_read[b].found.path);
                    let (a, b) = (path_to_blob(a), path_to_blob(b));
                    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
                });
                set.remove(0);
            }
            for n in set {
                let item = &to_read[n];
                if let Some(h) = hashed.get(&item.found.path) {
                    out.push(SetAside {
                        root: item.root,
                        path: item.found.path.clone(),
                        quick_hash: key.1.clone(),
                        hashed: h.clone(),
                    });
                    aside.push(n);
                }
            }
        }
    }
    aside.sort_unstable();
    for n in aside.into_iter().rev() {
        to_read.remove(n);
    }
    Ok(out)
}

/// The present files with `size` and `quick_hash` (their paths), and whether a
/// missing one has them.
fn known_files(
    c: &Connection,
    size: i64,
    quick_hash: &[u8],
) -> rusqlite::Result<(Vec<PathBuf>, bool)> {
    let mut present = Vec::new();
    let mut missing = false;
    let mut statement = c.prepare_cached(&format!(
        "SELECT d.path, f.name, f.missing_since IS NOT NULL
         FROM files f JOIN dirs d ON d.id = f.dir_id
         WHERE f.size = ?1 AND f.quick_hash = ?2
           AND coalesce(f.scan_state, 0) IN ({STATE_READ}, {STATE_COPY})"
    ))?;
    let rows = statement.query_map(params![size, quick_hash], |row| {
        let dir = crate::db::path_from_blob(row.get(0)?);
        let name = crate::db::path_from_blob(row.get(1)?);
        Ok((dir.join(name), row.get::<_, bool>(2)?))
    })?;
    for row in rows {
        let (path, gone) = row?;
        if gone {
            missing = true;
        } else {
            present.push(path);
        }
    }
    Ok((present, missing))
}

/// Records the files set aside as copies without tracks, with their hashes, for
/// the copies pass to group.
pub(crate) fn record(c: &Connection, files: &[SetAside]) -> rusqlite::Result<()> {
    for f in files {
        let (Some(dir), Some(name)) = (f.path.parent(), f.path.file_name()) else {
            continue;
        };
        let dir_id = ensure_dir(c, f.root, dir)?;
        let size = i64::try_from(f.hashed.size).unwrap_or(i64::MAX);
        c.prepare_cached(
            "INSERT INTO files (dir_id, name, name_text, size, mtime_ns, inode, dev, quick_hash,
                                scan_state, content_hash, hashed_size, hashed_mtime_ns)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?4, ?5)
             ON CONFLICT (dir_id, name) DO UPDATE SET
                 size = excluded.size, mtime_ns = excluded.mtime_ns, inode = excluded.inode,
                 dev = excluded.dev, quick_hash = excluded.quick_hash,
                 scan_state = excluded.scan_state, content_hash = excluded.content_hash,
                 hashed_size = excluded.hashed_size, hashed_mtime_ns = excluded.hashed_mtime_ns,
                 missing_since = NULL",
        )?
        .execute(params![
            dir_id,
            path_to_blob(Path::new(name)),
            display_text(name),
            size,
            f.hashed.mtime_ns,
            f.hashed.inode.cast_signed(),
            f.hashed.dev.cast_signed(),
            f.quick_hash,
            STATE_COPY,
            f.hashed.hash,
        ])?;
    }
    Ok(())
}

/// Runs `work` on each of `paths` on up to `threads` threads, the results in order.
fn parallel<T: Send>(
    paths: &[PathBuf],
    threads: usize,
    cancel: &CancelToken,
    work: impl Fn(&Path) -> Option<T> + Sync,
) -> Vec<Option<T>> {
    let results: Vec<Mutex<Option<T>>> = paths.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..threads.clamp(1, 8) {
            scope.spawn(|| {
                while !cancel.is_cancelled() {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index) else {
                        break;
                    };
                    let result = work(path);
                    if let Ok(mut slot) = results[index].lock() {
                        *slot = result;
                    }
                }
            });
        }
    });
    results
        .into_iter()
        .map(|slot| slot.into_inner().ok().flatten())
        .collect()
}
