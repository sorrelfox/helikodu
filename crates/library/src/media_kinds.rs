// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Media kinds, release years and the sections of a file.
//!
//! - The owner can give tracks another kind (music, audiobook, audio from a video
//!   file). The choice belongs to the track's identity and survives rescans.
//! - The years of the library, for browsing by year and decade.
//! - The sections of a file with chapters or a CUE sheet, for showing the whole file
//!   while one of its parts plays.

use std::time::Duration;

use helikodu_model::library::TrackId;
use rusqlite::{Connection, OptionalExtension, params};

use crate::Library;
use crate::catalog::{self, TrackKind, TrackSection, YearQuery, YearSummary, sql_track_id};
use crate::catalog::{track_id_from_sql, write::identity_of};
use crate::db::DbError;

/// One section of a file: a chapter or a CUE sheet track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSection {
    /// The track.
    pub id: TrackId,
    /// Where it starts in the file.
    pub start: Duration,
    /// Its length, when known.
    pub length: Option<Duration>,
    /// Its title.
    pub title: Option<String>,
    /// Its number in the file (the chapter or CUE track number).
    pub number: u32,
}

impl Library {
    /// Every release year with albums or tracks, newest first, then the tracks
    /// without a year.
    pub fn years(&self, query: &YearQuery) -> Result<Vec<YearSummary>, DbError> {
        self.database().read(|c| catalog::years(c, query))
    }

    /// Gives `tracks` the kind `kind`, chosen by the owner. The choice is kept for
    /// their identities: it applies to their copies too and survives rescans.
    /// Tracks that are not in the library are passed over.
    pub fn set_kind(&self, tracks: &[TrackId], kind: TrackKind) -> Result<(), DbError> {
        let tracks = tracks.to_vec();
        self.database()
            .writer()?
            .write(move |c| set_kind(c, &tracks, kind))
    }

    /// The sections of the file of `track` in file order, when `track` is a chapter
    /// or a CUE sheet track; empty for a whole file or a subsong.
    pub fn file_sections(&self, track: TrackId) -> Result<Vec<FileSection>, DbError> {
        self.database().read(move |c| file_sections(c, track))
    }
}

fn set_kind(c: &Connection, tracks: &[TrackId], kind: TrackKind) -> rusqlite::Result<()> {
    for &track in tracks {
        let Some(identity) = identity_of(c, track)? else {
            continue;
        };
        c.prepare_cached(
            "INSERT INTO chosen_kinds (identity_id, kind) VALUES (?1, ?2)
             ON CONFLICT (identity_id) DO UPDATE SET kind = excluded.kind",
        )?
        .execute(params![identity.0, kind.to_sql()])?;
        c.prepare_cached("UPDATE tracks SET kind = ?2 WHERE identity_id = ?1")?
            .execute(params![identity.0, kind.to_sql()])?;
    }
    Ok(())
}

fn file_sections(c: &Connection, track: TrackId) -> rusqlite::Result<Vec<FileSection>> {
    let found: Option<(i64, i64)> = c
        .prepare_cached("SELECT file_id, section FROM tracks WHERE id = ?1")?
        .query_row([sql_track_id(track)], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((file, section)) = found else {
        return Ok(Vec::new());
    };
    let section = TrackSection::from_sql(section);
    if !matches!(section, TrackSection::Chapter | TrackSection::CueSheet) {
        return Ok(Vec::new());
    }
    c.prepare_cached(
        "SELECT id, start_frame, sample_rate, duration_ms, title, sub_index FROM tracks
         WHERE file_id = ?1 AND section = ?2 ORDER BY sub_index, id",
    )?
    .query_map(params![file, section.to_sql()], |row| {
        let start: Option<i64> = row.get(1)?;
        let rate: Option<i64> = row.get(2)?;
        let start = match (start, rate) {
            (Some(frames), Some(rate)) if rate > 0 && frames >= 0 => {
                Duration::from_secs_f64(frames as f64 / rate as f64)
            }
            _ => Duration::ZERO,
        };
        Ok(FileSection {
            id: track_id_from_sql(row.get(0)?),
            start,
            length: row
                .get::<_, Option<i64>>(3)?
                .and_then(|ms| u64::try_from(ms).ok())
                .map(Duration::from_millis),
            title: row.get(4)?,
            number: row.get(5)?,
        })
    })?
    .collect()
}
