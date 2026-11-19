// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The merge rules of an import, as pure functions of the values.

use rusqlite::{Connection, OptionalExtension, params};

/// The statistics of an identity as the `stats` table holds them (times in
/// seconds since the epoch, the rating in half stars).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Stats {
    pub(super) play_count: i64,
    pub(super) skip_count: i64,
    pub(super) first_played: Option<i64>,
    pub(super) last_played: Option<i64>,
    pub(super) last_skipped: Option<i64>,
    pub(super) rating: Option<i64>,
    pub(super) loved: Option<i64>,
}

/// What an import brings for one identity: the entries that matched it, taken
/// together.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Incoming {
    pub(super) play_count: i64,
    pub(super) skip_count: i64,
    pub(super) first_played: Option<i64>,
    pub(super) last_played: Option<i64>,
    pub(super) last_skipped: Option<i64>,
    pub(super) rating: Option<i64>,
    pub(super) loved: bool,
    pub(super) added: Option<i64>,
}

impl Incoming {
    /// Adds the values of another entry that matched the same identity: counts
    /// add up, the earliest first and the latest last times, the highest rating.
    pub(super) fn combine(&mut self, other: &Incoming) {
        self.play_count = self.play_count.saturating_add(other.play_count);
        self.skip_count = self.skip_count.saturating_add(other.skip_count);
        self.first_played = min(self.first_played, other.first_played);
        self.last_played = self.last_played.max(other.last_played);
        self.last_skipped = self.last_skipped.max(other.last_skipped);
        self.rating = self.rating.max(other.rating);
        self.loved |= other.loved;
        self.added = min(self.added, other.added);
    }
}

/// How counts are merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CountRule {
    /// Add what the source counted since it was last imported (all of it the
    /// first time).
    AddNew,
    /// Take the higher count.
    Max,
}

/// The counts a source gave when it was last imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Baseline {
    pub(super) play_count: i64,
    pub(super) skip_count: i64,
}

