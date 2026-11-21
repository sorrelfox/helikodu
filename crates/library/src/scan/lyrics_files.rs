// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether a lyrics file is beside each track: `tracks.lyrics_file`.
//!
//! A scan does not read unchanged files again, so a `.lrc` or `.txt` file added or
//! removed beside one would go unnoticed. After every scan the tracks of the
//! scanned parts are checked against the disk: each folder is listed once, a file
//! is looked at closer only when a name in its folder could be its lyrics file,
//! and only the rows that differ are written.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use helikodu_meta::lyrics::sidecar_paths;
use rusqlite::{Connection, params};

use super::scope::Scope;
use crate::catalog::RootId;
use crate::db::{Database, DbError, path_from_blob, path_to_blob};

/// A file with tracks, as the check sees it.
#[derive(Debug)]
struct FileRow {
    id: i64,
    dir: PathBuf,
    name: OsString,
    /// The flag of its tracks; `None` when they disagree.
    flag: Option<bool>,
}

/// The columns [`file_row`] reads, grouped by file.
const COLUMNS: &str = "f.id, d.path, f.name, min(t.lyrics_file), max(t.lyrics_file)
    FROM files f JOIN dirs d ON d.id = f.dir_id JOIN tracks t ON t.file_id = f.id";

fn file_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileRow> {
    let (low, high): (bool, bool) = (row.get(3)?, row.get(4)?);
    Ok(FileRow {
        id: row.get(0)?,
        dir: path_from_blob(row.get(1)?),
        name: path_from_blob(row.get(2)?).into_os_string(),
        flag: (low == high).then_some(low),
    })
}

/// The present files with tracks in `scope` of `root`.
fn scope_files(c: &Connection, root: RootId, scope: &Scope) -> rusqlite::Result<Vec<FileRow>> {
    let present = "f.missing_since IS NULL";
    match scope {
        Scope::Root => c
            .prepare_cached(&format!(
                "SELECT {COLUMNS} WHERE d.root_id = ?1 AND {present} GROUP BY f.id"
            ))?
            .query_map([root.0], file_row)?
            .collect(),
        Scope::Folder(dir) => c
            .prepare_cached(&format!(
                "SELECT {COLUMNS} WHERE d.root_id = ?1 AND d.path = ?2 AND {present}
                 GROUP BY f.id"
            ))?
            .query_map(params![root.0, path_to_blob(dir)], file_row)?
            .collect(),
        Scope::Subtree(dir) => {
            // The paths below `dir` sort between `dir/` and `dir0`, as '0' follows '/'.
            let exact = path_to_blob(dir);
            let mut below = exact.to_vec();
            below.push(b'/');
            let mut end = exact.to_vec();
            end.push(b'0');
            c.prepare_cached(&format!(
                "SELECT {COLUMNS} WHERE d.root_id = ?1
                     AND (d.path = ?2 OR (d.path >= ?3 AND d.path < ?4)) AND {present}
                 GROUP BY f.id"
            ))?
            .query_map(params![root.0, exact, below, end], file_row)?
            .collect()
        }
    }
}

/// The files `ids` that have tracks.
fn files_by_id(c: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<FileRow>> {
    let mut statement =
        c.prepare_cached(&format!("SELECT {COLUMNS} WHERE f.id = ?1 GROUP BY f.id"))?;
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        out.extend(
            statement
                .query_map([id], file_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Ok(out)
}

/// Whether `name` could be a lyrics file: ends in `.lrc` or `.txt`, in any case.
/// Gives its base name.
fn lyrics_stem(name: &OsString) -> Option<&[u8]> {
    let bytes = name.as_bytes();
    let dot = bytes.iter().rposition(|&b| b == b'.')?;
    let extension = &bytes[dot + 1..];
    (extension.eq_ignore_ascii_case(b"lrc") || extension.eq_ignore_ascii_case(b"txt"))
        .then(|| &bytes[..dot])
}

/// The file names in `dir` that could be lyrics files, by base name; `None` when
/// the folder cannot be listed.
fn lyrics_stems(dir: &Path) -> Option<HashSet<Vec<u8>>> {
    let entries = fs::read_dir(dir).ok()?;
    Some(
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| lyrics_stem(&entry.file_name()).map(<[u8]>::to_vec))
            .collect(),
    )
}

/// The files of `rows` whose flag is not what the disk says now, with the flag.
fn differing(rows: Vec<FileRow>) -> Vec<(i64, bool)> {
    let mut by_dir: HashMap<PathBuf, Vec<FileRow>> = HashMap::new();
    for row in rows {
        by_dir.entry(row.dir.clone()).or_default().push(row);
    }
    let mut out = Vec::new();
    for (dir, rows) in by_dir {
        // A folder that cannot be listed keeps what it had.
        let Some(stems) = lyrics_stems(&dir) else {
            continue;
        };
        for row in rows {
            let path = dir.join(&row.name);
            let stem = path.file_stem().map(OsStrExt::as_bytes);
            let has =
                stem.is_some_and(|stem| stems.contains(stem)) && !sidecar_paths(&path).is_empty();
            if row.flag != Some(has) {
                out.push((row.id, has));
            }
        }
    }
    out
}

/// Writes the flags `changes`; returns how many files changed.
fn write(db: &Database, changes: Vec<(i64, bool)>) -> Result<usize, DbError> {
    if changes.is_empty() {
        return Ok(0);
    }
    let count = changes.len();
    db.writer()?.write(move |c| {
        let mut update = c.prepare_cached(
            "UPDATE tracks SET lyrics_file = ?2 WHERE file_id = ?1 AND lyrics_file <> ?2",
        )?;
        for (id, has) in &changes {
            update.execute(params![id, has])?;
        }
        Ok(())
    })?;
    Ok(count)
}

/// Brings the flag of the tracks in `scopes` up to date with the disk.
pub(crate) fn refresh_scopes(db: &Database, scopes: &[(RootId, Scope)]) -> Result<usize, DbError> {
    let mut rows = Vec::new();
    for (root, scope) in scopes {
        rows.extend(db.read(|c| scope_files(c, *root, scope))?);
    }
    write(db, differing(rows))
}

/// Brings the flag of the tracks of the files `ids` up to date with the disk.
pub(crate) fn refresh_files(db: &Database, ids: &[i64]) -> Result<usize, DbError> {
    if ids.is_empty() {
        return Ok(0);
    }
    let rows = db.read(|c| files_by_id(c, ids))?;
    write(db, differing(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lyrics_names_give_their_base_name() {
        let stem = |name: &str| lyrics_stem(&OsString::from(name)).map(<[u8]>::to_vec);
        assert_eq!(stem("01 Song.lrc"), Some(b"01 Song".to_vec()));
        assert_eq!(stem("01 Song.TXT"), Some(b"01 Song".to_vec()));
        assert_eq!(stem("01 Song.flac"), None);
        assert_eq!(stem("lrc"), None);
    }
}
