// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The items of a playlist: adding, inserting, moving, removing and listing them.
//!
//! Positions count from 0 without gaps and are unique within a playlist. A change of
//! positions first moves the rows concerned to negative positions and then to their new
//! ones, so the uniqueness constraint holds after every statement.

use helikodu_model::library::TrackId;
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::store::{check_static, touch};
use super::{
    DeadItem, DeadReference, ItemContent, NewItem, PlaylistId, PlaylistItem, PlaylistItemId,
    REF_FOREIGN, REF_PATH, REF_URL, Refusal,
};
use crate::catalog::{
    AlbumId, IdentityId, Page, Paged, TrackKind, TrackSection, TrackSummary, sql_track_id,
    track_id_from_sql,
};
use crate::db::unix_time::from_millis;
use crate::db::{path_from_blob, path_to_blob};

/// The number of items of `playlist`.
fn len(c: &Connection, playlist: PlaylistId) -> rusqlite::Result<usize> {
    let n: i64 = c
        .prepare_cached("SELECT count(*) FROM playlist_items WHERE playlist_id = ?1")?
        .query_row([playlist.0], |row| row.get(0))?;
    Ok(usize::try_from(n).unwrap_or(0))
}

fn sql_position(position: usize) -> i64 {
    i64::try_from(position).unwrap_or(i64::MAX)
}

/// Moves the items at `from` and after by `by` places.
fn shift(c: &Connection, playlist: PlaylistId, from: usize, by: i64) -> rusqlite::Result<()> {
    if by == 0 {
        return Ok(());
    }
    // Negative positions are free, so the first statement cannot collide; the second
    // flips them back.
    c.prepare_cached(
        "UPDATE playlist_items SET position = -1 - (position + ?3)
         WHERE playlist_id = ?1 AND position >= ?2",
    )?
    .execute(params![playlist.0, sql_position(from), by])?;
    c.prepare_cached(
        "UPDATE playlist_items SET position = -1 - position WHERE playlist_id = ?1 AND position < 0",
    )?
    .execute([playlist.0])?;
    Ok(())
}

