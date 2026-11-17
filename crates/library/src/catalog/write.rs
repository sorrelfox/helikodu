// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing tracks: roots, folders and files, the track row, its identity, its people,
//! album and genres, and its rows in the search indexes.
//!
//! Every function runs on the writer's connection inside the writer's transaction.

use std::path::{Path, PathBuf};

use helikodu_model::library::TrackId;
use rusqlite::{Connection, OptionalExtension, params};

use super::fingerprint::TrackFingerprint;
use super::ids::{IdentityId, RootId, sql_track_id, track_id_from_sql};
use super::track_record::{TrackRecord, TrackTags};
use crate::db::{display_text, path_from_blob, path_to_blob, unix_time};
use crate::search::{fold, index};
use crate::stats::store as stats_store;

/// The roles of `track_people.role`.
#[derive(Debug, Clone, Copy)]
enum Role {
    Artist = 0,
    AlbumArtist = 1,
    Composer = 2,
}

/// What writing a track did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackChange {
    /// A new track with a new identity.
    Added,
    /// A new track that got back the identity of a track that had left the library,
    /// with its statistics, rating and history.
    Returned,
    /// The track was already there; its tags and properties were written again.
    Updated,
    /// The track's file had disappeared and came back under this path; the track and
    /// its identity stay.
    Moved,
}

/// The result of writing a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackWrite {
    /// The track.
    pub track: TrackId,
    /// Its identity.
    pub identity: IdentityId,
    /// What happened.
    pub change: TrackChange,
}

/// Adds the library folder at `path`, or returns it if it is there already.
pub(crate) fn add_root(c: &Connection, path: &Path) -> rusqlite::Result<RootId> {
    c.prepare_cached(
        "INSERT INTO roots (path, path_text) VALUES (?1, ?2)
         ON CONFLICT (path) DO UPDATE SET path_text = excluded.path_text
         RETURNING id",
    )?
    .query_row(
        params![path_to_blob(path), display_text(path.as_os_str())],
        |row| row.get(0),
    )
    .map(RootId)
}

/// The folder row of `path`, created under `root` if it does not exist.
pub(crate) fn ensure_dir(c: &Connection, root: RootId, path: &Path) -> rusqlite::Result<i64> {
    let blob = path_to_blob(path);
    let found = c
        .prepare_cached("SELECT id FROM dirs WHERE path = ?1")?
        .query_row([blob], |row| row.get(0))
        .optional()?;
    if let Some(id) = found {
        return Ok(id);
    }
    c.prepare_cached(
        "INSERT INTO dirs (root_id, path, path_text) VALUES (?1, ?2, ?3) RETURNING id",
    )?
    .query_row(
        params![root.0, blob, display_text(path.as_os_str())],
        |row| row.get(0),
    )
}

/// The file row at `path`, if there is one.
pub(crate) fn file_at(c: &Connection, path: &Path) -> rusqlite::Result<Option<i64>> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(None);
    };
    c.prepare_cached(
        "SELECT f.id FROM files f JOIN dirs d ON d.id = f.dir_id
         WHERE d.path = ?1 AND f.name = ?2",
    )?
    .query_row(
        params![path_to_blob(dir), path_to_blob(Path::new(name))],
        |row| row.get(0),
    )
    .optional()
}

/// The track at `path` with the sub-track number `sub_index`.
pub(crate) fn track_at(
    c: &Connection,
    path: &Path,
    sub_index: u32,
) -> rusqlite::Result<Option<TrackId>> {
    let Some(file) = file_at(c, path)? else {
        return Ok(None);
    };
    c.prepare_cached("SELECT id FROM tracks WHERE file_id = ?1 AND sub_index = ?2")?
        .query_row(params![file, sub_index], |row| row.get(0))
        .optional()
        .map(|id| id.map(track_id_from_sql))
}

/// The identity of `track`.
pub(crate) fn identity_of(c: &Connection, track: TrackId) -> rusqlite::Result<Option<IdentityId>> {
    c.prepare_cached("SELECT identity_id FROM tracks WHERE id = ?1")?
        .query_row([sql_track_id(track)], |row| row.get(0))
        .optional()
        .map(|id| id.map(IdentityId))
}

