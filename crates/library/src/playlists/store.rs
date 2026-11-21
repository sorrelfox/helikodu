// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `playlists` rows: creating, renaming, duplicating, moving, locking and deleting
//! playlists and folders, and listing them.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::history::record;
use super::items::{append_tracks, insert_rows, remove_items, track_rows};
use super::smart::{self, smart_track_ids};
use super::{
    FOLDER, NewItem, PlaylistId, PlaylistNode, PlaylistSummary, PlaylistType, Refusal, SMART,
    STATIC,
};
use helikodu_model::library::TrackId;

use crate::catalog::{Page, Paged, sql_track_id};
use crate::db::unix_time::{from_millis, from_unix_seconds};
use crate::db::{path_from_blob, path_to_blob};

/// The next position among the children of `parent` (the top for `None`).
fn next_position(c: &Connection, parent: Option<PlaylistId>) -> rusqlite::Result<i64> {
    c.prepare_cached("SELECT coalesce(max(position), -1) + 1 FROM playlists WHERE parent_id IS ?1")?
        .query_row([parent.map(|p| p.0)], |row| row.get(0))
}

/// A new, empty row of `kind` named `name`, after the others in `parent`.
pub(crate) fn create_row(
    c: &Connection,
    name: &str,
    kind: i64,
    parent: Option<PlaylistId>,
    query: Option<&str>,
    now: i64,
) -> rusqlite::Result<PlaylistId> {
    if let Some(parent) = parent {
        check_folder(c, parent)?;
    }
    let position = next_position(c, parent)?;
    c.prepare_cached(
        "INSERT INTO playlists (parent_id, name, kind, query, position, created_at, modified_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         RETURNING id",
    )?
    .query_row(
        params![parent.map(|p| p.0), name, kind, query, position, now],
        |row| row.get(0).map(PlaylistId),
    )
}

/// A new, empty static playlist named `name`, after the others at the top. `now` is
/// in seconds since the Unix epoch.
pub(crate) fn create(c: &Connection, name: &str, now: i64) -> rusqlite::Result<PlaylistId> {
    create_row(c, name, STATIC, None, None, now)
}

/// A new, empty folder named `name` in `parent`.
pub(crate) fn create_folder(
    c: &Connection,
    name: &str,
    parent: Option<PlaylistId>,
    now: i64,
) -> rusqlite::Result<PlaylistId> {
    create_row(c, name, FOLDER, parent, None, now)
}

/// A new static playlist named `name` holding `items`.
pub(crate) fn insert_imported(
    c: &Connection,
    name: &str,
    items: &[NewItem],
    now: i64,
) -> rusqlite::Result<PlaylistId> {
    let id = create(c, name, now)?;
    insert_rows(c, id, 0, items)?;
    Ok(id)
}

