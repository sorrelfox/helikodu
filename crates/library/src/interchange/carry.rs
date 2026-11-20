// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Statistics carried from a replaced file to its replacement.
//!
//! When a file goes missing and another file of the same recording is in the
//! library (the same title and artist, lengths within 2 s, as the duplicate
//! finder's "same recording"), for example an MP3 replaced by a FLAC, the
//! missing track's plays, rating, history and playlist entries can move to the
//! new one. The counts add up, the earliest first and the latest last play win,
//! and the new track keeps its own rating when it has one. The move is an import
//! run, undone like one.

use std::path::PathBuf;
use std::time::SystemTime;

use helikodu_model::library::TrackId;
use rusqlite::{Connection, OptionalExtension};

use super::apply::{
    ROW_HISTORY_MOVED, ROW_ITEM_MOVED, change_stats, record_row, set_summary, start_run,
};
use super::merge::{self, Stats};
use super::{ImportError, ImportRunId, ImportRunKind};
use crate::Library;
use crate::catalog::{sql_track_id, track_id_from_sql};
use crate::db::path_from_blob;
use crate::db::unix_time::to_unix_seconds;
use crate::stats::store::stats_of;

/// A missing track with statistics and a track of the same recording that could
/// take them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarryCandidate {
    /// The missing track.
    pub old: TrackId,
    /// Its file.
    pub old_path: PathBuf,
    /// The track that could take its statistics.
    pub new: TrackId,
    /// Its file.
    pub new_path: PathBuf,
    /// The title.
    pub title: Option<String>,
    /// The artist.
    pub artist: Option<String>,
    /// The missing track's statistics.
    pub stats: crate::stats::TrackStats,
    /// Its history entries.
    pub history: usize,
    /// Its playlist entries.
    pub playlist_items: usize,
    /// The playlists those entries are in.
    pub playlists: usize,
}

const CANDIDATES: &str = "
    SELECT o.id, od.path, ofl.name, n.id, nd.path, nf.name, o.title, o.artist, o.identity_id,
           (SELECT count(*) FROM history h WHERE h.identity_id = o.identity_id),
           (SELECT count(*) FROM playlist_items i WHERE i.identity_id = o.identity_id),
           (SELECT count(DISTINCT i.playlist_id) FROM playlist_items i
            WHERE i.identity_id = o.identity_id)
    FROM tracks o JOIN files ofl ON ofl.id = o.file_id JOIN dirs od ON od.id = ofl.dir_id
    JOIN tracks n ON n.title_fold = o.title_fold AND n.identity_id != o.identity_id
         AND hk_fold(coalesce(n.main_artist, n.artist, ''))
             = hk_fold(coalesce(o.main_artist, o.artist, ''))
         AND abs(n.duration_ms - o.duration_ms) <= 2000
    JOIN files nf ON nf.id = n.file_id JOIN dirs nd ON nd.id = nf.dir_id
    WHERE ofl.missing_since IS NOT NULL AND nf.missing_since IS NULL
      AND o.title_fold IS NOT NULL AND o.title_fold != '' AND n.is_copy = 0
      AND o.duration_ms IS NOT NULL AND n.duration_ms IS NOT NULL
      AND (EXISTS (SELECT 1 FROM stats s WHERE s.identity_id = o.identity_id
                   AND (s.play_count > 0 OR s.skip_count > 0 OR s.rating IS NOT NULL
                        OR s.loved IS NOT NULL))
           OR EXISTS (SELECT 1 FROM history h WHERE h.identity_id = o.identity_id)
           OR EXISTS (SELECT 1 FROM playlist_items i WHERE i.identity_id = o.identity_id))
    ORDER BY od.path, ofl.name, nd.path, nf.name";

