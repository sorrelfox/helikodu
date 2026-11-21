// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The track analysis as the library keeps it: where a track's sound starts and
//! ends, its silent stretches and its loudness envelope ([`TrackAnalysis`]), or
//! why it could not be analysed.
//!
//! Like a loudness measurement, an analysis records the file's stamp (size and
//! modification time) and the track's span when it was made. While both still
//! match, it is current; a file or CUE sheet that changed since makes the track
//! count as not analysed. An analysis of an older version counts as not analysed
//! either. A failure is kept the same way, so that a file that cannot be decoded is
//! not tried again until it changes.
//!
//! The work comes in units ([`Library::analysis_unit`]): a track, or the tracks of
//! its album that need it. When the loudness is measured in the same pass and no
//! track of the album has loudness values yet, the unit is the whole album, so
//! that the album is measured as one programme.

use std::time::SystemTime;

use helikodu_model::analysis::{
    LoudnessEnvelope, SoundFeatures, TrackAnalysis, silences_from_bytes, silences_to_bytes,
};
use helikodu_model::library::TrackId;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::Library;
use crate::catalog::{AlbumId, TrackSection, sql_track_id, track_id_from_sql};
use crate::db::unix_time::{from_unix_seconds, to_unix_seconds};
use crate::db::{DbError, path_from_blob};
use crate::track_source::{TrackSource, section_of};

/// What came of analysing a track.
#[derive(Debug, Clone, PartialEq)]
pub enum AnalysisOutcome {
    /// The analysis.
    Analyzed(TrackAnalysis),
    /// Why the track could not be analysed.
    Failed(String),
}

/// An analysis as the library keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredAnalysis {
    /// The track.
    pub track: TrackId,
    /// The version of the analysis.
    pub version: u32,
    /// When it was made (to the second).
    pub analyzed_at: SystemTime,
    /// The analysis, or why there is none.
    pub outcome: AnalysisOutcome,
    /// Whether the file and the track's span are as they were when it was made.
    pub current: bool,
}

/// What a background analysis looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisPlan {
    /// The version of the analysis that counts as current.
    pub version: u32,
    /// When the loudness is measured in the same pass: the version of the
    /// measurement that counts as current. Only tracks without loudness values
    /// (neither in their tags nor a current measurement) are measured.
    pub loudness_version: Option<u32>,
    /// The largest album measured as one programme; the loudness of a larger one
    /// is left to the loudness scanner.
    pub max_album_tracks: usize,
}

/// A track of an [`AnalysisUnit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisTarget {
    /// The track.
    pub track: TrackId,
    /// Its file and span.
    pub source: TrackSource,
    /// Whether it needs the analysis; `false` for a track that is decoded again
    /// only for the loudness of its album.
    pub needs_analysis: bool,
}

/// Tracks that are analysed together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisUnit {
    /// The album of the tracks, if any.
    pub album: Option<AlbumId>,
    /// The tracks, in album order.
    pub targets: Vec<AnalysisTarget>,
    /// Whether the loudness of the tracks is measured: of the whole album as one
    /// programme when `album` is set (the unit then holds all its tracks), of the
    /// lone track otherwise.
    pub measure_loudness: bool,
}

/// How many tracks are analysed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnalysisCoverage {
    /// The tracks whose files are present.
    pub tracks: usize,
    /// Those with a current analysis.
    pub analyzed: usize,
    /// Those whose current analysis failed.
    pub failed: usize,
}

impl AnalysisCoverage {
    /// The tracks still to analyse.
    pub fn remaining(&self) -> usize {
        self.tracks.saturating_sub(self.analyzed + self.failed)
    }
}

/// The SQL condition that the analysis `a` of track `t` in file `f` is current.
const CURRENT: &str = "(a.file_size IS f.size AND a.file_mtime_ns IS f.mtime_ns
     AND a.start_frame IS t.start_frame AND a.end_frame IS t.end_frame)";