/// The identities with `fingerprint`, newest first.
pub(crate) fn identities_with(
    c: &Connection,
    fingerprint: &TrackFingerprint,
) -> rusqlite::Result<Vec<IdentityId>> {
    c.prepare_cached(
        "SELECT id FROM identities WHERE content_hash = ?1 AND audio_key = ?2 ORDER BY id DESC",
    )?
    .query_map(
        params![fingerprint.content_hash, fingerprint.audio_key()],
        |row| row.get(0).map(IdentityId),
    )?
    .collect()
}

/// Writes `record`: adds the track, or updates it, or moves it if its file came back
/// under a new path. `now` is in seconds since the Unix epoch.
pub(crate) fn upsert_track(
    c: &Connection,
    record: &TrackRecord,
    now: i64,
) -> rusqlite::Result<TrackWrite> {
    let (Some(dir), Some(name)) = (record.path.parent(), record.path.file_name()) else {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "not a file path: {}",
            record.path.display()
        )));
    };
    let dir_id = ensure_dir(c, record.root, dir)?;
    let (file_id, moved) = upsert_file(c, record, dir_id, Path::new(name))?;
    let fingerprint = TrackFingerprint::of(record);

    let existing: Option<(i64, i64)> = c
        .prepare_cached("SELECT id, identity_id FROM tracks WHERE file_id = ?1 AND sub_index = ?2")?
        .query_row(params![file_id, record.sub_index], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    let (track, identity, change) = if let Some((track, identity)) = existing {
        c.prepare_cached("UPDATE identities SET content_hash = ?2, audio_key = ?3 WHERE id = ?1")?
            .execute(params![
                identity,
                fingerprint.content_hash,
                fingerprint.audio_key()
            ])?;
        write_track_columns(c, Some(track), file_id, identity, record, now)?;
        let change = if moved {
            TrackChange::Moved
        } else {
            TrackChange::Updated
        };
        (track, identity, change)
    } else {
        let (identity, change) = match returning_identity(c, &fingerprint)? {
            Some(identity) => (identity, TrackChange::Returned),
            None => {
                let identity = c
                    .prepare_cached(
                        "INSERT INTO identities (created_at, content_hash, audio_key)
                         VALUES (?1, ?2, ?3) RETURNING id",
                    )?
                    .query_row(
                        params![now, fingerprint.content_hash, fingerprint.audio_key()],
                        |row| row.get(0),
                    )?;
                if let Some(rating) = record.rating_from_tags {
                    stats_store::set_rating(c, IdentityId(identity), Some(rating))?;
                }
                (identity, TrackChange::Added)
            }
        };
        let track = write_track_columns(c, None, file_id, identity, record, now)?;
        (track, identity, change)
    };

    write_people(c, track, &record.tags)?;
    write_genres(c, track, &record.tags.genres)?;
    if moved {
        // The path changed for every track of the file.
        index::reindex_file(c, file_id)?;
    } else {
        index::reindex_track(c, track)?;
    }
    Ok(TrackWrite {
        track: track_id_from_sql(track),
        identity: IdentityId(identity),
        change,
    })
}

