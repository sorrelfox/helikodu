// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Missing files: the library files that are gone from their place, finding them
//! elsewhere, relinking them and removing them from the library.
//!
//! - Files under a library folder that is masked (a drive that is not connected)
//!   are hidden, not missing, and are not listed.
//! - A file is recognised by its size and its quick hash (the size and the first
//!   and last 64 KiB), as the scanner recognises a moved file, so a renamed file is
//!   found too. A file that is back at its own place is found there.
//! - Relinking moves the library row to the file's new place: the tracks keep
//!   their ids, identities, statistics, ratings and places in playlists. The new
//!   place must be in a library folder.
//! - Removing takes the file and its tracks out of the library; their identities
//!   and statistics stay, for when the music comes back.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use helikodu_model::library::TrackId;
use rusqlite::{Connection, OptionalExtension, params};

use crate::Library;
use crate::catalog::write::{file_at, move_file, remove_file};
use crate::catalog::{RootId, track_id_from_sql};
use crate::copies::FileId;
use crate::db::{DbError, path_from_blob};
use crate::scan::CancelToken;

/// A library file that is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingFile {
    /// The file.
    pub file: FileId,
    /// Its first track, if it has one.
    pub track: Option<TrackId>,
    /// The title of that track.
    pub title: Option<String>,
    /// Its artist as credited.
    pub artist: Option<String>,
    /// Where it was.
    pub path: PathBuf,
    /// The library folder it was in.
    pub root: PathBuf,
    /// Since when it is missing, in seconds since the Unix epoch.
    pub missing_since: i64,
    /// How many tracks it has.
    pub tracks: usize,
}

/// A missing file found at another place (or back at its own).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundFile {
    /// The missing file.
    pub file: FileId,
    /// Where it is now.
    pub at: PathBuf,
}

/// Why a missing file was not relinked.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RelinkProblem {
    /// It is no longer missing, or no longer in the library.
    NotMissing,
    /// The file found is gone or changed since it was found.
    Changed,
    /// The new place is not in a library folder.
    OutsideLibrary,
}

/// What happened to one file asked to be relinked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelinkOutcome {
    /// The file.
    pub file: FileId,
    /// Why it was not relinked, or `None` when it was.
    pub problem: Option<RelinkProblem>,
}

struct Wanted {
    file: i64,
    path: PathBuf,
    size: u64,
    hash: Vec<u8>,
}

fn missing_list(c: &Connection) -> rusqlite::Result<Vec<MissingFile>> {
    c.prepare(
        "SELECT f.id, d.path, f.name, r.path, f.missing_since,
                (SELECT t.id FROM tracks t WHERE t.file_id = f.id ORDER BY t.sub_index LIMIT 1),
                (SELECT t.title FROM tracks t WHERE t.file_id = f.id ORDER BY t.sub_index LIMIT 1),
                (SELECT t.artist FROM tracks t WHERE t.file_id = f.id ORDER BY t.sub_index LIMIT 1),
                (SELECT count(*) FROM tracks t WHERE t.file_id = f.id)
         FROM files f JOIN dirs d ON d.id = f.dir_id JOIN roots r ON r.id = d.root_id
         WHERE f.missing_since IS NOT NULL AND r.masked = 0
         ORDER BY f.missing_since, d.path, f.name",
    )?
    .query_map([], |row| {
        let dir = path_from_blob(row.get(1)?);
        Ok(MissingFile {
            file: FileId(row.get(0)?),
            path: dir.join(path_from_blob(row.get(2)?)),
            root: path_from_blob(row.get(3)?),
            missing_since: row.get(4)?,
            track: row.get::<_, Option<i64>>(5)?.map(track_id_from_sql),
            title: row.get(6)?,
            artist: row.get(7)?,
            tracks: usize::try_from(row.get::<_, i64>(8)?).unwrap_or(0),
        })
    })?
    .collect()
}

