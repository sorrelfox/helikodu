// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Play counts and ratings written into the files, in batches.
//!
//! Nothing is written when a track is played. The library keeps, per file, the
//! play count and rating last written into it (`stats_in_files`); a batch writes
//! the files whose values in the library differ from those, through the same
//! path as tag edits (a new file written, read back and checked, then put in
//! place), and reads them again. Only whole-file tracks are written: a CUE sheet
//! track or a chapter shares its file with other tracks. These writes are not on
//! the undo history of tag edits: the library holds the values, and the files
//! follow it.
//!
//! Written: the rating as `POPM`, `FMPS_RATING` and `RATING` (see `tag_writes`);
//! the play count as the counter of the `POPM` frame and `FMPS_PLAYCOUNT`.

use std::path::PathBuf;

use helikodu_meta::tags::writing::{BatchItem, TagChanges, WriteError};
use rusqlite::{Connection, params};

use crate::Library;
use crate::db::{DbError, path_from_blob};
use crate::scan::{CancelToken, ScanError, ScanOptions};
use crate::tag_writes::TagWriteSettings;

/// Which statistics go into the files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatsToFiles {
    /// Ratings.
    pub ratings: bool,
    /// Play counts.
    pub play_counts: bool,
}

impl StatsToFiles {
    /// Whether anything goes into the files.
    pub fn any(self) -> bool {
        self.ratings || self.play_counts
    }
}

/// What a batch of [`Library::write_stats_to_files`] did.
#[derive(Debug, Default)]
pub struct StatsWriteReport {
    /// The files written.
    pub written: usize,
    /// The files that could not be written, and why. A file whose format holds
    /// no tags is not tried again until its values change.
    pub failed: Vec<(PathBuf, String)>,
    /// The files still to write after this batch.
    pub remaining: usize,
    /// The files left for later because they were to be skipped (they are
    /// counted in `remaining`).
    pub postponed: usize,
}

/// A file whose values differ from those last written.
#[derive(Debug)]
struct Pending {
    file: i64,
    path: PathBuf,
    play_count: i64,
    rating: Option<i64>,
}

/// The condition that a file needs writing, for `what`.
fn differs(what: StatsToFiles) -> String {
    let mut terms = Vec::new();
    if what.ratings {
        terms.push("coalesce(s.rating, -1) != coalesce(w.rating, -1)");
    }
    if what.play_counts {
        terms.push("coalesce(s.play_count, 0) != coalesce(w.play_count, 0)");
    }
    if terms.is_empty() {
        "0".to_owned()
    } else {
        terms.join(" OR ")
    }
}

const PENDING_FROM: &str = "FROM tracks t JOIN files f ON f.id = t.file_id
    JOIN dirs d ON d.id = f.dir_id
    LEFT JOIN stats s ON s.identity_id = t.identity_id
    LEFT JOIN stats_in_files w ON w.file_id = f.id
    WHERE t.sub_index = 0 AND f.missing_since IS NULL
      AND NOT EXISTS (SELECT 1 FROM tracks o WHERE o.file_id = f.id AND o.sub_index != 0)";

fn pending_count(c: &Connection, what: StatsToFiles) -> rusqlite::Result<usize> {
    let n: i64 = c.query_row(
        &format!("SELECT count(*) {PENDING_FROM} AND ({})", differs(what)),
        [],
        |row| row.get(0),
    )?;
    Ok(usize::try_from(n).unwrap_or(0))
}

fn pending(c: &Connection, what: StatsToFiles, limit: usize) -> rusqlite::Result<Vec<Pending>> {
    c.prepare(&format!(
        "SELECT f.id, d.path, f.name, coalesce(s.play_count, 0), s.rating {PENDING_FROM}
         AND ({}) ORDER BY f.id LIMIT ?1",
        differs(what)
    ))?
    .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
        Ok(Pending {
            file: row.get(0)?,
            path: path_from_blob(row.get(1)?).join(path_from_blob(row.get(2)?)),
            play_count: row.get(3)?,
            rating: row.get(4)?,
        })
    })?
    .collect()
}