/// The SQL condition that track `t` in file `f` needs an analysis of version `?1`.
const NEEDED: &str = "NOT EXISTS (SELECT 1 FROM track_analysis a
     WHERE a.track_id = t.id AND a.version >= ?1
       AND a.file_size IS f.size AND a.file_mtime_ns IS f.mtime_ns
       AND a.start_frame IS t.start_frame AND a.end_frame IS t.end_frame)";

/// The SQL condition that track `t` in file `f` has no loudness values: none in
/// its tags and no current measurement of version `?2` or newer.
const NO_LOUDNESS: &str = "(t.rg_track_gain IS NULL AND t.rg_album_gain IS NULL
     AND NOT EXISTS (SELECT 1 FROM loudness l
         WHERE l.track_id = t.id AND l.version >= ?2
           AND l.file_size IS f.size AND l.file_mtime_ns IS f.mtime_ns
           AND l.start_frame IS t.start_frame AND l.end_frame IS t.end_frame))";

impl Library {
    /// Stores what came of analysing `track` with version `version` of the
    /// analysis, replacing an earlier result, with the current stamp of its file.
    /// A track no longer in the library is left out.
    pub fn store_analysis(
        &self,
        track: TrackId,
        version: u32,
        outcome: AnalysisOutcome,
    ) -> Result<(), DbError> {
        let now = to_unix_seconds(SystemTime::now());
        self.database()
            .writer()?
            .write(move |c| store(c, track, version, &outcome, now))?;
        self.similarity_changed();
        Ok(())
    }

    /// The analysis of `track`, current or not; `None` if it was never analysed.
    pub fn track_analysis(&self, track: TrackId) -> Result<Option<StoredAnalysis>, DbError> {
        self.database().read(|c| {
            c.prepare_cached(&format!(
                "SELECT {COLUMNS}, {CURRENT}
                 FROM track_analysis a
                 JOIN tracks t ON t.id = a.track_id
                 JOIN files f ON f.id = t.file_id
                 WHERE a.track_id = ?1"
            ))?
            .query_row([sql_track_id(track)], stored_from_row)
            .optional()
        })
    }

    /// Up to `limit` tracks after `after` (in id order) that need an analysis of
    /// `version`: never analysed, analysed with an older version, or changed since.
    /// Tracks whose files are missing are left out.
    pub fn tracks_to_analyze(
        &self,
        version: u32,
        after: Option<TrackId>,
        limit: usize,
    ) -> Result<Vec<TrackId>, DbError> {
        let after = after.map_or(0, sql_track_id);
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.database().read(|c| {
            let mut statement = c.prepare_cached(&format!(
                "SELECT t.id FROM tracks t JOIN files f ON f.id = t.file_id
                 WHERE t.id > ?2 AND f.missing_since IS NULL AND {NEEDED}
                 ORDER BY t.id LIMIT ?3"
            ))?;
            let rows = statement.query_map(params![i64::from(version), after, limit], |row| {
                row.get::<_, i64>(0).map(track_id_from_sql)
            })?;
            rows.collect()
        })
    }

    /// The tracks to analyse together with `track` under `plan`, or `None` when
    /// it needs no analysis (or is gone, or its file is missing). See the module
    /// documentation.
    pub fn analysis_unit(
        &self,
        track: TrackId,
        plan: &AnalysisPlan,
    ) -> Result<Option<AnalysisUnit>, DbError> {
        let plan = *plan;
        self.database().read(move |c| unit(c, track, &plan))
    }