fn candidates(c: &Connection) -> rusqlite::Result<Vec<CarryCandidate>> {
    let mut statement = c.prepare(CANDIDATES)?;
    let rows: Vec<(CarryCandidate, i64)> = statement
        .query_map([], |row| {
            Ok((
                CarryCandidate {
                    old: track_id_from_sql(row.get(0)?),
                    old_path: path_from_blob(row.get(1)?).join(path_from_blob(row.get(2)?)),
                    new: track_id_from_sql(row.get(3)?),
                    new_path: path_from_blob(row.get(4)?).join(path_from_blob(row.get(5)?)),
                    title: row.get(6)?,
                    artist: row.get(7)?,
                    stats: crate::stats::TrackStats::default(),
                    history: usize::try_from(row.get::<_, i64>(9)?).unwrap_or(0),
                    playlist_items: usize::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
                    playlists: usize::try_from(row.get::<_, i64>(11)?).unwrap_or(0),
                },
                row.get(8)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    rows.into_iter()
        .map(|(mut candidate, identity)| {
            candidate.stats = stats_of(c, crate::catalog::IdentityId(identity))?;
            Ok(candidate)
        })
        .collect()
}

fn min(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The statistics of the new track after taking the old one's.
fn carried(old: &Stats, new: &Stats) -> Stats {
    Stats {
        play_count: new.play_count.saturating_add(old.play_count),
        skip_count: new.skip_count.saturating_add(old.skip_count),
        first_played: min(new.first_played, old.first_played),
        last_played: new.last_played.max(old.last_played),
        last_skipped: new.last_skipped.max(old.last_skipped),
        rating: new.rating.or(old.rating),
        loved: new.loved.max(old.loved),
    }
}

fn identity(c: &Connection, track: TrackId) -> rusqlite::Result<Option<(i64, String)>> {
    c.prepare_cached(
        "SELECT t.identity_id, d.path_text || '/' || f.name_text
         FROM tracks t JOIN files f ON f.id = t.file_id JOIN dirs d ON d.id = f.dir_id
         WHERE t.id = ?1",
    )?
    .query_row([sql_track_id(track)], |row| Ok((row.get(0)?, row.get(1)?)))
    .optional()
}

/// Moves the statistics of `old` to `new` as part of `run`; whether both
/// tracks exist.
fn carry_into(c: &Connection, run: i64, old: TrackId, new: TrackId) -> rusqlite::Result<bool> {
    let (Some((from, _)), Some((to, _))) = (identity(c, old)?, identity(c, new)?) else {
        return Ok(false);
    };
    if from == to {
        return Ok(true);
    }
    let (old_stats, old_existed) = merge::read(c, from)?;
    let (new_stats, new_existed) = merge::read(c, to)?;
    let after = carried(&old_stats, &new_stats);
    change_stats(c, run, to, &new_stats, new_existed, &after)?;
    change_stats(c, run, from, &old_stats, old_existed, &Stats::default())?;
    let history: Vec<i64> = c
        .prepare_cached("SELECT id FROM history WHERE identity_id = ?1")?
        .query_map([from], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in history {
        c.prepare_cached("UPDATE history SET identity_id = ?2 WHERE id = ?1")?
            .execute(rusqlite::params![id, to])?;
        record_row(c, run, ROW_HISTORY_MOVED, id, Some(from))?;
    }
    let items: Vec<i64> = c
        .prepare_cached("SELECT id FROM playlist_items WHERE identity_id = ?1")?
        .query_map([from], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in items {
        c.prepare_cached("UPDATE playlist_items SET identity_id = ?2 WHERE id = ?1")?
            .execute(rusqlite::params![id, to])?;
        record_row(c, run, ROW_ITEM_MOVED, id, Some(from))?;
    }
    Ok(true)
}

/// Carries each pair of `pairs` as one run.
fn carry(
    c: &Connection,
    pairs: &[(TrackId, TrackId)],
    now: i64,
) -> rusqlite::Result<Result<i64, ImportError>> {
    let mut paths = Vec::with_capacity(pairs.len());
    for &(old, new) in pairs {
        let (Some((_, from_path)), Some((_, to_path))) = (identity(c, old)?, identity(c, new)?)
        else {
            return Ok(Err(ImportError::UnknownTrack));
        };
        paths.push((from_path, to_path));
    }
    let Some((from_path, to_path)) = paths.first() else {
        return Ok(Err(ImportError::UnknownTrack));
    };
    let source = match paths.len() {
        1 => format!("{from_path} → {to_path}"),
        n => format!("{from_path} → {to_path} (and {} more)", n - 1),
    };
    let run = start_run(c, ImportRunKind::Carry, &source, now)?;
    let mut carried = 0;
    for &(old, new) in pairs {
        if carry_into(c, run, old, new)? {
            carried += 1;
        }
    }
    let summary = super::ImportSummary {
        totals: super::PlanTotals {
            tracks: carried,
            ..super::PlanTotals::default()
        },
        ..super::ImportSummary::default()
    };
    set_summary(c, run, &summary)?;
    Ok(Ok(run))
}

impl Library {
    /// The missing tracks with statistics, a rating, history or playlist entries
    /// that another track of the same recording (the same title and artist,
    /// lengths within 2 s) could take over: a file replaced by another. A missing
    /// track with several such tracks is listed once for each.
    pub fn carry_candidates(&self) -> Result<Vec<CarryCandidate>, ImportError> {
        Ok(self.database().read(candidates)?)
    }

    /// Moves the statistics, rating, history and playlist entries of `old` to
    /// `new` as one import run (see [`Library::undo_import`]). The counts add up,
    /// the earliest first and the latest last play win, and `new` keeps its own
    /// rating when it has one.
    pub fn carry_stats(&self, old: TrackId, new: TrackId) -> Result<ImportRunId, ImportError> {
        self.carry_stats_of(&[(old, new)])
    }

    /// [`carry_stats`](Self::carry_stats) for each pair of a missing track and
    /// its replacement in `pairs`, as one import run that is undone as a whole.
    /// Fails with [`ImportError::UnknownTrack`], changing nothing, when a track
    /// is not in the library or `pairs` is empty.
    pub fn carry_stats_of(&self, pairs: &[(TrackId, TrackId)]) -> Result<ImportRunId, ImportError> {
        let now = to_unix_seconds(SystemTime::now());
        let pairs = pairs.to_vec();
        let run = self
            .database()
            .writer()?
            .write(move |c| carry(c, &pairs, now))??;
        Ok(ImportRunId(run))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_rating_wins_and_counts_add() {
        let old = Stats {
            play_count: 4,
            first_played: Some(10),
            last_played: Some(50),
            rating: Some(8),
            ..Stats::default()
        };
        let new = Stats {
            play_count: 1,
            first_played: Some(100),
            last_played: Some(100),
            ..Stats::default()
        };
        let both = carried(&old, &new);
        assert_eq!(both.play_count, 5);
        assert_eq!(both.first_played, Some(10));
        assert_eq!(both.last_played, Some(100));
        assert_eq!(both.rating, Some(8));
        let rated = Stats {
            rating: Some(2),
            ..new
        };
        assert_eq!(carried(&old, &rated).rating, Some(2));
    }
}