/// The kind of `playlist` and whether it is locked; refuses an unknown one.
pub(crate) fn kind_of(c: &Connection, playlist: PlaylistId) -> rusqlite::Result<(i64, bool)> {
    let found: Option<(i64, Option<i64>)> = c
        .prepare_cached("SELECT kind, locked FROM playlists WHERE id = ?1")?
        .query_row([playlist.0], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    match found {
        None => Refusal::UnknownPlaylist(playlist).raise(),
        Some((kind, locked)) => Ok((kind, locked.unwrap_or(0) != 0)),
    }
}

/// Fails unless `playlist` exists and is static.
pub(crate) fn check_static(c: &Connection, playlist: PlaylistId) -> rusqlite::Result<()> {
    match kind_of(c, playlist)? {
        (STATIC, _) => Ok(()),
        _ => Refusal::NotStatic(playlist).raise(),
    }
}

/// Fails unless `playlist` is static and not locked.
pub(crate) fn check_editable(c: &Connection, playlist: PlaylistId) -> rusqlite::Result<()> {
    match kind_of(c, playlist)? {
        (STATIC, false) => Ok(()),
        (STATIC, true) => Refusal::Locked(playlist).raise(),
        _ => Refusal::NotStatic(playlist).raise(),
    }
}

/// Fails if `playlist` is the built-in Favourites playlist.
fn check_not_builtin(c: &Connection, playlist: PlaylistId) -> rusqlite::Result<()> {
    let builtin: bool = c
        .prepare_cached("SELECT builtin != 0 FROM playlists WHERE id = ?1")?
        .query_row([playlist.0], |row| row.get(0))
        .optional()?
        .unwrap_or(false);
    if builtin {
        Refusal::Builtin(playlist).raise()
    } else {
        Ok(())
    }
}

/// The built-in Favourites playlist. A database that has lost it gets a new,
/// empty one.
pub(crate) fn favourites(c: &Connection) -> rusqlite::Result<PlaylistId> {
    let found: Option<i64> = c
        .prepare_cached("SELECT id FROM playlists WHERE builtin != 0")?
        .query_row([], |row| row.get(0))
        .optional()?;
    if let Some(id) = found {
        return Ok(PlaylistId(id));
    }
    c.prepare_cached(
        "INSERT INTO playlists (parent_id, name, kind, position, created_at, modified_at, builtin)
         VALUES (NULL, 'Favourites', ?1, -1, CAST(strftime('%s', 'now') AS INTEGER),
                 CAST(strftime('%s', 'now') AS INTEGER), 1)
         RETURNING id",
    )?
    .query_row([STATIC], |row| row.get(0).map(PlaylistId))
}

/// Whether `track` is in Favourites: an item of it stands for the track's
/// identity, so a copy of the same file counts too.
pub(crate) fn is_favourite(c: &Connection, track: TrackId) -> rusqlite::Result<bool> {
    c.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM playlist_items i
                        JOIN playlists p ON p.id = i.playlist_id AND p.builtin != 0
                        JOIN tracks t ON t.identity_id = i.identity_id
                        WHERE t.id = ?1)",
    )?
    .query_row([sql_track_id(track)], |row| row.get(0))
}

/// Which of `tracks` are in Favourites.
pub(crate) fn favourite_tracks(
    c: &Connection,
    tracks: &[TrackId],
) -> rusqlite::Result<Vec<TrackId>> {
    let mut found = Vec::new();
    for &track in tracks {
        if is_favourite(c, track)? {
            found.push(track);
        }
    }
    Ok(found)
}

/// Adds `track` to the end of Favourites, or takes every item of it out. Does
/// nothing when it already is, or is not, there. Returns whether it is a
/// favourite now. The change can be undone like any edit of a playlist.
pub(crate) fn set_favourite(
    c: &Connection,
    track: TrackId,
    favourite: bool,
    now: i64,
) -> rusqlite::Result<bool> {
    let list = favourites(c)?;
    if favourite {
        if !is_favourite(c, track)? {
            append_tracks(c, list, &[track], now)?;
        }
        return Ok(true);
    }
    let positions: Vec<usize> = c
        .prepare_cached(
            "SELECT i.position FROM playlist_items i
             JOIN tracks t ON t.identity_id = i.identity_id
             WHERE i.playlist_id = ?1 AND t.id = ?2
             ORDER BY i.position",
        )?
        .query_map(params![list.0, sql_track_id(track)], |row| {
            row.get::<_, i64>(0)
        })?
        .map(|p| p.map(|p| usize::try_from(p).unwrap_or(0)))
        .collect::<rusqlite::Result<_>>()?;
    if !positions.is_empty() {
        remove_items(c, list, &positions, now)?;
    }
    Ok(false)
}

/// Fails unless `folder` exists and is a folder.
fn check_folder(c: &Connection, folder: PlaylistId) -> rusqlite::Result<()> {
    match kind_of(c, folder)? {
        (FOLDER, _) => Ok(()),
        _ => Refusal::BadFolder(folder).raise(),
    }
}

/// Records that `playlist` changed at `now`.
pub(crate) fn touch(c: &Connection, playlist: PlaylistId, now: i64) -> rusqlite::Result<()> {
    c.prepare_cached("UPDATE playlists SET modified_at = ?2 WHERE id = ?1")?
        .execute(params![playlist.0, now])?;
    Ok(())
}

