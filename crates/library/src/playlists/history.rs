// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Undo and redo of playlist edits.
//!
//! Before an edit, the playlist as it is (its name, its query and its items) is
//! recorded as an undo step, and the redo steps are dropped. Undoing records the
//! playlist as it is as a redo step and puts back the newest undo step; redoing does
//! the same the other way round. The newest [`KEPT`] undo steps of each playlist are
//! kept; the steps go with their playlist.

use rusqlite::{Connection, OptionalExtension, params};

use super::store::kind_of;
use super::{PlaylistId, Refusal};

/// The undo steps kept per playlist.
pub(crate) const KEPT: i64 = 100;

/// Records `playlist` as it is as a step of the kind `redo`.
fn snapshot(c: &Connection, playlist: PlaylistId, redo: bool, now: i64) -> rusqlite::Result<()> {
    let step: i64 = c
        .prepare_cached(
            "INSERT INTO playlist_history (playlist_id, redo, name, query, created_at)
             SELECT id, ?2, name, query, ?3 FROM playlists WHERE id = ?1
             RETURNING id",
        )?
        .query_row(params![playlist.0, i64::from(redo), now], |row| row.get(0))?;
    c.prepare_cached(
        "INSERT INTO playlist_history_items (step_id, position, identity_id, ref_kind,
                                             missing_ref, title, artist, album, duration_ms)
         SELECT ?2, position, identity_id, ref_kind, missing_ref, title, artist, album,
                duration_ms
         FROM playlist_items WHERE playlist_id = ?1",
    )?
    .execute(params![playlist.0, step])?;
    Ok(())
}

/// Records `playlist` as it is before an edit, so that the edit can be undone.
pub(crate) fn record(c: &Connection, playlist: PlaylistId, now: i64) -> rusqlite::Result<()> {
    snapshot(c, playlist, false, now)?;
    c.prepare_cached("DELETE FROM playlist_history WHERE playlist_id = ?1 AND redo = 1")?
        .execute([playlist.0])?;
    c.prepare_cached(
        "DELETE FROM playlist_history WHERE playlist_id = ?1 AND redo = 0 AND id NOT IN
            (SELECT id FROM playlist_history WHERE playlist_id = ?1 AND redo = 0
             ORDER BY id DESC LIMIT ?2)",
    )?
    .execute(params![playlist.0, KEPT])?;
    Ok(())
}

/// Undoes the newest edit of `playlist` (`redo` false) or does the newest undone edit
/// again (`redo` true). Returns whether there was one. A locked playlist refuses.
pub(crate) fn history_step(
    c: &Connection,
    playlist: PlaylistId,
    redo: bool,
    now: i64,
) -> rusqlite::Result<bool> {
    let (_, locked) = kind_of(c, playlist)?;
    if locked {
        return Refusal::Locked(playlist).raise();
    }
    let step: Option<(i64, String, Option<String>)> = c
        .prepare_cached(
            "SELECT id, name, query FROM playlist_history
             WHERE playlist_id = ?1 AND redo = ?2 ORDER BY id DESC LIMIT 1",
        )?
        .query_row(params![playlist.0, i64::from(redo)], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    let Some((step, name, query)) = step else {
        return Ok(false);
    };
    snapshot(c, playlist, !redo, now)?;
    c.prepare_cached("UPDATE playlists SET name = ?2, query = ?3, modified_at = ?4 WHERE id = ?1")?
        .execute(params![playlist.0, name, query, now])?;
    c.prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1")?
        .execute([playlist.0])?;
    // An identity that has gone since is kept as a dead item with what it was.
    c.prepare_cached(
        "INSERT INTO playlist_items (playlist_id, position, identity_id, ref_kind, missing_ref,
                                     title, artist, album, duration_ms)
         SELECT ?1, h.position,
                (SELECT id FROM identities WHERE id = h.identity_id),
                h.ref_kind, h.missing_ref, h.title, h.artist, h.album, h.duration_ms
         FROM playlist_history_items h WHERE h.step_id = ?2 ORDER BY h.position",
    )?
    .execute(params![playlist.0, step])?;
    c.prepare_cached("DELETE FROM playlist_history WHERE id = ?1")?
        .execute([step])?;
    Ok(true)
}
