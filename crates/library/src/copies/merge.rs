// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Merging the identities of copies into one, and splitting one off again.
//!
//! Copies share one identity: one set of statistics, one rating, one history, the
//! same playlist entries. When a scan confirms a group whose tracks have identities
//! of their own (a library from before copies were recognised), they are merged
//! into the identity of the main copy's track:
//!
//! - play and skip counts are added up; the first play is the earliest, the last
//!   play and the last skip the latest;
//! - the rating is the main copy's when it has one, or else the one set most
//!   recently, or else (no times known) the highest;
//! - the position to resume from is that of the copy played most recently;
//! - history entries, playlist entries and analysis data move to the merged
//!   identity; the other identities are removed.
//!
//! Before the first merge that combines data of two identities, the database is
//! backed up once (see `backup`); until then such merges wait ([`Gate`]).

use rusqlite::{Connection, OptionalExtension, params};

/// Whether merges that combine the data of two identities may run.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Gate {
    /// The backup exists, so they may.
    pub(crate) allowed: bool,
    /// A merge was left out because it was not allowed.
    pub(crate) blocked: bool,
}

/// One `stats` row.
#[derive(Debug, Clone, Default)]
struct StatsRow {
    play_count: i64,
    skip_count: i64,
    first_played: Option<i64>,
    last_played: Option<i64>,
    last_skipped: Option<i64>,
    rating: Option<i64>,
    rated_at: Option<i64>,
    loved: Option<i64>,
    resume_ms: Option<i64>,
    flags: Option<i64>,
}

fn stats_row(c: &Connection, identity: i64) -> rusqlite::Result<Option<StatsRow>> {
    c.prepare_cached(
        "SELECT play_count, skip_count, first_played, last_played, last_skipped, rating,
                rated_at, loved, resume_ms, flags
         FROM stats WHERE identity_id = ?1",
    )?
    .query_row([identity], |row| {
        Ok(StatsRow {
            play_count: row.get(0)?,
            skip_count: row.get(1)?,
            first_played: row.get(2)?,
            last_played: row.get(3)?,
            last_skipped: row.get(4)?,
            rating: row.get(5)?,
            rated_at: row.get(6)?,
            loved: row.get(7)?,
            resume_ms: row.get(8)?,
            flags: row.get(9)?,
        })
    })
    .optional()
}

/// Whether `identity` carries anything a merge would combine.
fn has_data(c: &Connection, identity: i64) -> rusqlite::Result<bool> {
    c.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM stats WHERE identity_id = ?1
                        AND (play_count > 0 OR skip_count > 0 OR rating IS NOT NULL
                             OR resume_ms IS NOT NULL OR first_played IS NOT NULL
                             OR last_skipped IS NOT NULL OR loved IS NOT NULL
                             OR coalesce(flags, 0) != 0))
             OR EXISTS (SELECT 1 FROM history WHERE identity_id = ?1)
             OR EXISTS (SELECT 1 FROM playlist_items WHERE identity_id = ?1)
             OR EXISTS (SELECT 1 FROM analysis WHERE identity_id = ?1)",
    )?
    .query_row([identity], |row| row.get(0))
}

/// The statistics of a merge of `rows`, the first being the main copy's.
fn combine(rows: &[StatsRow]) -> StatsRow {
    let Some(main) = rows.first() else {
        return StatsRow::default();
    };
    let min = |f: fn(&StatsRow) -> Option<i64>| rows.iter().filter_map(f).min();
    let max = |f: fn(&StatsRow) -> Option<i64>| rows.iter().filter_map(f).max();
    let (rating, rated_at) = if main.rating.is_some() {
        (main.rating, main.rated_at)
    } else {
        let rated = rows.iter().filter(|r| r.rating.is_some());
        let newest = rated
            .clone()
            .filter(|r| r.rated_at.is_some())
            .max_by_key(|r| r.rated_at);
        match newest {
            Some(r) => (r.rating, r.rated_at),
            None => (rated.filter_map(|r| r.rating).max(), None),
        }
    };
    // The resume position of the copy played last; without plays, the main copy's.
    let resume_ms = rows
        .iter()
        .filter(|r| r.last_played.is_some())
        .max_by_key(|r| r.last_played)
        .map_or(main.resume_ms, |r| r.resume_ms);
    StatsRow {
        play_count: rows.iter().map(|r| r.play_count).sum(),
        skip_count: rows.iter().map(|r| r.skip_count).sum(),
        first_played: min(|r| r.first_played),
        last_played: max(|r| r.last_played),
        last_skipped: max(|r| r.last_skipped),
        rating,
        rated_at,
        loved: max(|r| r.loved),
        resume_ms,
        flags: main.flags.or_else(|| rows.iter().find_map(|r| r.flags)),
    }
}