fn min(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// What the source added to a count since `baseline`; nothing when its count
/// went down.
fn new_since(count: i64, baseline: Option<i64>) -> i64 {
    (count - baseline.unwrap_or(0)).max(0)
}

/// The statistics after merging `incoming` into `current`.
pub(super) fn merge(
    current: &Stats,
    incoming: &Incoming,
    rule: CountRule,
    baseline: Option<Baseline>,
    replace_ratings: bool,
) -> Stats {
    let (play_count, skip_count) = match rule {
        CountRule::AddNew => (
            current.play_count.saturating_add(new_since(
                incoming.play_count,
                baseline.map(|b| b.play_count),
            )),
            current.skip_count.saturating_add(new_since(
                incoming.skip_count,
                baseline.map(|b| b.skip_count),
            )),
        ),
        CountRule::Max => (
            current.play_count.max(incoming.play_count),
            current.skip_count.max(incoming.skip_count),
        ),
    };
    let rating = match incoming.rating {
        Some(r) if replace_ratings || current.rating.is_none() => Some(r),
        _ => current.rating,
    };
    // A last play is a play: the first play was no later.
    let first = min(incoming.first_played, incoming.last_played);
    Stats {
        play_count,
        skip_count,
        first_played: min(current.first_played, first),
        last_played: current.last_played.max(incoming.last_played),
        last_skipped: current.last_skipped.max(incoming.last_skipped),
        rating,
        loved: if incoming.loved && current.loved.is_none() {
            Some(1)
        } else {
            current.loved
        },
    }
}

/// The statistics after undoing a run that changed them from `before` to
/// `after`, from `current`: the counts lose what the run added (plays made since
/// stay), every other value goes back where it still holds what the run set.
pub(super) fn undo(current: &Stats, before: &Stats, after: &Stats) -> Stats {
    fn back<T: PartialEq + Copy>(current: T, before: T, after: T) -> T {
        if current == after { before } else { current }
    }
    Stats {
        play_count: (current.play_count - (after.play_count - before.play_count)).max(0),
        skip_count: (current.skip_count - (after.skip_count - before.skip_count)).max(0),
        first_played: back(
            current.first_played,
            before.first_played,
            after.first_played,
        ),
        last_played: back(current.last_played, before.last_played, after.last_played),
        last_skipped: back(
            current.last_skipped,
            before.last_skipped,
            after.last_skipped,
        ),
        rating: back(current.rating, before.rating, after.rating),
        loved: back(current.loved, before.loved, after.loved),
    }
}

/// The statistics of `identity`, and whether it has a row.
pub(super) fn read(c: &Connection, identity: i64) -> rusqlite::Result<(Stats, bool)> {
    let row = c
        .prepare_cached(
            "SELECT play_count, skip_count, first_played, last_played, last_skipped, rating,
                    loved
             FROM stats WHERE identity_id = ?1",
        )?
        .query_row([identity], |row| {
            Ok(Stats {
                play_count: row.get(0)?,
                skip_count: row.get(1)?,
                first_played: row.get(2)?,
                last_played: row.get(3)?,
                last_skipped: row.get(4)?,
                rating: row.get(5)?,
                loved: row.get(6)?,
            })
        })
        .optional()?;
    Ok(match row {
        Some(stats) => (stats, true),
        None => (Stats::default(), false),
    })
}

/// Writes `stats` for `identity`; `rating_changed` sets when it was rated.
pub(super) fn write(
    c: &Connection,
    identity: i64,
    stats: &Stats,
    rating_changed: bool,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO stats (identity_id, play_count, skip_count, first_played, last_played,
                            last_skipped, rating, loved)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (identity_id) DO UPDATE SET
             play_count = excluded.play_count, skip_count = excluded.skip_count,
             first_played = excluded.first_played, last_played = excluded.last_played,
             last_skipped = excluded.last_skipped, rating = excluded.rating,
             loved = excluded.loved",
    )?
    .execute(params![
        identity,
        stats.play_count,
        stats.skip_count,
        stats.first_played,
        stats.last_played,
        stats.last_skipped,
        stats.rating,
        stats.loved
    ])?;
    if rating_changed {
        c.prepare_cached("UPDATE stats SET rated_at = unixepoch() WHERE identity_id = ?1")?
            .execute([identity])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ours() -> Stats {
        Stats {
            play_count: 3,
            skip_count: 1,
            first_played: Some(500),
            last_played: Some(900),
            rating: Some(6),
            ..Stats::default()
        }
    }

    fn theirs() -> Incoming {
        Incoming {
            play_count: 10,
            skip_count: 2,
            last_played: Some(400),
            rating: Some(10),
            loved: true,
            ..Incoming::default()
        }
    }

    #[test]
    fn counts_add_and_times_spread() {
        let merged = merge(&ours(), &theirs(), CountRule::AddNew, None, false);
        assert_eq!(merged.play_count, 13);
        assert_eq!(merged.skip_count, 3);
        // Their last play is earlier than our first.
        assert_eq!(merged.first_played, Some(400));
        assert_eq!(merged.last_played, Some(900));
        // Our rating stays unless asked.
        assert_eq!(merged.rating, Some(6));
        assert_eq!(merged.loved, Some(1));
        let replaced = merge(&ours(), &theirs(), CountRule::AddNew, None, true);
        assert_eq!(replaced.rating, Some(10));
        let unrated = Stats {
            rating: None,
            ..ours()
        };
        assert_eq!(
            merge(&unrated, &theirs(), CountRule::AddNew, None, false).rating,
            Some(10)
        );
    }

    #[test]
    fn a_baseline_adds_only_new_plays() {
        let baseline = Baseline {
            play_count: 8,
            skip_count: 2,
        };
        let merged = merge(&ours(), &theirs(), CountRule::AddNew, Some(baseline), false);
        assert_eq!(merged.play_count, 5);
        assert_eq!(merged.skip_count, 1);
        // A source whose count went down adds nothing.
        let reset = Baseline {
            play_count: 50,
            skip_count: 50,
        };
        let merged = merge(&ours(), &theirs(), CountRule::AddNew, Some(reset), false);
        assert_eq!(merged.play_count, 3);
    }

    #[test]
    fn the_higher_count_for_own_data() {
        let merged = merge(&ours(), &theirs(), CountRule::Max, None, false);
        assert_eq!(merged.play_count, 10);
        assert_eq!(merged.skip_count, 2);
    }

    #[test]
    fn undo_keeps_what_came_since() {
        let before = ours();
        let after = merge(&before, &theirs(), CountRule::AddNew, None, true);
        // Two plays since the import.
        let current = Stats {
            play_count: after.play_count + 2,
            last_played: Some(2000),
            ..after
        };
        let undone = undo(&current, &before, &after);
        assert_eq!(undone.play_count, 5);
        assert_eq!(undone.last_played, Some(2000));
        assert_eq!(undone.rating, Some(6));
        assert_eq!(undone.first_played, Some(500));
        assert_eq!(undone.loved, None);
    }
}