/// Renames `playlist`; the rename can be undone.
pub(crate) fn rename(
    c: &Connection,
    playlist: PlaylistId,
    name: &str,
    now: i64,
) -> rusqlite::Result<()> {
    let (kind, _) = kind_of(c, playlist)?;
    check_not_builtin(c, playlist)?;
    if kind != FOLDER {
        record(c, playlist, now)?;
    }
    c.prepare_cached("UPDATE playlists SET name = ?2, modified_at = ?3 WHERE id = ?1")?
        .execute(params![playlist.0, name, now])?;
    Ok(())
}

/// Deletes `playlist` and its items. The playlists and folders of a deleted folder
/// move up into its parent.
pub(crate) fn delete(c: &Connection, playlist: PlaylistId) -> rusqlite::Result<()> {
    let (kind, _) = kind_of(c, playlist)?;
    check_not_builtin(c, playlist)?;
    if kind == FOLDER {
        let parent: Option<i64> = c
            .prepare_cached("SELECT parent_id FROM playlists WHERE id = ?1")?
            .query_row([playlist.0], |row| row.get(0))?;
        let parent = parent.map(PlaylistId);
        let children: Vec<i64> = c
            .prepare_cached("SELECT id FROM playlists WHERE parent_id = ?1 ORDER BY position, id")?
            .query_map([playlist.0], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let first = next_position(c, parent)?;
        let mut set =
            c.prepare_cached("UPDATE playlists SET parent_id = ?2, position = ?3 WHERE id = ?1")?;
        for (position, child) in (first..).zip(children) {
            set.execute(params![child, parent.map(|p| p.0), position])?;
        }
        c.prepare_cached("UPDATE generators SET parent_id = ?2 WHERE parent_id = ?1")?
            .execute(params![playlist.0, parent.map(|p| p.0)])?;
    }
    c.prepare_cached("DELETE FROM playlists WHERE id = ?1")?
        .execute([playlist.0])?;
    Ok(())
}

/// A static copy of `playlist` named `name`, after it in its folder. A copy of a
/// smart playlist holds the tracks its query selects now.
pub(crate) fn duplicate(
    c: &Connection,
    playlist: PlaylistId,
    name: &str,
    now: i64,
) -> rusqlite::Result<PlaylistId> {
    let (kind, _) = kind_of(c, playlist)?;
    if kind == FOLDER {
        return Refusal::NotStatic(playlist).raise();
    }
    let parent: Option<i64> = c
        .prepare_cached("SELECT parent_id FROM playlists WHERE id = ?1")?
        .query_row([playlist.0], |row| row.get(0))?;
    let copy = create_row(c, name, STATIC, parent.map(PlaylistId), None, now)?;
    if kind == SMART {
        let tracks = smart_track_ids(c, playlist, now)?;
        insert_rows(c, copy, 0, &track_rows(c, &tracks)?)?;
        return Ok(copy);
    }
    c.prepare_cached(
        "INSERT INTO playlist_items (playlist_id, position, identity_id, ref_kind, missing_ref,
                                     title, artist, album, duration_ms)
         SELECT ?2, position, identity_id, ref_kind, missing_ref, title, artist, album, duration_ms
         FROM playlist_items WHERE playlist_id = ?1",
    )?
    .execute(params![playlist.0, copy.0])?;
    Ok(copy)
}

/// Locks `playlist` against changes of its items and query, or unlocks it.
pub(crate) fn set_locked(
    c: &Connection,
    playlist: PlaylistId,
    locked: bool,
) -> rusqlite::Result<()> {
    kind_of(c, playlist)?;
    if locked {
        check_not_builtin(c, playlist)?;
    }
    c.prepare_cached("UPDATE playlists SET locked = ?2 WHERE id = ?1")?
        .execute(params![playlist.0, i64::from(locked)])?;
    Ok(())
}