/// Inserts `rows` at `at`, moving the items from there on back.
pub(crate) fn insert_rows(
    c: &Connection,
    playlist: PlaylistId,
    at: usize,
    rows: &[NewItem],
) -> rusqlite::Result<()> {
    shift(
        c,
        playlist,
        at,
        i64::try_from(rows.len()).unwrap_or(i64::MAX),
    )?;
    let mut insert = c.prepare_cached(
        "INSERT INTO playlist_items (playlist_id, position, identity_id, ref_kind, missing_ref,
                                     title, artist, album, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    for (offset, row) in rows.iter().enumerate() {
        insert.execute(params![
            playlist.0,
            sql_position(at + offset),
            row.identity,
            row.ref_kind,
            row.reference,
            row.title,
            row.artist,
            row.album,
            row.duration_ms,
        ])?;
    }
    Ok(())
}

/// The items for `tracks`, with what is known of each now; refuses a track that is not
/// in the library.
fn track_rows(c: &Connection, tracks: &[TrackId]) -> rusqlite::Result<Vec<NewItem>> {
    let mut query = c.prepare_cached(
        "SELECT t.identity_id, d.path, f.name, t.title, t.artist, a.title, t.duration_ms
         FROM tracks t JOIN files f ON f.id = t.file_id JOIN dirs d ON d.id = f.dir_id
         LEFT JOIN albums a ON a.id = t.album_id
         WHERE t.id = ?1",
    )?;
    let mut rows = Vec::with_capacity(tracks.len());
    for &track in tracks {
        let row = query
            .query_row([sql_track_id(track)], |row| {
                let path = path_from_blob(row.get(1)?).join(path_from_blob(row.get(2)?));
                Ok(NewItem {
                    identity: Some(row.get(0)?),
                    ref_kind: REF_PATH,
                    reference: Some(path_to_blob(&path).to_vec()),
                    title: row.get(3)?,
                    artist: row.get(4)?,
                    album: row.get(5)?,
                    duration_ms: row.get(6)?,
                })
            })
            .optional()?;
        match row {
            Some(row) => rows.push(row),
            None => return Refusal::UnknownTrack(track).raise(),
        }
    }
    Ok(rows)
}

/// Adds `tracks` at the end of `playlist`. Returns the position of the first.
pub(crate) fn append_tracks(
    c: &Connection,
    playlist: PlaylistId,
    tracks: &[TrackId],
    now: i64,
) -> rusqlite::Result<usize> {
    check_static(c, playlist)?;
    let at = len(c, playlist)?;
    insert_rows(c, playlist, at, &track_rows(c, tracks)?)?;
    touch(c, playlist, now)?;
    Ok(at)
}

/// Inserts `tracks` into `playlist` at `at` (at most its length).
pub(crate) fn insert_tracks(
    c: &Connection,
    playlist: PlaylistId,
    at: usize,
    tracks: &[TrackId],
    now: i64,
) -> rusqlite::Result<()> {
    check_static(c, playlist)?;
    let n = len(c, playlist)?;
    if at > n {
        return Refusal::BadPosition {
            position: at,
            len: n,
        }
        .raise();
    }
    insert_rows(c, playlist, at, &track_rows(c, tracks)?)?;
    touch(c, playlist, now)
}

/// The ids of the items of `playlist` in order.
fn item_ids(c: &Connection, playlist: PlaylistId) -> rusqlite::Result<Vec<i64>> {
    c.prepare_cached("SELECT id FROM playlist_items WHERE playlist_id = ?1 ORDER BY position")?
        .query_map([playlist.0], |row| row.get(0))?
        .collect()
}

/// `positions` sorted without repeats; refuses one past the end.
fn checked(positions: &[usize], len: usize) -> rusqlite::Result<Vec<usize>> {
    let mut sorted = positions.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    match sorted.last() {
        Some(&last) if last >= len => Refusal::BadPosition {
            position: last,
            len,
        }
        .raise(),
        _ => Ok(sorted),
    }
}

/// Moves the items at `positions` (in their order) so that they stand together before
/// the item that was at `to`, or at the end when `to` is the length. Returns the new
/// position of the first.
pub(crate) fn move_items(
    c: &Connection,
    playlist: PlaylistId,
    positions: &[usize],
    to: usize,
    now: i64,
) -> rusqlite::Result<usize> {
    check_static(c, playlist)?;
    let ids = item_ids(c, playlist)?;
    let moving = checked(positions, ids.len())?;
    if to > ids.len() {
        return Refusal::BadPosition {
            position: to,
            len: ids.len(),
        }
        .raise();
    }
    let before = moving.iter().filter(|&&p| p < to).count();
    let target = to - before;
    if moving.is_empty() {
        return Ok(target);
    }
    let mut picked = Vec::with_capacity(moving.len());
    let mut rest = Vec::with_capacity(ids.len());
    let mut next = moving.iter().peekable();
    for (position, &id) in ids.iter().enumerate() {
        if next.peek() == Some(&&position) {
            next.next();
            picked.push(id);
        } else {
            rest.push(id);
        }
    }
    let mut order = rest;
    order.splice(target..target, picked);
    let first_change = order.iter().zip(&ids).position(|(a, b)| a != b);
    if let Some(start) = first_change {
        let end = order
            .iter()
            .zip(&ids)
            .rposition(|(a, b)| a != b)
            .map_or(start, |e| e + 1);
        // Only the span that changed is rewritten.
        renumber(c, playlist, start, end, &order[start..end])?;
        touch(c, playlist, now)?;
    }
    Ok(target)
}

/// Gives the items `ids` the positions `start..`. They are the items now between
/// `start` and `end`.
fn renumber(
    c: &Connection,
    playlist: PlaylistId,
    start: usize,
    end: usize,
    ids: &[i64],
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "UPDATE playlist_items SET position = -1 - position
         WHERE playlist_id = ?1 AND position >= ?2 AND position < ?3",
    )?
    .execute(params![playlist.0, sql_position(start), sql_position(end)])?;
    let mut set = c.prepare_cached("UPDATE playlist_items SET position = ?2 WHERE id = ?1")?;
    for (offset, id) in ids.iter().enumerate() {
        set.execute(params![id, sql_position(start + offset)])?;
    }
    Ok(())
}

/// Removes the items at `positions`. Returns how many were removed.
pub(crate) fn remove_items(
    c: &Connection,
    playlist: PlaylistId,
    positions: &[usize],
    now: i64,
) -> rusqlite::Result<usize> {
    check_static(c, playlist)?;
    let ids = item_ids(c, playlist)?;
    let removing = checked(positions, ids.len())?;
    let Some(&first) = removing.first() else {
        return Ok(0);
    };
    let mut delete = c.prepare_cached("DELETE FROM playlist_items WHERE id = ?1")?;
    for &position in &removing {
        delete.execute([ids[position]])?;
    }
    let kept: Vec<i64> = ids[first..]
        .iter()
        .enumerate()
        .filter(|(offset, _)| removing.binary_search(&(first + offset)).is_err())
        .map(|(_, &id)| id)
        .collect();
    renumber(c, playlist, first, ids.len(), &kept)?;
    touch(c, playlist, now)?;
    Ok(removing.len())
}

