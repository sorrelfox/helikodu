// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smart playlists: a name and a query, kept as printed query text with its sort keys
//! and limit. Their tracks are read through the query each time, so they follow the
//! library as it changes.

use std::time::Duration;

use helikodu_model::library::TrackId;
use helikodu_model::query::Query;
use rusqlite::{Connection, params};

use super::history::record;
use super::store::{create_row, kind_of};
use super::{PlaylistId, Refusal, SMART};
use crate::catalog::{Page, Paged, TrackSummary, sql_track_id};
use crate::db::unix_time::from_millis;
use crate::query::{parse_query_lenient, track_ids_on, track_summaries};

/// The query stored as `text`. Text a later version wrote and this one cannot read
/// searches for its words.
pub(crate) fn read_query(text: &str) -> Query {
    parse_query_lenient(text).query
}

/// The tracks the query `text` selects now; none when it cannot run.
pub(crate) fn tracks_of(c: &Connection, text: &str, now: i64) -> rusqlite::Result<Vec<TrackId>> {
    Ok(track_ids_on(c, &read_query(text), now)?.unwrap_or_default())
}

/// The number and total length of `tracks`.
pub(crate) fn totals(c: &Connection, tracks: &[TrackId]) -> rusqlite::Result<(usize, Duration)> {
    let list: Vec<String> = tracks
        .iter()
        .map(|t| sql_track_id(*t).to_string())
        .collect();
    let millis: i64 = c
        .prepare_cached(
            "SELECT coalesce(sum(t.duration_ms), 0) FROM json_each(?1) k
             JOIN tracks t ON t.id = k.value",
        )?
        .query_row([format!("[{}]", list.join(","))], |row| row.get(0))?;
    Ok((tracks.len(), from_millis(millis)))
}

/// The query text of the smart playlist `playlist`; refuses another kind.
fn query_text(c: &Connection, playlist: PlaylistId) -> rusqlite::Result<String> {
    match kind_of(c, playlist)? {
        (SMART, _) => {}
        _ => return Refusal::NotSmart(playlist).raise(),
    }
    c.prepare_cached("SELECT coalesce(query, '') FROM playlists WHERE id = ?1")?
        .query_row([playlist.0], |row| row.get(0))
}

/// The query of the smart playlist `playlist`.
pub(crate) fn smart_query(c: &Connection, playlist: PlaylistId) -> rusqlite::Result<Query> {
    query_text(c, playlist).map(|text| read_query(&text))
}

/// Every track of the smart playlist `playlist` now, in its order.
pub(crate) fn smart_track_ids(
    c: &Connection,
    playlist: PlaylistId,
    now: i64,
) -> rusqlite::Result<Vec<TrackId>> {
    tracks_of(c, &query_text(c, playlist)?, now)
}

/// One page of the tracks of the smart playlist `playlist` now, with their number.
pub(crate) fn smart_tracks(
    c: &Connection,
    playlist: PlaylistId,
    page: Page,
    now: i64,
) -> rusqlite::Result<Paged<TrackSummary>> {
    let ids = smart_track_ids(c, playlist, now)?;
    let start = page.offset.min(ids.len());
    let end = start.saturating_add(page.limit).min(ids.len());
    let wanted: Vec<i64> = ids[start..end].iter().map(|t| sql_track_id(*t)).collect();
    Ok(Paged {
        items: track_summaries(c, &wanted)?,
        total: ids.len(),
    })
}

/// A new smart playlist named `name` with the query `text`, after the others in
/// `parent`.
pub(crate) fn create_smart(
    c: &Connection,
    name: &str,
    text: &str,
    parent: Option<PlaylistId>,
    now: i64,
) -> rusqlite::Result<PlaylistId> {
    create_row(c, name, SMART, parent, Some(text), now)
}

/// Replaces the query of the smart playlist `playlist` with `text`; the change can be
/// undone. A locked playlist refuses.
pub(crate) fn set_query(
    c: &Connection,
    playlist: PlaylistId,
    text: &str,
    now: i64,
) -> rusqlite::Result<()> {
    match kind_of(c, playlist)? {
        (SMART, false) => {}
        (SMART, true) => return Refusal::Locked(playlist).raise(),
        _ => return Refusal::NotSmart(playlist).raise(),
    }
    let current = query_text(c, playlist)?;
    if current == text {
        return Ok(());
    }
    record(c, playlist, now)?;
    c.prepare_cached("UPDATE playlists SET query = ?2, modified_at = ?3 WHERE id = ?1")?
        .execute(params![playlist.0, text, now])?;
    Ok(())
}

/// When each of `tracks` came into the library, by track.
pub(crate) fn added_times(
    c: &Connection,
    tracks: &[TrackId],
) -> rusqlite::Result<std::collections::HashMap<TrackId, std::time::SystemTime>> {
    let list: Vec<String> = tracks
        .iter()
        .map(|t| sql_track_id(*t).to_string())
        .collect();
    c.prepare_cached(
        "SELECT t.id, t.added_at FROM json_each(?1) k JOIN tracks t ON t.id = k.value",
    )?
    .query_map([format!("[{}]", list.join(","))], |row| {
        Ok((
            crate::catalog::track_id_from_sql(row.get(0)?),
            crate::db::unix_time::from_unix_seconds(row.get(1)?),
        ))
    })?
    .collect()
}
