// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing tracks: roots, folders and files, the track row, its identity, its people,
//! album and genres, and its rows in the search indexes.
//!
//! Every function runs on the writer's connection inside the writer's transaction.

use std::path::{Path, PathBuf};

use helikodu_model::library::TrackId;
use rusqlite::{Connection, OptionalExtension, params};

use super::albums::{Regroup, main_artist};
use super::covers;
use super::fingerprint::TrackFingerprint;
use super::folders::disc_folder;
use super::ids::{IdentityId, RootId, sql_track_id, track_id_from_sql};
use super::people::{artists_fold, write_genres, write_people};
use super::track_record::TrackRecord;
use crate::copies;
use crate::db::{display_text, path_from_blob, path_to_blob, unix_time};
use crate::search::{fold, index};
use crate::stats::store as stats_store;

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
/// under a new path. Regroups the albums it affects and updates the search indexes.
/// `now` is in seconds since the Unix epoch.
pub(crate) fn upsert_track(
    c: &Connection,
    record: &TrackRecord,
    now: i64,
) -> rusqlite::Result<TrackWrite> {
    let mut regroup = Regroup::default();
    let written = write_track(c, record, now, &mut regroup)?;
    finish_tracks(c, regroup, &[written])?;
    Ok(written.write)
}

/// Writes `records` like [`upsert_track`], regrouping the albums they affect once
/// for all of them.
pub(crate) fn upsert_tracks(
    c: &Connection,
    records: &[TrackRecord],
    now: i64,
) -> rusqlite::Result<Vec<TrackWrite>> {
    let mut regroup = Regroup::default();
    let mut written = Vec::with_capacity(records.len());
    for record in records {
        written.push(write_track(c, record, now, &mut regroup)?);
    }
    finish_tracks(c, regroup, &written)?;
    Ok(written.iter().map(|w| w.write).collect())
}

/// A track written by [`write_track`], before its albums are regrouped and its search
/// rows written.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Written {
    pub(crate) write: TrackWrite,
    file: i64,
    moved: bool,
}

/// Regroups the albums of `regroup` and writes the search rows of the tracks
/// `written` and of the tracks whose album changed.
pub(crate) fn finish_tracks(
    c: &Connection,
    regroup: Regroup,
    written: &[Written],
) -> rusqlite::Result<()> {
    let mut reindex = if regroup.is_empty() {
        Vec::new()
    } else {
        regroup.run(c)?
    };
    let mut files: Vec<i64> = Vec::new();
    for w in written {
        if w.moved {
            // The path changed for every track of the file.
            files.push(w.file);
        } else {
            reindex.push(sql_track_id(w.write.track));
        }
    }
    files.sort_unstable();
    files.dedup();
    for file in files {
        index::reindex_file(c, file)?;
    }
    reindex.sort_unstable();
    reindex.dedup();
    for track in reindex {
        index::reindex_track(c, track)?;
    }
    Ok(())
}