/// The columns of an item: the item `i`, its track `t` (the first track with its
/// identity), the track's file `f` and album `a`.
const ITEM: &str = "i.id, i.position, i.identity_id, i.ref_kind, i.missing_ref, i.title, i.artist,
    i.album, i.duration_ms,
    t.id, t.title, t.artist, t.album_id, a.title, t.disc_no, t.track_no, t.duration_ms, t.year,
    t.kind, t.section, t.sub_index, f.missing_since IS NOT NULL, t.copies";

const ITEM_FROM: &str = "FROM playlist_items i
    LEFT JOIN tracks t ON t.id = (SELECT t2.id FROM tracks t2 JOIN files f2 ON f2.id = t2.file_id
        WHERE t2.identity_id = i.identity_id
        ORDER BY f2.missing_since IS NOT NULL, t2.is_copy, t2.id LIMIT 1)
    LEFT JOIN files f ON f.id = t.file_id
    LEFT JOIN albums a ON a.id = t.album_id";

fn item(row: &Row<'_>) -> rusqlite::Result<PlaylistItem> {
    let id = PlaylistItemId(row.get(0)?);
    let position = usize::try_from(row.get::<_, i64>(1)?).unwrap_or(0);
    let content = match row.get::<_, Option<i64>>(9)? {
        Some(track) => ItemContent::Track(TrackSummary {
            id: track_id_from_sql(track),
            title: row.get(10)?,
            artist: row.get(11)?,
            album: row.get::<_, Option<i64>>(12)?.map(AlbumId),
            album_title: row.get(13)?,
            disc_no: row.get(14)?,
            track_no: row.get(15)?,
            duration: row.get::<_, Option<i64>>(16)?.map(from_millis),
            year: row.get(17)?,
            kind: TrackKind::from_sql(row.get(18)?),
            section: TrackSection::from_sql(row.get(19)?),
            sub_index: row.get(20)?,
            missing: row.get(21)?,
            copies: u32::try_from(row.get::<_, i64>(22)?).unwrap_or(0),
        }),
        None => ItemContent::Dead(DeadItem {
            reference: reference(row.get(3)?, row.get(4)?),
            identity: row.get::<_, Option<i64>>(2)?.map(IdentityId),
            title: row.get(5)?,
            artist: row.get(6)?,
            album: row.get(7)?,
            duration: row.get::<_, Option<i64>>(8)?.map(from_millis),
        }),
    };
    Ok(PlaylistItem {
        id,
        position,
        content,
    })
}

/// The reference stored as `kind` and `bytes`.
pub(crate) fn reference(kind: i64, bytes: Option<Vec<u8>>) -> DeadReference {
    let Some(bytes) = bytes else {
        return DeadReference::Unknown;
    };
    match kind {
        REF_FOREIGN => DeadReference::Foreign(bytes),
        REF_URL => DeadReference::Url(String::from_utf8_lossy(&bytes).into_owned()),
        _ if bytes.is_empty() => DeadReference::Unknown,
        _ => DeadReference::Path(path_from_blob(bytes)),
    }
}

/// One page of the items of the static playlist `playlist`, in order.
pub(crate) fn item_page(
    c: &Connection,
    playlist: PlaylistId,
    page: Page,
) -> rusqlite::Result<Paged<PlaylistItem>> {
    check_static(c, playlist)?;
    let (limit, offset) = page.sql();
    let items = c
        .prepare_cached(&format!(
            "SELECT {ITEM} {ITEM_FROM} WHERE i.playlist_id = ?1
             ORDER BY i.position LIMIT ?2 OFFSET ?3"
        ))?
        .query_map(params![playlist.0, limit, offset], item)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Paged {
        items,
        total: len(c, playlist)?,
    })
}

/// The dead items of the static playlist `playlist`, in order.
pub(crate) fn dead_items(
    c: &Connection,
    playlist: PlaylistId,
) -> rusqlite::Result<Vec<PlaylistItem>> {
    check_static(c, playlist)?;
    c.prepare_cached(&format!(
        "SELECT {ITEM} {ITEM_FROM} WHERE i.playlist_id = ?1 AND t.id IS NULL ORDER BY i.position"
    ))?
    .query_map([playlist.0], item)?
    .collect()
}