fn wanted(c: &Connection, files: &[FileId]) -> rusqlite::Result<Vec<Wanted>> {
    let mut statement = c.prepare_cached(
        "SELECT d.path, f.name, f.size, f.quick_hash FROM files f JOIN dirs d ON d.id = f.dir_id
         WHERE f.id = ?1 AND f.missing_since IS NOT NULL",
    )?;
    let mut out = Vec::new();
    for file in files {
        let found = statement
            .query_row([file.0], |row| {
                Ok((
                    path_from_blob(row.get(0)?).join(path_from_blob(row.get(1)?)),
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                ))
            })
            .optional()?;
        if let Some((path, Some(size), Some(hash))) = found
            && !hash.is_empty()
        {
            out.push(Wanted {
                file: file.0,
                path,
                size: u64::try_from(size).unwrap_or(0),
                hash,
            });
        }
    }
    Ok(out)
}

/// The quick hash of the file at `path` with `size`.
fn quick_hash_of(path: &Path, size: u64) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    helikodu_meta::worker::quick_hash(&mut file, size).ok()
}

/// The roots that are not masked.
fn roots(c: &Connection) -> rusqlite::Result<Vec<(i64, PathBuf)>> {
    c.prepare_cached("SELECT id, path FROM roots WHERE masked = 0")?
        .query_map([], |row| Ok((row.get(0)?, path_from_blob(row.get(1)?))))?
        .collect()
}

fn root_of(roots: &[(i64, PathBuf)], path: &Path) -> Option<i64> {
    roots
        .iter()
        .filter(|(_, root)| path.starts_with(root) && path != root)
        .max_by_key(|(_, root)| root.as_os_str().len())
        .map(|(id, _)| *id)
}

impl Library {
    /// The library files that are missing, oldest first, leaving out the folders
    /// of drives that are not connected.
    pub fn missing_files(&self) -> Result<Vec<MissingFile>, DbError> {
        self.database().read(missing_list)
    }