    /// How many tracks have a current analysis of `version`.
    pub fn analysis_coverage(&self, version: u32) -> Result<AnalysisCoverage, DbError> {
        self.database().read(|c| {
            c.query_row(
                &format!(
                    "SELECT count(*),
                            count(CASE WHEN a.version >= ?1 AND {CURRENT}
                                        AND a.failure IS NULL THEN 1 END),
                            count(CASE WHEN a.version >= ?1 AND {CURRENT}
                                        AND a.failure IS NOT NULL THEN 1 END)
                     FROM tracks t
                     JOIN files f ON f.id = t.file_id
                     LEFT JOIN track_analysis a ON a.track_id = t.id
                     WHERE f.missing_since IS NULL"
                ),
                [i64::from(version)],
                |row| {
                    let count = |i| {
                        row.get::<_, i64>(i)
                            .map(|n| usize::try_from(n).unwrap_or(0))
                    };
                    Ok(AnalysisCoverage {
                        tracks: count(0)?,
                        analyzed: count(1)?,
                        failed: count(2)?,
                    })
                },
            )
        })
    }
}

const COLUMNS: &str = "a.track_id, a.version, a.analyzed_at, a.failure, a.sample_rate, a.frames,
     a.audio_start, a.audio_end, a.fade_in_end, a.fade_out_start, a.integrated,
     a.head_loudness, a.tail_loudness, a.silences, a.envelope, a.features";

fn stored_from_row(row: &Row<'_>) -> rusqlite::Result<StoredAnalysis> {
    let frame = |i| -> rusqlite::Result<Option<u64>> {
        Ok(row
            .get::<_, Option<i64>>(i)?
            .and_then(|v| u64::try_from(v).ok()))
    };
    let outcome = match row.get::<_, Option<String>>(3)? {
        Some(failure) => AnalysisOutcome::Failed(failure),
        None => {
            let audio = match (frame(6)?, frame(7)?) {
                (Some(start), Some(end)) => Some(start..end),
                _ => None,
            };
            let silences: Option<Vec<u8>> = row.get(13)?;
            let envelope: Option<Vec<u8>> = row.get(14)?;
            let features: Option<Vec<u8>> = row.get(15)?;
            AnalysisOutcome::Analyzed(TrackAnalysis {
                rate: row
                    .get::<_, Option<i64>>(4)?
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or(0),
                frames: frame(5)?.unwrap_or(0),
                audio,
                silences: silences
                    .as_deref()
                    .map(silences_from_bytes)
                    .unwrap_or_default(),
                integrated_lufs: row.get(10)?,
                head_lufs: row.get(11)?,
                tail_lufs: row.get(12)?,
                fade_in_end: frame(8)?,
                fade_out_start: frame(9)?,
                envelope: envelope
                    .as_deref()
                    .and_then(LoudnessEnvelope::from_bytes)
                    .unwrap_or_default(),
                features: features
                    .as_deref()
                    .and_then(SoundFeatures::from_bytes)
                    .map(Box::new),
            })
        }
    };
    Ok(StoredAnalysis {
        track: track_id_from_sql(row.get(0)?),
        version: u32::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
        analyzed_at: from_unix_seconds(row.get(2)?),
        outcome,
        current: row.get(16)?,
    })
}

fn store(
    c: &Connection,
    track: TrackId,
    version: u32,
    outcome: &AnalysisOutcome,
    now: i64,
) -> rusqlite::Result<()> {
    let mut insert = c.prepare_cached(
        "INSERT OR REPLACE INTO track_analysis
             (track_id, version, analyzed_at, file_size, file_mtime_ns, start_frame, end_frame,
              failure, sample_rate, frames, audio_start, audio_end, fade_in_end, fade_out_start,
              integrated, head_loudness, tail_loudness, silences, envelope, features)
         SELECT t.id, ?2, ?3, f.size, f.mtime_ns, t.start_frame, t.end_frame,
                ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
         FROM tracks t JOIN files f ON f.id = t.file_id
         WHERE t.id = ?1",
    )?;
    let frame = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
    let finite = |v: Option<f64>| v.filter(|v| v.is_finite());
    match outcome {
        AnalysisOutcome::Failed(reason) => {
            insert.execute(params![
                sql_track_id(track),
                i64::from(version),
                now,
                reason,
                None::<i64>,
                None::<i64>,
                None::<i64>,
                None::<i64>,
                None::<i64>,
                None::<i64>,
                None::<f64>,
                None::<f64>,
                None::<f64>,
                None::<Vec<u8>>,
                None::<Vec<u8>>,
                None::<Vec<u8>>,
            ])?;
        }
        AnalysisOutcome::Analyzed(a) => {
            let silences = (!a.silences.is_empty()).then(|| silences_to_bytes(&a.silences));
            insert.execute(params![
                sql_track_id(track),
                i64::from(version),
                now,
                None::<String>,
                i64::from(a.rate),
                frame(a.frames),
                a.audio.as_ref().map(|r| frame(r.start)),
                a.audio.as_ref().map(|r| frame(r.end)),
                a.fade_in_end.map(frame),
                a.fade_out_start.map(frame),
                finite(a.integrated_lufs),
                finite(a.head_lufs),
                finite(a.tail_lufs),
                silences,
                a.envelope.to_bytes(),
                a.features.as_deref().map(SoundFeatures::to_bytes),
            ])?;
        }
    }
    Ok(())
}