/// The file row for `record`: the one at its path, or a missing file with the same
/// size and quick hash that moved here, or a new one. Returns its id and whether it
/// moved.
fn upsert_file(
    c: &Connection,
    record: &TrackRecord,
    dir_id: i64,
    name: &Path,
) -> rusqlite::Result<(i64, bool)> {
    let stamp = &record.file;
    let size = i64::try_from(stamp.size).unwrap_or(i64::MAX);
    let inode = stamp.inode.and_then(|inode| i64::try_from(inode).ok());
    let name_blob = path_to_blob(name);
    let name_text = display_text(name.as_os_str());
    let here: Option<i64> = c
        .prepare_cached("SELECT id FROM files WHERE dir_id = ?1 AND name = ?2")?
        .query_row(params![dir_id, name_blob], |row| row.get(0))
        .optional()?;
    if let Some(id) = here {
        c.prepare_cached(
            "UPDATE files SET size = ?2, mtime_ns = ?3, inode = ?4, quick_hash = ?5,
                              missing_since = NULL
             WHERE id = ?1",
        )?
        .execute(params![id, size, stamp.mtime_ns, inode, stamp.quick_hash])?;
        return Ok((id, false));
    }
    let moved: Option<i64> = if stamp.quick_hash.is_empty() {
        None
    } else {
        c.prepare_cached(
            "SELECT id FROM files
             WHERE size = ?1 AND quick_hash = ?2 AND missing_since IS NOT NULL
             ORDER BY missing_since DESC LIMIT 1",
        )?
        .query_row(params![size, stamp.quick_hash], |row| row.get(0))
        .optional()?
    };
    if let Some(id) = moved {
        c.prepare_cached(
            "UPDATE files SET dir_id = ?2, name = ?3, name_text = ?4, mtime_ns = ?5,
                              inode = ?6, missing_since = NULL
             WHERE id = ?1",
        )?
        .execute(params![
            id,
            dir_id,
            name_blob,
            name_text,
            stamp.mtime_ns,
            inode
        ])?;
        return Ok((id, true));
    }
    let id = c
        .prepare_cached(
            "INSERT INTO files (dir_id, name, name_text, size, mtime_ns, inode, quick_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING id",
        )?
        .query_row(
            params![
                dir_id,
                name_blob,
                name_text,
                size,
                stamp.mtime_ns,
                inode,
                stamp.quick_hash
            ],
            |row| row.get(0),
        )?;
    Ok((id, false))
}

/// An identity with `fingerprint` that no track uses any more: the track left the
/// library and has come back.
fn returning_identity(
    c: &Connection,
    fingerprint: &TrackFingerprint,
) -> rusqlite::Result<Option<i64>> {
    if fingerprint.content_hash.is_empty() {
        return Ok(None);
    }
    c.prepare_cached(
        "SELECT i.id FROM identities i
         WHERE i.content_hash = ?1 AND i.audio_key = ?2
           AND NOT EXISTS (SELECT 1 FROM tracks t WHERE t.identity_id = i.id)
         ORDER BY i.id DESC LIMIT 1",
    )?
    .query_row(
        params![fingerprint.content_hash, fingerprint.audio_key()],
        |row| row.get(0),
    )
    .optional()
}

/// Inserts the track row (`track` is `None`) or updates it, and returns its id.
fn write_track_columns(
    c: &Connection,
    track: Option<i64>,
    file_id: i64,
    identity: i64,
    record: &TrackRecord,
    now: i64,
) -> rusqlite::Result<i64> {
    let tags = &record.tags;
    let audio = &record.audio;
    let album = write_album(c, tags)?;
    let title_sort = tags.title_sort.as_ref().or(tags.title.as_ref());
    let title_fold = tags.title.as_deref().map(fold);
    let (start, end) = match record.frames {
        Some((start, end)) => (
            i64::try_from(start).ok(),
            end.and_then(|end| i64::try_from(end).ok()),
        ),
        None => (None, None),
    };
    let duration_ms = audio.duration.map(unix_time::to_millis);
    let values = params![
        file_id,
        record.sub_index,
        start,
        end,
        identity,
        album,
        record.kind.to_sql(),
        tags.title,
        title_sort,
        title_fold,
        tags.track_no,
        tags.disc_no,
        tags.date,
        tags.year,
        duration_ms,
        audio.codec,
        audio.container,
        audio.sample_rate,
        audio.bit_depth,
        audio.channels,
        audio.bitrate,
        now,
    ];
    match track {
        None => c
            .prepare_cached(
                "INSERT INTO tracks (file_id, sub_index, start_frame, end_frame, identity_id,
                     album_id, kind, title, title_sort, title_fold, track_no, disc_no, date,
                     year, duration_ms, codec, container, sample_rate, bit_depth, channels,
                     bitrate, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                         ?16, ?17, ?18, ?19, ?20, ?21, ?22)
                 RETURNING id",
            )?
            .query_row(values, |row| row.get(0)),
        Some(track) => {
            c.prepare_cached(
                "UPDATE tracks SET file_id = ?1, sub_index = ?2, start_frame = ?3,
                     end_frame = ?4, identity_id = ?5, album_id = ?6, kind = ?7, title = ?8,
                     title_sort = ?9, title_fold = ?10, track_no = ?11, disc_no = ?12,
                     date = ?13, year = ?14, duration_ms = ?15, codec = ?16, container = ?17,
                     sample_rate = ?18, bit_depth = ?19, channels = ?20, bitrate = ?21,
                     modified_at = ?22
                 WHERE id = ?23",
            )?
            .execute(rusqlite::params_from_iter(
                values
                    .iter()
                    .copied()
                    .chain(std::iter::once(&track as &dyn rusqlite::ToSql)),
            ))?;
            Ok(track)
        }
    }
}