    /// Looks for the missing `files` in `folder` and the folders below it (in
    /// every library folder when `None`), and at their own places. A file is
    /// recognised by its size and quick hash; files the library already has at
    /// their place are not offered. Stops early when `cancel` is set.
    pub fn find_missing(
        &self,
        files: &[FileId],
        folder: Option<&Path>,
        cancel: &CancelToken,
    ) -> Result<Vec<FoundFile>, DbError> {
        let files = files.to_vec();
        let (wanted, roots) = self
            .database()
            .read(|c| Ok((wanted(c, &files)?, roots(c)?)))?;
        let mut found = Vec::new();
        let mut open: Vec<&Wanted> = Vec::new();
        // Back at their own place.
        for w in &wanted {
            let back = fs::symlink_metadata(&w.path)
                .is_ok_and(|m| m.is_file() && m.len() == w.size)
                && quick_hash_of(&w.path, w.size).as_deref() == Some(w.hash.as_slice());
            if back {
                found.push(FoundFile {
                    file: FileId(w.file),
                    at: w.path.clone(),
                });
            } else {
                open.push(w);
            }
        }
        if open.is_empty() {
            return Ok(found);
        }
        let mut by_size: HashMap<u64, Vec<&Wanted>> = HashMap::new();
        for w in &open {
            by_size.entry(w.size).or_default().push(w);
        }
        let mut matched: HashSet<i64> = HashSet::new();
        let folders: Vec<PathBuf> = match folder {
            Some(folder) => vec![folder.to_path_buf()],
            None => roots.iter().map(|(_, r)| r.clone()).collect(),
        };
        let mut pending = folders;
        let mut seen_dirs = HashSet::new();
        while let Some(dir) = pending.pop() {
            if cancel.is_cancelled() {
                break;
            }
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if kind.is_dir() {
                    if let Ok(m) = entry.metadata()
                        && seen_dirs.insert((m.dev(), m.ino()))
                    {
                        pending.push(path);
                    }
                    continue;
                }
                if !kind.is_file() {
                    continue;
                }
                let Ok(m) = entry.metadata() else { continue };
                let Some(candidates) = by_size.get(&m.len()) else {
                    continue;
                };
                if candidates.iter().all(|w| matched.contains(&w.file)) {
                    continue;
                }
                let Some(hash) = quick_hash_of(&path, m.len()) else {
                    continue;
                };
                let Some(w) = candidates
                    .iter()
                    .find(|w| !matched.contains(&w.file) && w.hash == hash)
                else {
                    continue;
                };
                // A file the library has at its place already is not a lost one.
                let at = path.clone();
                let known = self.database().read(|c| {
                    c.prepare_cached(
                        "SELECT f.missing_since IS NULL FROM files f JOIN dirs d ON d.id = f.dir_id
                         WHERE d.path = ?1 AND f.name = ?2",
                    )?
                    .query_row(
                        params![
                            crate::db::path_to_blob(at.parent().unwrap_or(Path::new(""))),
                            crate::db::path_to_blob(Path::new(at.file_name().unwrap_or_default()))
                        ],
                        |row| row.get::<_, bool>(0),
                    )
                    .optional()
                })?;
                if known == Some(true) {
                    continue;
                }
                matched.insert(w.file);
                found.push(FoundFile {
                    file: FileId(w.file),
                    at: path,
                });
            }
        }
        Ok(found)
    }

    /// Moves the library rows of the missing files to where they were found. Each
    /// file is checked again first: still missing, and the file found still has its
    /// size and quick hash.
    pub fn relink_missing(&self, found: &[FoundFile]) -> Result<Vec<RelinkOutcome>, DbError> {
        let mut outcomes = Vec::with_capacity(found.len());
        for item in found {
            let file = item.file;
            let (wanted, roots) = self
                .database()
                .read(|c| Ok((wanted(c, &[file])?, roots(c)?)))?;
            let Some(w) = wanted.into_iter().next() else {
                outcomes.push(RelinkOutcome {
                    file,
                    problem: Some(RelinkProblem::NotMissing),
                });
                continue;
            };
            let Some(root) = root_of(&roots, &item.at) else {
                outcomes.push(RelinkOutcome {
                    file,
                    problem: Some(RelinkProblem::OutsideLibrary),
                });
                continue;
            };
            let Ok(m) = fs::symlink_metadata(&item.at) else {
                outcomes.push(RelinkOutcome {
                    file,
                    problem: Some(RelinkProblem::Changed),
                });
                continue;
            };
            if !m.is_file()
                || m.len() != w.size
                || quick_hash_of(&item.at, w.size).as_deref() != Some(w.hash.as_slice())
            {
                outcomes.push(RelinkOutcome {
                    file,
                    problem: Some(RelinkProblem::Changed),
                });
                continue;
            }
            let (from, to) = (w.path.clone(), item.at.clone());
            let id = file.0;
            let mtime = m
                .mtime()
                .saturating_mul(1_000_000_000)
                .saturating_add(m.mtime_nsec());
            let (inode, dev) = (m.ino().cast_signed(), m.dev().cast_signed());
            let now = crate::db::unix_time::to_unix_seconds(std::time::SystemTime::now());
            self.database().writer()?.write(move |c| {
                if from != to {
                    if let Some(other) = file_at(c, &to)?
                        && other != id
                    {
                        remove_file(c, &to)?;
                    }
                    move_file(c, RootId(root), &from, &to)?;
                }
                c.prepare_cached(
                    "UPDATE files SET missing_since = NULL, mtime_ns = ?2, inode = ?3, dev = ?4
                     WHERE id = ?1",
                )?
                .execute(params![id, mtime, inode, dev])?;
                // A copy that is back may take its place in its group again.
                let mut work = crate::copies::groups::Work::new(c, now)?;
                crate::copies::groups::refresh_groups_of(c, &[id], &mut work)?;
                work.finish(c).map(drop)
            })?;
            outcomes.push(RelinkOutcome {
                file,
                problem: None,
            });
        }
        Ok(outcomes)
    }

    /// Removes the missing `files` and their tracks from the library; a file that
    /// is no longer missing stays. Their identities and statistics stay. Returns
    /// how many files went.
    pub fn remove_missing(&self, files: &[FileId]) -> Result<usize, DbError> {
        let files = files.to_vec();
        self.database().writer()?.write(move |c| {
            let mut removed = 0;
            for file in &files {
                let path: Option<PathBuf> = c
                    .prepare_cached(
                        "SELECT d.path, f.name FROM files f JOIN dirs d ON d.id = f.dir_id
                         WHERE f.id = ?1 AND f.missing_since IS NOT NULL",
                    )?
                    .query_row([file.0], |row| {
                        Ok(path_from_blob(row.get(0)?).join(path_from_blob(row.get(1)?)))
                    })
                    .optional()?;
                if let Some(path) = path {
                    remove_file(c, &path)?;
                    removed += 1;
                }
            }
            Ok(removed)
        })
    }
}