/// Shows `playlist` in groups by album, or not.
pub(crate) fn set_grouped(
    c: &Connection,
    playlist: PlaylistId,
    grouped: bool,
) -> rusqlite::Result<()> {
    kind_of(c, playlist)?;
    c.prepare_cached("UPDATE playlists SET grouped = ?2 WHERE id = ?1")?
        .execute(params![playlist.0, i64::from(grouped)])?;
    Ok(())
}

/// Moves `playlist` into `parent` (the top for `None`) at `position` among its
/// children, or after them. A folder cannot go into itself or a folder inside it.
pub(crate) fn move_to(
    c: &Connection,
    playlist: PlaylistId,
    parent: Option<PlaylistId>,
    position: Option<usize>,
) -> rusqlite::Result<()> {
    kind_of(c, playlist)?;
    check_not_builtin(c, playlist)?;
    if let Some(parent) = parent {
        check_folder(c, parent)?;
        // Up from `parent`: the playlist must not be met.
        let mut at = Some(parent);
        while let Some(folder) = at {
            if folder == playlist {
                return Refusal::BadFolder(parent).raise();
            }
            at = c
                .prepare_cached("SELECT parent_id FROM playlists WHERE id = ?1")?
                .query_row([folder.0], |row| row.get::<_, Option<i64>>(0))?
                .map(PlaylistId);
        }
    }
    let siblings: Vec<i64> = c
        .prepare_cached(
            "SELECT id FROM playlists WHERE parent_id IS ?1 AND id != ?2 ORDER BY position, id",
        )?
        .query_map(params![parent.map(|p| p.0), playlist.0], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let at = position.unwrap_or(siblings.len()).min(siblings.len());
    let mut order = siblings;
    order.insert(at, playlist.0);
    let mut set =
        c.prepare_cached("UPDATE playlists SET parent_id = ?2, position = ?3 WHERE id = ?1")?;
    for (index, id) in order.iter().enumerate() {
        set.execute(params![
            id,
            parent.map(|p| p.0),
            i64::try_from(index).unwrap_or(i64::MAX)
        ])?;
    }
    Ok(())
}

/// The columns of a [`PlaylistSummary`], from the playlists `p`.
const SUMMARY: &str = "p.id, p.name, p.created_at, p.modified_at,
    (SELECT count(*) FROM playlist_items i WHERE i.playlist_id = p.id),
    (SELECT count(*) FROM playlist_items i WHERE i.playlist_id = p.id
        AND NOT EXISTS (SELECT 1 FROM tracks t WHERE t.identity_id = i.identity_id)),
    (SELECT coalesce(sum(coalesce(
        (SELECT t.duration_ms FROM tracks t WHERE t.identity_id = i.identity_id
         ORDER BY t.id LIMIT 1),
        i.duration_ms)), 0)
     FROM playlist_items i WHERE i.playlist_id = p.id),
    p.kind, p.parent_id, p.locked, p.grouped, p.source_path, p.query,
    (SELECT count(*) FROM playlists c WHERE c.parent_id = p.id),
    EXISTS (SELECT 1 FROM playlist_history h WHERE h.playlist_id = p.id AND h.redo = 0),
    EXISTS (SELECT 1 FROM playlist_history h WHERE h.playlist_id = p.id AND h.redo = 1),
    p.builtin != 0";

/// A summary row, and the query text of a smart playlist.
fn summary(row: &Row<'_>) -> rusqlite::Result<(PlaylistSummary, Option<String>)> {
    let count = |n: i64| usize::try_from(n).unwrap_or(0);
    let kind = PlaylistType::from_sql(row.get(7)?);
    let items = match kind {
        PlaylistType::Folder => count(row.get(13)?),
        _ => count(row.get(4)?),
    };
    Ok((
        PlaylistSummary {
            id: PlaylistId(row.get(0)?),
            name: row.get(1)?,
            kind,
            parent: row.get::<_, Option<i64>>(8)?.map(PlaylistId),
            created: from_unix_seconds(row.get::<_, Option<i64>>(2)?.unwrap_or(0)),
            modified: from_unix_seconds(row.get::<_, Option<i64>>(3)?.unwrap_or(0)),
            items,
            dead: count(row.get(5)?),
            duration: from_millis(row.get(6)?),
            locked: row.get::<_, Option<i64>>(9)?.unwrap_or(0) != 0,
            grouped: row.get::<_, i64>(10)? != 0,
            source: row.get::<_, Option<Vec<u8>>>(11)?.map(path_from_blob),
            can_undo: row.get(14)?,
            can_redo: row.get(15)?,
            builtin: row.get(16)?,
        },
        row.get(12)?,
    ))
}

/// `summary` with the count and length of a smart playlist's tracks now.
fn finish(
    c: &Connection,
    (mut summary, query): (PlaylistSummary, Option<String>),
    now: i64,
) -> rusqlite::Result<PlaylistSummary> {
    if summary.kind == PlaylistType::Smart {
        let tracks = smart::tracks_of(c, query.as_deref().unwrap_or_default(), now)?;
        let (items, duration) = smart::totals(c, &tracks)?;
        summary.items = items;
        summary.duration = duration;
    }
    Ok(summary)
}

/// One page of the static playlists at the top, in their order.
pub(crate) fn playlists(
    c: &Connection,
    page: Page,
    now: i64,
) -> rusqlite::Result<Paged<PlaylistSummary>> {
    let (limit, offset) = page.sql();
    let rows = c
        .prepare_cached(&format!(
            "SELECT {SUMMARY} FROM playlists p
             WHERE p.kind = ?1 AND p.parent_id IS NULL
             ORDER BY p.builtin DESC, p.position, p.id LIMIT ?2 OFFSET ?3"
        ))?
        .query_map(params![STATIC, limit, offset], summary)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let items = rows
        .into_iter()
        .map(|row| finish(c, row, now))
        .collect::<rusqlite::Result<_>>()?;
    let total: i64 = c
        .prepare_cached("SELECT count(*) FROM playlists WHERE kind = ?1 AND parent_id IS NULL")?
        .query_row([STATIC], |row| row.get(0))?;
    Ok(Paged {
        items,
        total: usize::try_from(total).unwrap_or(0),
    })
}

/// The playlist or folder `playlist`, if there is one.
pub(crate) fn playlist(
    c: &Connection,
    playlist: PlaylistId,
    now: i64,
) -> rusqlite::Result<Option<PlaylistSummary>> {
    let row = c
        .prepare_cached(&format!(
            "SELECT {SUMMARY} FROM playlists p WHERE p.id = ?1"
        ))?
        .query_row([playlist.0], summary)
        .optional()?;
    row.map(|row| finish(c, row, now)).transpose()
}

/// Every playlist and folder, each folder followed by what is in it.
pub(crate) fn playlist_tree(c: &Connection, now: i64) -> rusqlite::Result<Vec<PlaylistNode>> {
    let rows = c
        .prepare_cached(&format!(
            "SELECT {SUMMARY} FROM playlists p ORDER BY p.builtin DESC, p.position, p.id"
        ))?
        .query_map([], summary)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut children: HashMap<Option<PlaylistId>, Vec<PlaylistSummary>> = HashMap::new();
    let known: Vec<PlaylistId> = rows.iter().map(|(s, _)| s.id).collect();
    for row in rows {
        let summary = finish(c, row, now)?;
        // A row whose folder is gone shows at the top rather than not at all.
        let parent = summary.parent.filter(|p| known.contains(p));
        children.entry(parent).or_default().push(summary);
    }
    let mut out = Vec::with_capacity(known.len());
    let mut stack: Vec<(PlaylistSummary, usize)> = children
        .remove(&None)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .map(|s| (s, 0))
        .collect();
    while let Some((summary, depth)) = stack.pop() {
        if let Some(inner) = children.remove(&Some(summary.id)) {
            stack.extend(inner.into_iter().rev().map(|s| (s, depth + 1)));
        }
        out.push(PlaylistNode { summary, depth });
    }
    Ok(out)
}

/// The path of a playlist file as stored.
pub(crate) fn source_blob(path: &std::path::Path) -> Vec<u8> {
    path_to_blob(path).to_vec()
}