/// Writes the rows of `record` (file, track, identity, people, genres) and notes in
/// `regroup` what its albums depend on; [`finish_tracks`] does the rest.
pub(crate) fn write_track(
    c: &Connection,
    record: &TrackRecord,
    now: i64,
    regroup: &mut Regroup,
) -> rusqlite::Result<Written> {
    let (Some(dir), Some(name)) = (record.path.parent(), record.path.file_name()) else {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "not a file path: {}",
            record.path.display()
        )));
    };
    let dir_id = ensure_dir(c, record.root, dir)?;
    let (file_id, moved) = upsert_file(c, record, dir_id, Path::new(name), now)?;
    if moved {
        refresh_album_dir(c, record.root, file_id, dir, dir_id, regroup)?;
    }
    let fingerprint = TrackFingerprint::of(record);
    let folder = FolderFacts::of(c, record.root, dir, dir_id)?;

    let existing: Option<(i64, i64)> = c
        .prepare_cached("SELECT id, identity_id FROM tracks WHERE file_id = ?1 AND sub_index = ?2")?
        .query_row(params![file_id, record.sub_index], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    let (track, identity, change) = if let Some((track, identity)) = existing {
        regroup.add_track(c, track)?;
        c.prepare_cached("UPDATE identities SET content_hash = ?2, audio_key = ?3 WHERE id = ?1")?
            .execute(params![
                identity,
                fingerprint.content_hash,
                fingerprint.audio_key()
            ])?;
        write_track_columns(c, Some(track), file_id, identity, record, &folder, now)?;
        let change = if moved {
            TrackChange::Moved
        } else {
            TrackChange::Updated
        };
        (track, identity, change)
    } else {
        let section = (
            i64::from(record.sub_index),
            record
                .frames
                .and_then(|(start, _)| i64::try_from(start).ok()),
            record
                .frames
                .and_then(|(_, end)| end)
                .and_then(|end| i64::try_from(end).ok()),
        );
        // A copy's track shares the identity of the same track of its group.
        let shared = copies::groups::shared_identity(c, file_id, section)?;
        let returning = match shared {
            Some(identity) => Some((identity, TrackChange::Added)),
            None => returning_identity(c, &fingerprint)?.map(|i| (i, TrackChange::Returned)),
        };
        let (identity, change) = match returning {
            Some(found) => found,
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
        let track = write_track_columns(c, None, file_id, identity, record, &folder, now)?;
        (track, identity, change)
    };

    write_people(c, track, &record.tags, &record.catalog)?;
    write_genres(c, track, &record.tags.genres)?;
    if let Some(covers) = &record.covers {
        covers::write_track_covers(c, track, dir_id, &record.path, covers)?;
    }
    regroup.add_track(c, track)?;
    Ok(Written {
        write: TrackWrite {
            track: track_id_from_sql(track),
            identity: IdentityId(identity),
            change,
        },
        file: file_id,
        moved,
    })
}

/// What a track's folder says about its album.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FolderFacts {
    /// The album folder: the folder, or the one above a disc folder.
    album_dir: i64,
    /// The disc number in the folder's name.
    disc: Option<u32>,
    /// The disc title in the folder's name.
    disc_subtitle: Option<String>,
}

impl FolderFacts {
    /// The facts of the folder `dir` (row `dir_id`) in the library folder `root`.
    fn of(c: &Connection, root: RootId, dir: &Path, dir_id: i64) -> rusqlite::Result<Self> {
        let Some(disc) = dir
            .file_name()
            .and_then(|name| disc_folder(&display_text(name)))
        else {
            return Ok(Self {
                album_dir: dir_id,
                disc: None,
                disc_subtitle: None,
            });
        };
        let mut album_dir = dir_id;
        if disc.whole_name
            && let Some(parent) = dir.parent()
        {
            let root_path: Option<Vec<u8>> = c
                .prepare_cached("SELECT path FROM roots WHERE id = ?1")?
                .query_row([root.0], |row| row.get(0))
                .optional()?;
            // A disc folder that is itself the library folder is its own album folder.
            if root_path.is_some_and(|root| root != path_to_blob(dir)) {
                album_dir = ensure_dir(c, root, parent)?;
            }
        }
        Ok(Self {
            album_dir,
            disc: Some(disc.number),
            disc_subtitle: disc.subtitle,
        })
    }
}

/// Rewrites the album folder of every track of the file `file`, after it moved to
/// the folder `dir` (row `dir_id`) of `root`, noting the albums affected.
pub(crate) fn refresh_album_dir(
    c: &Connection,
    root: RootId,
    file: i64,
    dir: &Path,
    dir_id: i64,
    regroup: &mut Regroup,
) -> rusqlite::Result<()> {
    regroup.add_tracks_where(c, "file_id = ?1", file)?;
    let folder = FolderFacts::of(c, root, dir, dir_id)?;
    c.prepare_cached("UPDATE tracks SET album_dir_id = ?2 WHERE file_id = ?1")?
        .execute(params![file, folder.album_dir])?;
    regroup.add_tracks_where(c, "file_id = ?1", file)?;
    Ok(())
}

