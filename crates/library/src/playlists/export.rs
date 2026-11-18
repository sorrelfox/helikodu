// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Exporting a playlist into a file.
//!
//! A live item is written with its track's current path and tags; a dead one with the
//! path or URL and tags it was known by, unless dead items are left out. The file is
//! written under a temporary name in its folder, synced, renamed over the target and
//! the folder synced, so a crash leaves either the old file or the new one.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use helikodu_meta::playlists::{EntryHints, EntryLocation, Playlist, PlaylistEntry};
use rusqlite::{Connection, params};

use super::items::reference;
use super::store::check_static;
use super::{DeadReference, PlaylistId};
use crate::db::path_from_blob;
use crate::db::unix_time::from_millis;

/// The items of `playlist` as entries, and the number of dead items. Dead items are
/// left out with `skip_dead`, and always when nothing is known of where they were.
pub(crate) fn export_rows(
    c: &Connection,
    playlist: PlaylistId,
    skip_dead: bool,
) -> rusqlite::Result<(Playlist, usize)> {
    check_static(c, playlist)?;
    let name: String = c
        .prepare_cached("SELECT name FROM playlists WHERE id = ?1")?
        .query_row([playlist.0], |row| row.get(0))?;
    let mut query = c.prepare_cached(
        "SELECT i.ref_kind, i.missing_ref, i.title, i.artist, i.album, i.duration_ms,
                t.id, d.path, f.name, t.title, t.artist, a.title, t.duration_ms, t.track_no
         FROM playlist_items i
         LEFT JOIN tracks t ON t.id = (SELECT min(t2.id) FROM tracks t2
                                       WHERE t2.identity_id = i.identity_id)
         LEFT JOIN files f ON f.id = t.file_id
         LEFT JOIN dirs d ON d.id = f.dir_id
         LEFT JOIN albums a ON a.id = t.album_id
         WHERE i.playlist_id = ?1 ORDER BY i.position",
    )?;
    let mut rows = query.query(params![playlist.0])?;
    let mut entries = Vec::new();
    let mut dead = 0;
    while let Some(row) = rows.next()? {
        if row.get::<_, Option<i64>>(6)?.is_some() {
            let path = path_from_blob(row.get(7)?).join(path_from_blob(row.get(8)?));
            let mut entry = PlaylistEntry::local(path);
            entry.hints = EntryHints {
                title: row.get(9)?,
                artist: row.get(10)?,
                album: row.get(11)?,
                duration: row.get::<_, Option<i64>>(12)?.map(from_millis),
                track_number: row.get(13)?,
            };
            entries.push(entry);
            continue;
        }
        dead += 1;
        if skip_dead {
            continue;
        }
        let location = match reference(row.get(0)?, row.get(1)?) {
            DeadReference::Unknown => continue,
            DeadReference::Path(path) => EntryLocation::Local(path),
            DeadReference::Foreign(path) => EntryLocation::Foreign(path),
            DeadReference::Url(url) => EntryLocation::Remote(url),
        };
        let mut entry = PlaylistEntry::new(location);
        entry.hints = EntryHints {
            title: row.get(2)?,
            artist: row.get(3)?,
            album: row.get(4)?,
            duration: row.get::<_, Option<i64>>(5)?.map(from_millis),
            track_number: None,
        };
        entries.push(entry);
    }
    Ok((
        Playlist {
            title: Some(name),
            entries,
        },
        dead,
    ))
}

/// Writes `bytes` to `path` so that a crash leaves either the old file or the new one.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let folder = match path.parent() {
        Some(folder) if !folder.as_os_str().is_empty() => folder.to_owned(),
        _ => PathBuf::from("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a file path"))?;
    let mut temporary_name = b".".to_vec();
    temporary_name.extend_from_slice(name.as_bytes());
    temporary_name.extend_from_slice(
        format!(
            ".{}-{}.tmp",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )
        .as_bytes(),
    );
    let temporary = folder.join(std::ffi::OsStr::from_bytes(&temporary_name));
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        // The rename itself is durable once the folder is synced.
        File::open(&folder)?.sync_all()
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}