/// Records what was written into `file`.
fn record(
    c: &Connection,
    file: i64,
    play_count: Option<i64>,
    rating: Option<Option<i64>>,
) -> rusqlite::Result<()> {
    c.prepare_cached("INSERT INTO stats_in_files (file_id) VALUES (?1) ON CONFLICT DO NOTHING")?
        .execute([file])?;
    if let Some(count) = play_count {
        c.prepare_cached("UPDATE stats_in_files SET play_count = ?2 WHERE file_id = ?1")?
            .execute(params![file, count])?;
    }
    if let Some(rating) = rating {
        c.prepare_cached("UPDATE stats_in_files SET rating = ?2 WHERE file_id = ?1")?
            .execute(params![file, rating])?;
    }
    Ok(())
}

/// Records that the rating of the file of `track` was written as `rating`.
pub(crate) fn record_rating(
    c: &Connection,
    track: i64,
    rating: Option<i64>,
) -> rusqlite::Result<()> {
    let file: Option<i64> = c
        .prepare_cached("SELECT file_id FROM tracks WHERE id = ?1")?
        .query_row([track], |row| row.get(0))
        .ok();
    match file {
        Some(file) => record(c, file, None, Some(rating)),
        None => Ok(()),
    }
}

/// Whether a failed write would fail again however often it is tried.
fn lasting(error: &WriteError) -> bool {
    matches!(
        error,
        WriteError::ReadOnlyFormat | WriteError::Parse(_) | WriteError::InvalidValue { .. }
    )
}

impl Library {
    /// How many files have a play count or rating (as `what` chooses) that differs
    /// from what was last written into them.
    pub fn pending_stats_writes(&self, what: StatsToFiles) -> Result<usize, DbError> {
        self.database().read(move |c| pending_count(c, what))
    }

    /// Writes the play counts and ratings (as `what` chooses) of up to `limit`
    /// files whose values differ from what was last written into them, then reads
    /// the written files again. The files in `skip` (such as the one playing) are
    /// left for a later batch. Meant to run now and then (when the application
    /// is idle, or before it quits), not after every play.
    pub fn write_stats_to_files(
        &self,
        what: StatsToFiles,
        settings: &TagWriteSettings,
        scan: &ScanOptions,
        limit: usize,
        skip: &[PathBuf],
        cancel: &CancelToken,
    ) -> Result<StatsWriteReport, ScanError> {
        let mut report = StatsWriteReport::default();
        if !what.any() || limit == 0 {
            return Ok(report);
        }
        let wanted = limit.saturating_add(skip.len());
        let mut files = self.database().read(move |c| pending(c, what, wanted))?;
        let before = files.len();
        files.retain(|p| !skip.contains(&p.path));
        report.postponed = before - files.len();
        files.truncate(limit);
        let items: Vec<BatchItem> = files
            .iter()
            .map(|p| {
                let mut changes = TagChanges::new();
                if what.ratings {
                    changes.rating = Some(
                        p.rating
                            .and_then(|r| u8::try_from(r).ok())
                            .and_then(helikodu_meta::tags::Rating::from_half_stars),
                    );
                }
                if what.play_counts {
                    changes.play_count = Some(u64::try_from(p.play_count).ok().filter(|&n| n > 0));
                }
                BatchItem {
                    path: p.path.clone(),
                    expected: None,
                    changes,
                }
            })
            .collect();
        let written = self.write_tags(&items, settings, scan, &mut |_| {}, cancel)?;
        let mut done: Vec<(i64, Option<i64>, Option<Option<i64>>)> = Vec::new();
        for (file, (path, result)) in files.iter().zip(&written.batch.results) {
            let values = (
                file.file,
                what.play_counts.then_some(file.play_count),
                what.ratings.then_some(file.rating),
            );
            match result {
                Ok(_) => {
                    report.written += 1;
                    done.push(values);
                }
                Err(error) => {
                    if lasting(error) {
                        done.push(values);
                    }
                    report.failed.push((path.clone(), error.to_string()));
                }
            }
        }
        self.database().writer()?.write(move |c| {
            for (file, plays, rating) in done {
                record(c, file, plays, rating)?;
            }
            Ok(())
        })?;
        report.remaining = self.pending_stats_writes(what)?;
        Ok(report)
    }
}