/// A track's row as a unit needs it.
struct UnitRow {
    target: AnalysisTarget,
    album: Option<i64>,
    no_loudness: bool,
}

fn unit(
    c: &Connection,
    track: TrackId,
    plan: &AnalysisPlan,
) -> rusqlite::Result<Option<AnalysisUnit>> {
    let loudness_version = i64::from(plan.loudness_version.unwrap_or(u32::MAX));
    let select = format!(
        "SELECT t.id, d.path, f.name, t.section, t.start_frame, t.end_frame, t.sample_rate,
                t.sub_index, t.album_id, {NEEDED}, {NO_LOUDNESS}
         FROM tracks t
         JOIN files f ON f.id = t.file_id
         JOIN dirs d ON d.id = f.dir_id
         WHERE f.missing_since IS NULL"
    );
    let read = |row: &Row<'_>| -> rusqlite::Result<UnitRow> {
        let section = TrackSection::from_sql(row.get(3)?);
        let span = (row.get(4)?, row.get(5)?, row.get(6)?);
        Ok(UnitRow {
            target: AnalysisTarget {
                track: track_id_from_sql(row.get(0)?),
                source: TrackSource {
                    path: path_from_blob(row.get(1)?).join(path_from_blob(row.get(2)?)),
                    section: section_of(section, span, row.get(7)?),
                },
                needs_analysis: row.get(9)?,
            },
            album: row.get(8)?,
            no_loudness: row.get(10)?,
        })
    };
    let version = i64::from(plan.version);
    let first = c
        .prepare_cached(&format!("{select} AND t.id = ?3"))?
        .query_row(
            params![version, loudness_version, sql_track_id(track)],
            read,
        )
        .optional()?;
    let Some(first) = first.filter(|row| row.target.needs_analysis) else {
        return Ok(None);
    };
    let measure = plan.loudness_version.is_some();
    let Some(album) = first.album else {
        return Ok(Some(AnalysisUnit {
            album: None,
            measure_loudness: measure && first.no_loudness,
            targets: vec![first.target],
        }));
    };
    let mut statement = c.prepare_cached(&format!(
        "{select} AND t.album_id = ?3
         ORDER BY t.disc_no IS NULL, t.disc_no, t.track_no IS NULL, t.track_no, t.id"
    ))?;
    let rows: Vec<UnitRow> = statement
        .query_map(params![version, loudness_version, album], read)?
        .collect::<rusqlite::Result<_>>()?;
    let whole =
        measure && rows.len() <= plan.max_album_tracks && rows.iter().all(|row| row.no_loudness);
    let targets = rows
        .into_iter()
        .map(|row| row.target)
        .filter(|target| whole || target.needs_analysis)
        .collect();
    Ok(Some(AnalysisUnit {
        album: Some(AlbumId(album)),
        targets,
        measure_loudness: whole,
    }))
}