/// The album row for the tags, created if needed. `None` without an album title.
///
/// The key is provisional: the folded album artist (or first artist), title and year.
/// The album model with MusicBrainz IDs, compilations and the split by folder replaces
/// it.
fn write_album(c: &Connection, tags: &TrackTags) -> rusqlite::Result<Option<i64>> {
    let Some(title) = tags.album.as_deref() else {
        return Ok(None);
    };
    let artist = tags
        .album_artists
        .first()
        .or(tags.artists.first())
        .map_or(String::new(), |artist| fold(artist));
    let title_fold = fold(title);
    let year = tags.year.map_or(String::new(), |year| year.to_string());
    let key = format!("{artist}\u{1f}{title_fold}\u{1f}{year}");
    c.prepare_cached(
        "INSERT INTO albums (album_key, title, title_sort, title_fold, year)
         VALUES (?1, ?2, ?2, ?3, ?4)
         ON CONFLICT (album_key) DO UPDATE SET title = excluded.title
         RETURNING id",
    )?
    .query_row(params![key, title, title_fold, tags.year], |row| row.get(0))
    .map(Some)
}

/// Replaces the track's artists, album artists and composers, and the folded artist
/// names that search scans (`tracks.artists_fold`).
fn write_people(c: &Connection, track: i64, tags: &TrackTags) -> rusqlite::Result<()> {
    c.prepare_cached("DELETE FROM track_people WHERE track_id = ?1")?
        .execute([track])?;
    let mut insert = c.prepare_cached(
        "INSERT OR IGNORE INTO track_people (track_id, person_id, role, position)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for (role, names) in [
        (Role::Artist, &tags.artists),
        (Role::AlbumArtist, &tags.album_artists),
        (Role::Composer, &tags.composers),
    ] {
        for (position, name) in (0_i64..).zip(names) {
            let person = person(c, name)?;
            insert.execute(params![track, person, role as i64, position])?;
        }
    }
    let artists: Vec<String> = tags
        .artists
        .iter()
        .chain(&tags.album_artists)
        .map(|name| fold(name))
        .collect();
    let artists_fold = (!artists.is_empty()).then(|| artists.join("\u{1f}"));
    c.prepare_cached("UPDATE tracks SET artists_fold = ?2 WHERE id = ?1")?
        .execute(params![track, artists_fold])?;
    Ok(())
}

/// The person called `name`, created if needed.
fn person(c: &Connection, name: &str) -> rusqlite::Result<i64> {
    let folded = fold(name);
    let found = c
        .prepare_cached("SELECT id FROM people WHERE fold = ?1 AND name = ?2 LIMIT 1")?
        .query_row(params![folded, name], |row| row.get(0))
        .optional()?;
    match found {
        Some(id) => Ok(id),
        None => c
            .prepare_cached(
                "INSERT INTO people (name, sort_name, fold) VALUES (?1, ?1, ?2) RETURNING id",
            )?
            .query_row(params![name, folded], |row| row.get(0)),
    }
}

/// Replaces the track's genres.
fn write_genres(c: &Connection, track: i64, genres: &[String]) -> rusqlite::Result<()> {
    c.prepare_cached("DELETE FROM track_genres WHERE track_id = ?1")?
        .execute([track])?;
    let mut upsert = c.prepare_cached(
        "INSERT INTO genres (name, fold) VALUES (?1, ?2)
         ON CONFLICT (name) DO UPDATE SET fold = excluded.fold
         RETURNING id",
    )?;
    let mut insert = c.prepare_cached(
        "INSERT OR IGNORE INTO track_genres (track_id, genre_id, position) VALUES (?1, ?2, ?3)",
    )?;
    for (position, name) in (0_i64..).zip(genres) {
        let genre: i64 = upsert.query_row(params![name, fold(name)], |row| row.get(0))?;
        insert.execute(params![track, genre, position])?;
    }
    Ok(())
}

