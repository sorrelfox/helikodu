// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading and writing `stats` and `history`.

use std::time::Duration;

use helikodu_model::library::TrackId;
use rusqlite::{Connection, OptionalExtension, params};

use super::rating::Rating;
use super::records::{HistoryEntry, HistoryQuery, Listen, TrackStats};
use super::rules::{ListenEnd, ListenOutcome, PlayRules};
use crate::catalog::{IdentityId, TrackKind, sql_track_id, track_id_from_sql};
use crate::db::unix_time::{from_millis, from_unix_seconds, to_millis, to_unix_seconds};

/// The `stats` row of `identity`, created empty if there is none.
fn ensure_row(c: &Connection, identity: IdentityId) -> rusqlite::Result<()> {
    c.prepare_cached("INSERT INTO stats (identity_id) VALUES (?1) ON CONFLICT DO NOTHING")?
        .execute([identity.0])?;
    Ok(())
}

/// The identity, length and kind of `track`.
pub(crate) fn track_facts(
    c: &Connection,
    track: TrackId,
) -> rusqlite::Result<Option<(IdentityId, Option<Duration>, TrackKind)>> {
    c.prepare_cached("SELECT identity_id, duration_ms, kind FROM tracks WHERE id = ?1")?
        .query_row([sql_track_id(track)], |row| {
            Ok((
                IdentityId(row.get(0)?),
                row.get::<_, Option<i64>>(1)?.map(from_millis),
                TrackKind::from_sql(row.get(2)?),
            ))
        })
        .optional()
}

/// Records `listen` of `track` under `rules` and returns what it counted as; `None` if
/// the track is not in the library.
pub(crate) fn record_listen(
    c: &Connection,
    track: TrackId,
    listen: &Listen,
    rules: &PlayRules,
) -> rusqlite::Result<Option<ListenOutcome>> {
    let Some((identity, length, kind)) = track_facts(c, track)? else {
        return Ok(None);
    };
    let outcome = rules.classify(listen.played, length, listen.end);
    let started = to_unix_seconds(listen.started_at);
    ensure_row(c, identity)?;
    match outcome {
        ListenOutcome::Play => {
            c.prepare_cached(
                "UPDATE stats SET play_count = play_count + 1,
                     first_played = coalesce(first_played, ?2),
                     last_played = max(coalesce(last_played, ?2), ?2)
                 WHERE identity_id = ?1",
            )?
            .execute(params![identity.0, started])?;
        }
        ListenOutcome::Skip => {
            c.prepare_cached(
                "UPDATE stats SET skip_count = skip_count + 1,
                     last_skipped = max(coalesce(last_skipped, ?2), ?2)
                 WHERE identity_id = ?1",
            )?
            .execute(params![identity.0, started])?;
        }
        ListenOutcome::Partial => {}
    }
    if rules.keeps_resume_position(length, kind) {
        let resume = match (listen.end, listen.position) {
            (ListenEnd::Finished, _) => None,
            (_, position) => position.map(to_millis),
        };
        if resume.is_some() || listen.end == ListenEnd::Finished {
            set_resume_ms(c, identity, resume)?;
        }
    }
    if !listen.played.is_zero() {
        c.prepare_cached(
            "INSERT INTO history (identity_id, started_at, played_ms, counted, context)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?
        .execute(params![
            identity.0,
            started,
            to_millis(listen.played),
            outcome == ListenOutcome::Play,
            listen.context
        ])?;
    }
    Ok(Some(outcome))
}

/// Sets or clears the rating of `identity`.
pub(crate) fn set_rating(
    c: &Connection,
    identity: IdentityId,
    rating: Option<Rating>,
) -> rusqlite::Result<()> {
    ensure_row(c, identity)?;
    c.prepare_cached("UPDATE stats SET rating = ?2 WHERE identity_id = ?1")?
        .execute(params![identity.0, rating.map(Rating::half_stars)])?;
    Ok(())
}

/// Sets or clears where listening to `identity` stopped.
pub(crate) fn set_resume_ms(
    c: &Connection,
    identity: IdentityId,
    resume_ms: Option<i64>,
) -> rusqlite::Result<()> {
    ensure_row(c, identity)?;
    c.prepare_cached("UPDATE stats SET resume_ms = ?2 WHERE identity_id = ?1")?
        .execute(params![identity.0, resume_ms])?;
    Ok(())
}

/// The statistics of `identity`; empty ones if nothing was recorded.
pub(crate) fn stats_of(c: &Connection, identity: IdentityId) -> rusqlite::Result<TrackStats> {
    let stats = c
        .prepare_cached(
            "SELECT play_count, skip_count, first_played, last_played, last_skipped, rating,
                    resume_ms
             FROM stats WHERE identity_id = ?1",
        )?
        .query_row([identity.0], |row| {
            Ok(TrackStats {
                play_count: u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                skip_count: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                first_played: row.get::<_, Option<i64>>(2)?.map(from_unix_seconds),
                last_played: row.get::<_, Option<i64>>(3)?.map(from_unix_seconds),
                last_skipped: row.get::<_, Option<i64>>(4)?.map(from_unix_seconds),
                rating: row
                    .get::<_, Option<u8>>(5)?
                    .and_then(Rating::from_half_stars),
                resume_position: row.get::<_, Option<i64>>(6)?.map(from_millis),
            })
        })
        .optional()?;
    Ok(stats.unwrap_or_default())
}

/// The history entries `query` asks for, newest first. `identity` restricts them to
/// one identity.
pub(crate) fn history(
    c: &Connection,
    identity: Option<IdentityId>,
    query: &HistoryQuery,
) -> rusqlite::Result<Vec<HistoryEntry>> {
    let limit = i64::try_from(query.limit).unwrap_or(i64::MAX);
    c.prepare_cached(
        "SELECT h.identity_id,
                (SELECT t.id FROM tracks t WHERE t.identity_id = h.identity_id
                 ORDER BY t.id LIMIT 1),
                h.started_at, h.played_ms, h.counted, h.context
         FROM history h
         WHERE (?1 IS NULL OR h.identity_id = ?1)
           AND (?2 IS NULL OR h.started_at >= ?2)
           AND (?3 IS NULL OR h.started_at < ?3)
           AND (?4 = 0 OR h.counted = 1)
         ORDER BY h.started_at DESC, h.id DESC
         LIMIT ?5",
    )?
    .query_map(
        params![
            identity.map(|i| i.0),
            query.since.map(to_unix_seconds),
            query.until.map(to_unix_seconds),
            query.plays_only,
            limit
        ],
        |row| {
            Ok(HistoryEntry {
                identity: IdentityId(row.get(0)?),
                track: row.get::<_, Option<i64>>(1)?.map(track_id_from_sql),
                started_at: from_unix_seconds(row.get(2)?),
                played: from_millis(row.get(3)?),
                counted: row.get(4)?,
                context: row.get(5)?,
            })
        },
    )?
    .collect()
}