/// The file row for `record`: the one at its path, or a missing file with the same
/// size and quick hash that moved here, or a new one. Returns its id and whether it
/// moved.
fn upsert_file(
    c: &Connection,
    record: &TrackRecord,
    dir_id: i64,
    name: &Path,
    now: i64,
) -> rusqlite::Result<(i64, bool)> {
    let stamp = &record.file;
    let size = i64::try_from(stamp.size).unwrap_or(i64::MAX);
    let inode = stamp.inode.and_then(|inode| i64::try_from(inode).ok());
    let name_blob = path_to_blob(name);
    let name_text = display_text(name.as_os_str());
    // The row, its size, quick hash and group of copies.
    type Here = (i64, Option<i64>, Option<Vec<u8>>, Option<i64>);
    let here: Option<Here> = c
        .prepare_cached(
            "SELECT id, size, quick_hash, copy_group FROM files WHERE dir_id = ?1 AND name = ?2",
        )?
        .query_row(params![dir_id, name_blob], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?;
    if let Some((id, old_size, old_hash, group)) = here {
        // A copy whose content changed is no longer one.
        if group.is_some()
            && (old_size != Some(size) || old_hash.as_deref() != Some(stamp.quick_hash.as_slice()))
        {
            let mut work = copies::groups::Work::new(c, now)?;
            copies::groups::leave(c, id, &mut work)?;
            work.finish(c)?;
        }
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

/// Inserts the track row (`track` is `None`) or updates it, and returns its id. The
/// album is left to the regrouping.
fn write_track_columns(
    c: &Connection,
    track: Option<i64>,
    file_id: i64,
    identity: i64,
    record: &TrackRecord,
    folder: &FolderFacts,
    now: i64,
) -> rusqlite::Result<i64> {
    let tags = &record.tags;
    let more = &record.catalog;
    let audio = &record.audio;
    let gain = &record.replay_gain;
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
    let artist = more
        .artist_credit
        .clone()
        .or_else(|| (!tags.artists.is_empty()).then(|| tags.artists.join(", ")));
    let main = artist
        .as_deref()
        .map(|credit| main_artist(tags.artists.first().map_or(credit, String::as_str)))
        .map(str::to_owned);
    let album_artist = more
        .album_artist_credit
        .clone()
        .or_else(|| (!tags.album_artists.is_empty()).then(|| tags.album_artists.join(", ")));
    let album_fold = tags.album.as_deref().map(fold);
    let release_type = (!more.release_types.is_empty()).then(|| more.release_types.join("; "));
    let disc_no = tags.disc_no.or(folder.disc);
    let disc_subtitle = more
        .disc_subtitle
        .clone()
        .or_else(|| folder.disc_subtitle.clone());
    let values = params![
        file_id,
        record.sub_index,
        start,
        end,
        identity,
        record.kind.to_sql(),
        record.section.to_sql(),
        tags.title,
        title_sort,
        title_fold,
        artist,
        tags.track_no,
        more.track_total,
        disc_no,
        more.disc_total,
        disc_subtitle,
        tags.date,
        tags.year,
        more.original_year,
        duration_ms,
        audio.codec,
        audio.container,
        audio.sample_rate,
        audio.bit_depth,
        audio.channels,
        audio.bitrate,
        more.work,
        more.movement,
        more.movement_no,
        more.has_lyrics,
        tags.album,
        album_fold,
        more.album_sort,
        album_artist,
        more.album_artist_sort,
        main,
        more.compilation,
        release_type,
        more.release_mbid,
        more.release_group_mbid,
        folder.album_dir,
        artists_fold(tags),
        now,
        gain.track_gain_db,
        gain.track_peak,
        gain.album_gain_db,
        gain.album_peak,
    ];
    match track {
        None => c
            .prepare_cached(
                "INSERT INTO tracks (file_id, sub_index, start_frame, end_frame, identity_id,
                     kind, section, title, title_sort, title_fold, artist, track_no,
                     track_total, disc_no, disc_total, disc_subtitle, date, year,
                     original_year, duration_ms, codec, container, sample_rate, bit_depth,
                     channels, bitrate, work, movement, movement_no, has_lyrics, album_title,
                     album_fold, album_sort, album_artist, album_artist_sort, main_artist,
                     compilation, release_type, release_mbid, release_group_mbid,
                     album_dir_id, artists_fold, added_at, rg_track_gain, rg_track_peak,
                     rg_album_gain, rg_album_peak)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                         ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28,
                         ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40, ?41, ?42,
                         ?43, ?44, ?45, ?46, ?47)
                 RETURNING id",
            )?
            .query_row(values, |row| row.get(0)),
        Some(track) => {
            c.prepare_cached(
                "UPDATE tracks SET file_id = ?1, sub_index = ?2, start_frame = ?3,
                     end_frame = ?4, identity_id = ?5, kind = ?6, section = ?7, title = ?8,
                     title_sort = ?9, title_fold = ?10, artist = ?11, track_no = ?12,
                     track_total = ?13, disc_no = ?14, disc_total = ?15,
                     disc_subtitle = ?16, date = ?17, year = ?18, original_year = ?19,
                     duration_ms = ?20, codec = ?21, container = ?22, sample_rate = ?23,
                     bit_depth = ?24, channels = ?25, bitrate = ?26, work = ?27,
                     movement = ?28, movement_no = ?29, has_lyrics = ?30, album_title = ?31,
                     album_fold = ?32, album_sort = ?33, album_artist = ?34,
                     album_artist_sort = ?35, main_artist = ?36, compilation = ?37,
                     release_type = ?38, release_mbid = ?39, release_group_mbid = ?40,
                     album_dir_id = ?41, artists_fold = ?42, modified_at = ?43,
                     rg_track_gain = ?44, rg_track_peak = ?45, rg_album_gain = ?46,
                     rg_album_peak = ?47
                 WHERE id = ?48",
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

/// Moves the file at `from` to `to`, in the folder of `root`. Its tracks keep their ids
/// and identities. Returns whether there was a file at `from`.
pub(crate) fn move_file(
    c: &Connection,
    root: RootId,
    from: &Path,
    to: &Path,
) -> rusqlite::Result<bool> {
    let mut regroup = Regroup::default();
    let moved = move_one_file(c, root, from, to, &mut regroup)?;
    finish_tracks(c, regroup, &[])?;
    Ok(moved)
}

fn move_one_file(
    c: &Connection,
    root: RootId,
    from: &Path,
    to: &Path,
    regroup: &mut Regroup,
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
    refresh_album_dir(c, root, file, dir, dir_id, regroup)?;
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
    let mut regroup = Regroup::default();
    let mut moved = 0;
    for file in files {
        let Ok(relative) = file.strip_prefix(from) else {
            continue;
        };
        if move_one_file(c, root, &file, &to.join(relative), &mut regroup)? {
            moved += 1;
        }
    }
    finish_tracks(c, regroup, &[])?;
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
    // Another copy takes over from a missing main copy.
    let mut work = copies::groups::Work::new(c, since)?;
    copies::groups::refresh_groups_of(c, &[file], &mut work)?;
    work.finish(c)?;
    Ok(true)
}

/// Removes the file at `path` and its tracks. Their identities stay, with their
/// statistics, so the tracks get them back if they return. Returns the number of
/// tracks removed.
pub(crate) fn remove_file(c: &Connection, path: &Path) -> rusqlite::Result<usize> {
    let Some(file) = file_at(c, path)? else {
        return Ok(0);
    };
    // A main copy's tracks may move to the next copy first.
    let mut work = copies::groups::Work::new(c, 0)?;
    copies::groups::before_removal(c, &[file], &mut work)?;
    work.finish(c)?;
    let tracks: i64 = c
        .prepare_cached("SELECT count(*) FROM tracks WHERE file_id = ?1")?
        .query_row([file], |row| row.get(0))?;
    let mut regroup = Regroup::default();
    regroup.add_tracks_where(c, "file_id = ?1", file)?;
    // The tracks go with the file (ON DELETE CASCADE); triggers take them out of the
    // search indexes and remove the albums, people and genres no track has any more.
    c.prepare_cached("DELETE FROM files WHERE id = ?1")?
        .execute([file])?;
    finish_tracks(c, regroup, &[])?;
    Ok(usize::try_from(tracks).unwrap_or(0))
}

/// Removes `track`, and its file if no other track uses it. Its identity stays. Returns
/// whether the track existed.
pub(crate) fn remove_track(c: &Connection, track: TrackId) -> rusqlite::Result<bool> {
    let mut regroup = Regroup::default();
    regroup.add_tracks_where(c, "id = ?1", sql_track_id(track))?;
    let file: Option<i64> = c
        .prepare_cached("DELETE FROM tracks WHERE id = ?1 RETURNING file_id")?
        .query_row([sql_track_id(track)], |row| row.get(0))
        .optional()?;
    let Some(file) = file else {
        return Ok(false);
    };
    let unused: bool = c
        .prepare_cached("SELECT NOT EXISTS (SELECT 1 FROM tracks WHERE file_id = ?1)")?
        .query_row([file], |row| row.get(0))?;
    if unused {
        let mut work = copies::groups::Work::new(c, 0)?;
        copies::groups::before_removal(c, &[file], &mut work)?;
        work.finish(c)?;
    }
    c.prepare_cached(
        "DELETE FROM files WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM tracks WHERE file_id = ?1)",
    )?
    .execute([file])?;
    finish_tracks(c, regroup, &[])?;
    Ok(true)
}
