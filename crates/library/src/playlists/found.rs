// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Playlist files found in the library folders.
//!
//! The folders are walked for `.m3u`, `.m3u8`, `.pls` and `.xspf` files, without
//! following symbolic links and leaving out hidden folders and what the scan leaves
//! out. A file not imported before becomes a new static playlist that remembers the
//! file; a file that changed since replaces the items of its playlist (an edit that
//! can be undone), unless the playlist is locked. A playlist whose file has gone stays.

use std::collections::HashMap;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use super::history::record;
use super::items::insert_rows;
use super::store::{create, source_blob, touch};
use super::{NewItem, PlaylistId};
use crate::db::path_from_blob;
use crate::scan::ExcludeRule;
use crate::scan::filter::matches_pattern;

/// The most folders a walk enters below a library folder.
const MAX_DEPTH: usize = 32;
/// The most playlist files a walk reports.
const MAX_FILES: usize = 10_000;

/// A playlist file found in a library folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoundPlaylist {
    pub(crate) path: PathBuf,
    pub(crate) mtime_ns: i64,
}

/// A playlist imported from a found file: the playlist, the file's modification
/// time then, and whether the playlist is locked.
pub(crate) type Imported = (PlaylistId, Option<i64>, bool);

/// What picking up the playlist files of the library folders did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoundReport {
    /// The files that became new playlists.
    pub imported: usize,
    /// The files that changed and replaced the items of their playlist.
    pub refreshed: usize,
    /// The files that could not be read, with why.
    pub failed: Vec<(PathBuf, String)>,
}

fn is_playlist_file(name: &[u8]) -> bool {
    let Some(dot) = name.iter().rposition(|&b| b == b'.') else {
        return false;
    };
    let extension = name[dot + 1..].to_ascii_lowercase();
    matches!(extension.as_slice(), b"m3u" | b"m3u8" | b"pls" | b"xspf")
}

fn excluded(path: &Path, name: &[u8], exclude: &[ExcludeRule]) -> bool {
    name.first() == Some(&b'.')
        || exclude.iter().any(|rule| match rule {
            ExcludeRule::Path(prefix) => path.starts_with(prefix),
            ExcludeRule::Name(pattern) => matches_pattern(pattern.as_bytes(), name),
        })
}

/// The playlist files in `roots`, by path.
pub(crate) fn find_playlist_files(
    roots: &[PathBuf],
    exclude: &[ExcludeRule],
) -> Vec<FoundPlaylist> {
    let mut found = Vec::new();
    let mut pending: Vec<(PathBuf, usize)> = roots.iter().map(|r| (r.clone(), 0)).collect();
    while let Some((folder, depth)) = pending.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if excluded(&path, name.as_bytes(), exclude) {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth < MAX_DEPTH {
                    pending.push((path, depth + 1));
                }
            } else if kind.is_file()
                && is_playlist_file(name.as_bytes())
                && let Ok(meta) = entry.metadata()
            {
                found.push(FoundPlaylist {
                    path,
                    mtime_ns: meta.mtime().saturating_mul(1_000_000_000) + meta.mtime_nsec(),
                });
                if found.len() >= MAX_FILES {
                    pending.clear();
                    break;
                }
            }
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// The playlists imported from found files: by file, the playlist, the file's
/// modification time then and whether the playlist is locked.
pub(crate) fn imported_sources(c: &Connection) -> rusqlite::Result<HashMap<PathBuf, Imported>> {
    c.prepare_cached(
        "SELECT source_path, id, source_mtime, coalesce(locked, 0) FROM playlists
         WHERE source_path IS NOT NULL",
    )?
    .query_map([], |row| {
        Ok((
            path_from_blob(row.get(0)?),
            (
                PlaylistId(row.get(1)?),
                row.get(2)?,
                row.get::<_, i64>(3)? != 0,
            ),
        ))
    })?
    .collect()
}

/// Stores the found file `file` with `items`: as a new playlist named `name`, or as
/// the new items of `existing`. Returns the playlist.
pub(crate) fn store_found(
    c: &Connection,
    file: &FoundPlaylist,
    name: &str,
    items: &[NewItem],
    existing: Option<PlaylistId>,
    now: i64,
) -> rusqlite::Result<PlaylistId> {
    let playlist = match existing {
        Some(playlist) => {
            record(c, playlist, now)?;
            c.prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1")?
                .execute([playlist.0])?;
            touch(c, playlist, now)?;
            playlist
        }
        None => create(c, name, now)?,
    };
    insert_rows(c, playlist, 0, items)?;
    c.prepare_cached("UPDATE playlists SET source_path = ?2, source_mtime = ?3 WHERE id = ?1")?
        .execute(params![playlist.0, source_blob(&file.path), file.mtime_ns])?;
    Ok(playlist)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_extensions_in_any_case() {
        for name in ["a.m3u", "b.M3U8", "c.pls", "d.XSPF"] {
            assert!(is_playlist_file(name.as_bytes()), "{name}");
        }
        for name in ["a.flac", "m3u", "a.m3u.bak", "a.cue"] {
            assert!(!is_playlist_file(name.as_bytes()), "{name}");
        }
    }
}
