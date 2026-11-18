// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `playlists` rows: creating, renaming, duplicating, deleting and listing.

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::items::insert_rows;
use super::{NewItem, PlaylistId, PlaylistSummary, Refusal, STATIC};
use crate::catalog::{Page, Paged};
use crate::db::unix_time::{from_millis, from_unix_seconds};

/// A new, empty static playlist named `name`, after the others. `now` is in seconds
/// since the Unix epoch.
pub(crate) fn create(c: &Connection, name: &str, now: i64) -> rusqlite::Result<PlaylistId> {
    c.prepare_cached(
        "INSERT INTO playlists (parent_id, name, kind, position, created_at, modified_at)
         VALUES (NULL, ?1, ?2,
                 (SELECT coalesce(max(position), -1) + 1 FROM playlists WHERE parent_id IS NULL),
                 ?3, ?3)
         RETURNING id",
    )?
    .query_row(params![name, STATIC, now], |row| row.get(0).map(PlaylistId))
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

/// Fails unless `playlist` exists and is static.
pub(crate) fn check_static(c: &Connection, playlist: PlaylistId) -> rusqlite::Result<()> {
    let kind: Option<i64> = c
        .prepare_cached("SELECT kind FROM playlists WHERE id = ?1")?
        .query_row([playlist.0], |row| row.get(0))
        .optional()?;
    match kind {
        None => Refusal::UnknownPlaylist(playlist).raise(),
        Some(STATIC) => Ok(()),
        Some(_) => Refusal::NotStatic(playlist).raise(),
    }
}

/// Records that `playlist` changed at `now`.
pub(crate) fn touch(c: &Connection, playlist: PlaylistId, now: i64) -> rusqlite::Result<()> {
    c.prepare_cached("UPDATE playlists SET modified_at = ?2 WHERE id = ?1")?
        .execute(params![playlist.0, now])?;
    Ok(())
}

/// Renames `playlist`.
pub(crate) fn rename(
    c: &Connection,
    playlist: PlaylistId,
    name: &str,
    now: i64,
) -> rusqlite::Result<()> {
    let changed = c
        .prepare_cached("UPDATE playlists SET name = ?2, modified_at = ?3 WHERE id = ?1")?
        .execute(params![playlist.0, name, now])?;
    if changed == 0 {
        return Refusal::UnknownPlaylist(playlist).raise();
    }
    Ok(())
}

/// Deletes `playlist` and its items.
pub(crate) fn delete(c: &Connection, playlist: PlaylistId) -> rusqlite::Result<()> {
    let deleted = c
        .prepare_cached("DELETE FROM playlists WHERE id = ?1")?
        .execute([playlist.0])?;
    if deleted == 0 {
        return Refusal::UnknownPlaylist(playlist).raise();
    }
    Ok(())
}

/// A copy of the static playlist `playlist` named `name`, after the others.
pub(crate) fn duplicate(
    c: &Connection,
    playlist: PlaylistId,
    name: &str,
    now: i64,
) -> rusqlite::Result<PlaylistId> {
    check_static(c, playlist)?;
    let copy = create(c, name, now)?;
    c.prepare_cached(
        "INSERT INTO playlist_items (playlist_id, position, identity_id, ref_kind, missing_ref,
                                     title, artist, album, duration_ms)
         SELECT ?2, position, identity_id, ref_kind, missing_ref, title, artist, album, duration_ms
         FROM playlist_items WHERE playlist_id = ?1",
    )?
    .execute(params![playlist.0, copy.0])?;
    Ok(copy)
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
     FROM playlist_items i WHERE i.playlist_id = p.id)";

fn summary(row: &Row<'_>) -> rusqlite::Result<PlaylistSummary> {
    let count = |n: i64| usize::try_from(n).unwrap_or(0);
    Ok(PlaylistSummary {
        id: PlaylistId(row.get(0)?),
        name: row.get(1)?,
        created: from_unix_seconds(row.get::<_, Option<i64>>(2)?.unwrap_or(0)),
        modified: from_unix_seconds(row.get::<_, Option<i64>>(3)?.unwrap_or(0)),
        items: count(row.get(4)?),
        dead: count(row.get(5)?),
        duration: from_millis(row.get(6)?),
    })
}

/// One page of the static playlists, in the order they were created.
pub(crate) fn playlists(c: &Connection, page: Page) -> rusqlite::Result<Paged<PlaylistSummary>> {
    let (limit, offset) = page.sql();
    let items = c
        .prepare_cached(&format!(
            "SELECT {SUMMARY} FROM playlists p
             WHERE p.kind = ?1 AND p.parent_id IS NULL
             ORDER BY p.position, p.id LIMIT ?2 OFFSET ?3"
        ))?
        .query_map(params![STATIC, limit, offset], summary)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let total: i64 = c
        .prepare_cached("SELECT count(*) FROM playlists WHERE kind = ?1 AND parent_id IS NULL")?
        .query_row([STATIC], |row| row.get(0))?;
    Ok(Paged {
        items,
        total: usize::try_from(total).unwrap_or(0),
    })
}

/// The static playlist `playlist`, if there is one.
pub(crate) fn playlist(
    c: &Connection,
    playlist: PlaylistId,
) -> rusqlite::Result<Option<PlaylistSummary>> {
    c.prepare_cached(&format!(
        "SELECT {SUMMARY} FROM playlists p WHERE p.id = ?1 AND p.kind = ?2"
    ))?
    .query_row(params![playlist.0, STATIC], summary)
    .optional()
}