/// Moves the file at `from` to `to`, in the folder of `root`. Its tracks keep their ids
/// and identities. Returns whether there was a file at `from`.
pub(crate) fn move_file(
    c: &Connection,
    root: RootId,
    from: &Path,
    to: &Path,
) -> rusqlite::Result<bool> {
    let Some(file) = file_at(c, from)? else {
        return Ok(false);
    };
    let (Some(dir), Some(name)) = (to.parent(), to.file_name()) else {
        return Ok(false);
    };
    let dir_id = ensure_dir(c, root, dir)?;
    c.prepare_cached(
        "UPDATE files SET dir_id = ?2, name = ?3, name_text = ?4, missing_since = NULL
         WHERE id = ?1",
    )?
    .execute(params![
        file,
        dir_id,
        path_to_blob(Path::new(name)),
        display_text(name)
    ])?;
    index::reindex_file(c, file)?;
    Ok(true)
}

/// Moves every file in the folder `from` and the folders below it to the same place
/// below `to`, in the folder of `root`, as when a watched folder is renamed. Their
/// tracks keep their ids and identities. Returns the number of files moved.
pub(crate) fn move_folder(
    c: &Connection,
    root: RootId,
    from: &Path,
    to: &Path,
) -> rusqlite::Result<usize> {
    let exact = path_to_blob(from);
    // The paths below `from` sort between `from/` and `from0`, as '0' follows '/'.
    let mut below = exact.to_vec();
    below.push(b'/');
    let mut end = exact.to_vec();
    end.push(b'0');
    let files: Vec<PathBuf> = c
        .prepare_cached(
            "SELECT d.path, f.name FROM files f JOIN dirs d ON d.id = f.dir_id
             WHERE d.path = ?1 OR (d.path >= ?2 AND d.path < ?3)",
        )?
        .query_map(params![exact, below, end], |row| {
            Ok(path_from_blob(row.get(0)?).join(path_from_blob(row.get(1)?)))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut moved = 0;
    for file in files {
        let Ok(relative) = file.strip_prefix(from) else {
            continue;
        };
        if move_file(c, root, &file, &to.join(relative))? {
            moved += 1;
        }
    }
    Ok(moved)
}

/// Records that the file at `path` disappeared at `since` (seconds since the epoch).
/// Returns whether there was a file at `path`.
pub(crate) fn mark_file_missing(c: &Connection, path: &Path, since: i64) -> rusqlite::Result<bool> {
    let Some(file) = file_at(c, path)? else {
        return Ok(false);
    };
    c.prepare_cached("UPDATE files SET missing_since = ?2 WHERE id = ?1")?
        .execute(params![file, since])?;
    Ok(true)
}

/// Removes the file at `path` and its tracks. Their identities stay, with their
/// statistics, so the tracks get them back if they return. Returns the number of
/// tracks removed.
pub(crate) fn remove_file(c: &Connection, path: &Path) -> rusqlite::Result<usize> {
    let Some(file) = file_at(c, path)? else {
        return Ok(0);
    };
    let tracks: i64 = c
        .prepare_cached("SELECT count(*) FROM tracks WHERE file_id = ?1")?
        .query_row([file], |row| row.get(0))?;
    // The tracks go with the file (ON DELETE CASCADE), and a trigger takes them out of
    // the search indexes.
    c.prepare_cached("DELETE FROM files WHERE id = ?1")?
        .execute([file])?;
    Ok(usize::try_from(tracks).unwrap_or(0))
}

/// Removes `track`, and its file if no other track uses it. Its identity stays. Returns
/// whether the track existed.
pub(crate) fn remove_track(c: &Connection, track: TrackId) -> rusqlite::Result<bool> {
    let file: Option<i64> = c
        .prepare_cached("DELETE FROM tracks WHERE id = ?1 RETURNING file_id")?
        .query_row([sql_track_id(track)], |row| row.get(0))
        .optional()?;
    let Some(file) = file else {
        return Ok(false);
    };
    c.prepare_cached(
        "DELETE FROM files WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM tracks WHERE file_id = ?1)",
    )?
    .execute([file])?;
    Ok(true)
}