/// Merges the identities `others` into `target` (see the module documentation).
/// Returns whether it merged; a merge that combines data while `gate` does not
/// allow it is left out and noted in `gate`.
pub(crate) fn merge(
    c: &Connection,
    target: i64,
    others: &[i64],
    gate: &mut Gate,
) -> rusqlite::Result<bool> {
    let mut others: Vec<i64> = others.iter().copied().filter(|&o| o != target).collect();
    others.sort_unstable();
    others.dedup();
    if others.is_empty() {
        return Ok(true);
    }
    if !gate.allowed {
        let mut with_data = usize::from(has_data(c, target)?);
        for &other in &others {
            with_data += usize::from(has_data(c, other)?);
        }
        if with_data >= 2 {
            gate.blocked = true;
            return Ok(false);
        }
    }
    let mut rows = Vec::with_capacity(others.len() + 1);
    let mut any = false;
    for &identity in std::iter::once(&target).chain(&others) {
        let row = stats_row(c, identity)?;
        any |= row.is_some();
        rows.push(row.unwrap_or_default());
    }
    if any {
        let s = combine(&rows);
        c.prepare_cached(
            "INSERT INTO stats (identity_id, play_count, skip_count, first_played, last_played,
                                last_skipped, rating, rated_at, loved, resume_ms, flags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (identity_id) DO UPDATE SET
                 play_count = excluded.play_count, skip_count = excluded.skip_count,
                 first_played = excluded.first_played, last_played = excluded.last_played,
                 last_skipped = excluded.last_skipped, rating = excluded.rating,
                 rated_at = excluded.rated_at, loved = excluded.loved,
                 resume_ms = excluded.resume_ms, flags = excluded.flags",
        )?
        .execute(params![
            target,
            s.play_count,
            s.skip_count,
            s.first_played,
            s.last_played,
            s.last_skipped,
            s.rating,
            s.rated_at,
            s.loved,
            s.resume_ms,
            s.flags
        ])?;
    }
    let target_has_analysis: bool = c
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM analysis WHERE identity_id = ?1)")?
        .query_row([target], |row| row.get(0))?;
    let mut has_analysis = target_has_analysis;
    for &other in &others {
        c.prepare_cached("DELETE FROM stats WHERE identity_id = ?1")?
            .execute([other])?;
        c.prepare_cached("UPDATE history SET identity_id = ?2 WHERE identity_id = ?1")?
            .execute(params![other, target])?;
        c.prepare_cached("UPDATE playlist_items SET identity_id = ?2 WHERE identity_id = ?1")?
            .execute(params![other, target])?;
        if has_analysis {
            c.prepare_cached("DELETE FROM analysis WHERE identity_id = ?1")?
                .execute([other])?;
        } else {
            has_analysis = c
                .prepare_cached("UPDATE analysis SET identity_id = ?2 WHERE identity_id = ?1")?
                .execute(params![other, target])?
                > 0;
        }
        c.prepare_cached(
            "INSERT OR IGNORE INTO chosen_kinds (identity_id, kind)
             SELECT ?2, kind FROM chosen_kinds WHERE identity_id = ?1",
        )?
        .execute(params![other, target])?;
        crate::labels::move_identity(c, other, target)?;
        c.prepare_cached("UPDATE tracks SET identity_id = ?2 WHERE identity_id = ?1")?
            .execute(params![other, target])?;
        c.prepare_cached("DELETE FROM identities WHERE id = ?1")?
            .execute([other])?;
    }
    Ok(true)
}

/// A new identity for tracks that stop being copies of the tracks of `identity`:
/// the same fingerprint and a copy of its statistics (not its history or playlist
/// entries, which stay with `identity`). Returns its id.
pub(crate) fn split_off(c: &Connection, identity: i64, now: i64) -> rusqlite::Result<i64> {
    let new: i64 = c
        .prepare_cached(
            "INSERT INTO identities (created_at, content_hash, audio_key)
             SELECT ?2, content_hash, audio_key FROM identities WHERE id = ?1
             RETURNING id",
        )?
        .query_row(params![identity, now], |row| row.get(0))
        .optional()?
        .map_or_else(
            || {
                c.prepare_cached("INSERT INTO identities (created_at) VALUES (?1) RETURNING id")?
                    .query_row([now], |row| row.get(0))
            },
            Ok,
        )?;
    c.prepare_cached(
        "INSERT INTO stats (identity_id, play_count, skip_count, first_played, last_played,
                            last_skipped, rating, rated_at, loved, resume_ms, flags)
         SELECT ?2, play_count, skip_count, first_played, last_played, last_skipped, rating,
                rated_at, loved, resume_ms, flags
         FROM stats WHERE identity_id = ?1",
    )?
    .execute(params![identity, new])?;
    crate::labels::copy_identity(c, identity, new)?;
    Ok(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(plays: i64, last: Option<i64>, rating: Option<i64>, rated: Option<i64>) -> StatsRow {
        StatsRow {
            play_count: plays,
            last_played: last,
            first_played: last,
            rating,
            rated_at: rated,
            resume_ms: last.map(|l| l * 10),
            ..StatsRow::default()
        }
    }

    #[test]
    fn counts_add_up_and_times_spread() {
        let merged = combine(&[row(2, Some(50), None, None), row(3, Some(80), None, None)]);
        assert_eq!(merged.play_count, 5);
        assert_eq!(merged.first_played, Some(50));
        assert_eq!(merged.last_played, Some(80));
        assert_eq!(merged.resume_ms, Some(800));
    }

    #[test]
    fn the_rating_rules() {
        // The main copy's rating wins.
        let main_rated = combine(&[row(0, None, Some(4), None), row(0, None, Some(10), Some(9))]);
        assert_eq!(main_rated.rating, Some(4));
        // Else the newest.
        let newest = combine(&[
            row(0, None, None, None),
            row(0, None, Some(6), Some(5)),
            row(0, None, Some(2), Some(7)),
        ]);
        assert_eq!(newest.rating, Some(2));
        // Else the highest.
        let highest = combine(&[
            row(0, None, None, None),
            row(0, None, Some(6), None),
            row(0, None, Some(8), None),
        ]);
        assert_eq!(highest.rating, Some(8));
    }
}
