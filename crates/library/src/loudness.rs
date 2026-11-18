// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Loudness measured by the loudness scanner, kept apart from the values in the
//! tags (which the scan stores in the track rows).
//!
//! A measurement belongs to a track and records the file's stamp (size and
//! modification time) and the track's span when it was taken. While both still
//! match, it is current; a file or CUE sheet that changed since makes it stale: it
//! counts as unmeasured, and playback falls back to the tags. Writing the
//! measured values into the file's tags changes its stamp, so the caller restamps
//! the measurements afterwards ([`Library::restamp_loudness`]).
//!
//! Gains are derived from the stored loudness against the ReplayGain 2.0
//! reference of −18 LUFS ([`REFERENCE_LUFS`]); peaks are true peaks.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use helikodu_model::library::TrackId;
use helikodu_model::player::Loudness;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::Library;
use crate::catalog::{AlbumId, TrackSection, sql_track_id, track_id_from_sql};
use crate::db::unix_time::{from_unix_seconds, to_unix_seconds};
use crate::db::{DbError, path_from_blob};
use crate::track_source::{TrackSource, span_of};

/// The ReplayGain 2.0 reference loudness, in LUFS.
pub const REFERENCE_LUFS: f64 = -18.0;

/// The values of an album measured as one programme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlbumMeasurement {
    /// The integrated loudness in LUFS; `None` for silence.
    pub integrated_lufs: Option<f64>,
    /// The loudness range in LU.
    pub range_lu: f64,
    /// The largest sample peak, linear.
    pub sample_peak: f64,
    /// The largest true peak, linear.
    pub true_peak: f64,
}

impl AlbumMeasurement {
    /// The ReplayGain 2.0 album gain in dB; `None` for silence.
    pub fn gain_db(&self) -> Option<f64> {
        self.integrated_lufs.map(|lufs| REFERENCE_LUFS - lufs)
    }
}

/// The measured loudness of one track, as the scanner hands it to the library.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasuredLoudness {
    /// The track.
    pub track: TrackId,
    /// The version of the measurement.
    pub version: u32,
    /// The integrated loudness in LUFS; `None` for silence.
    pub integrated_lufs: Option<f64>,
    /// The loudness range in LU.
    pub range_lu: f64,
    /// The sample peak, linear.
    pub sample_peak: f64,
    /// The true peak, linear.
    pub true_peak: f64,
    /// The album the track was measured with; `None` when measured on its own.
    pub album: Option<AlbumMeasurement>,
}

impl MeasuredLoudness {
    /// The ReplayGain 2.0 track gain in dB; `None` for silence.
    pub fn track_gain_db(&self) -> Option<f64> {
        self.integrated_lufs.map(|lufs| REFERENCE_LUFS - lufs)
    }

    /// The values as playback takes them: gains relative to −18 LUFS, true peaks.
    pub fn playback_values(&self) -> Loudness {
        let album = self.album.as_ref();
        Loudness {
            track_gain_db: self.track_gain_db(),
            track_peak: Some(self.true_peak),
            album_gain_db: album.and_then(AlbumMeasurement::gain_db),
            album_peak: album.map(|a| a.true_peak),
        }
    }

    /// The values playback uses when the file's tags say `tags`: the measured
    /// track values, and the measured album values when the track was measured
    /// with its album; the tags fill what was not measured.
    pub fn over_tags(&self, tags: Loudness) -> Loudness {
        let measured = self.playback_values();
        let track = measured.track_gain_db.is_some();
        let album = measured.album_gain_db.is_some();
        Loudness {
            track_gain_db: if track {
                measured.track_gain_db
            } else {
                tags.track_gain_db
            },
            track_peak: if track {
                measured.track_peak
            } else {
                tags.track_peak
            },
            album_gain_db: if album {
                measured.album_gain_db
            } else {
                tags.album_gain_db
            },
            album_peak: if album {
                measured.album_peak
            } else {
                tags.album_peak
            },
        }
    }
}

/// A measurement as the library keeps it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoredLoudness {
    /// The values.
    pub measured: MeasuredLoudness,
    /// When they were measured (to the second).
    pub measured_at: SystemTime,
    /// Whether the file and the track's span are as they were when measured.
    pub current: bool,
}

/// Which tracks to measure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoudnessSelection {
    /// These tracks. Album values are measured for an album only when all of its
    /// tracks are among them.
    Tracks(Vec<TrackId>),
    /// Every track of these albums, with album values.
    Albums(Vec<AlbumId>),
    /// The tracks without a current measurement of `version` or newer, and with
    /// them the other tracks of their albums, so that the album values cover the
    /// whole album.
    Unmeasured {
        /// The version of the measurement that counts as current.
        version: u32,
    },
}

/// A track to measure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoudnessTarget {
    /// The track.
    pub track: TrackId,
    /// Its file and span.
    pub source: TrackSource,
    /// Its album.
    pub album: Option<AlbumId>,
    /// The number of tracks of its album in the library (with files present);
    /// 0 without an album.
    pub album_size: usize,
    /// Its codec, as the scan read it.
    pub codec: Option<String>,
    /// Whether it is the whole file (not a CUE sheet track or a chapter).
    pub whole_file: bool,
}

/// How many tracks have loudness values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoudnessCoverage {
    /// The tracks whose files are present.
    pub tracks: usize,
    /// Those with a gain from their tags or from a current measurement.
    pub with_values: usize,
    /// Those with a current measurement.
    pub measured: usize,
}

/// The SQL condition that a measurement `l` of track `t` in file `f` is current.
const CURRENT: &str = "(l.file_size IS f.size AND l.file_mtime_ns IS f.mtime_ns
     AND l.start_frame IS t.start_frame AND l.end_frame IS t.end_frame)";

impl Library {
    /// Stores measurements, replacing earlier ones of the same tracks, with the
    /// current stamps of their files. Measurements of tracks that are no longer
    /// in the library are left out.
    pub fn store_loudness(&self, measurements: Vec<MeasuredLoudness>) -> Result<(), DbError> {
        let now = to_unix_seconds(SystemTime::now());
        self.database()
            .writer()?
            .write(move |c| store(c, &measurements, now))
    }

    /// Takes the current stamps of the files of `tracks` into their
    /// measurements, after their tags were written and the files read again.
    pub fn restamp_loudness(&self, tracks: Vec<TrackId>) -> Result<(), DbError> {
        self.database().writer()?.write(move |c| {
            let mut update = c.prepare_cached(
                "UPDATE loudness SET
                     file_size = (SELECT f.size FROM tracks t JOIN files f ON f.id = t.file_id
                                  WHERE t.id = loudness.track_id),
                     file_mtime_ns = (SELECT f.mtime_ns FROM tracks t JOIN files f ON f.id = t.file_id
                                      WHERE t.id = loudness.track_id)
                 WHERE track_id = ?1",
            )?;
            for track in &tracks {
                update.execute([sql_track_id(*track)])?;
            }
            Ok(())
        })
    }

    /// The measurement of `track`, current or not; `None` if it was never measured.
    pub fn measured_loudness(&self, track: TrackId) -> Result<Option<StoredLoudness>, DbError> {
        self.database().read(|c| {
            c.prepare_cached(&format!(
                "SELECT {COLUMNS}, {CURRENT}
                 FROM loudness l
                 JOIN tracks t ON t.id = l.track_id
                 JOIN files f ON f.id = t.file_id
                 WHERE l.track_id = ?1"
            ))?
            .query_row([sql_track_id(track)], stored_from_row)
            .optional()
        })
    }

    /// The tracks to measure for `selection`, in album order (albums by id, then
    /// disc and track number), tracks without an album last. Tracks whose files
    /// are missing are left out.
    pub fn loudness_targets(
        &self,
        selection: &LoudnessSelection,
    ) -> Result<Vec<LoudnessTarget>, DbError> {
        let selection = selection.clone();
        self.database().read(move |c| targets(c, &selection))
    }

    /// How many tracks have loudness values, from tags or measured.
    pub fn loudness_coverage(&self) -> Result<LoudnessCoverage, DbError> {
        self.database().read(|c| {
            c.query_row(
                &format!(
                    "SELECT count(*),
                            count(CASE WHEN t.rg_track_gain IS NOT NULL
                                         OR t.rg_album_gain IS NOT NULL
                                         OR (l.track_id IS NOT NULL AND {CURRENT}
                                             AND l.integrated IS NOT NULL)
                                       THEN 1 END),
                            count(CASE WHEN l.track_id IS NOT NULL AND {CURRENT} THEN 1 END)
                     FROM tracks t
                     JOIN files f ON f.id = t.file_id
                     LEFT JOIN loudness l ON l.track_id = t.id
                     WHERE f.missing_since IS NULL"
                ),
                [],
                |row| {
                    let count = |i| {
                        row.get::<_, i64>(i)
                            .map(|n| usize::try_from(n).unwrap_or(0))
                    };
                    Ok(LoudnessCoverage {
                        tracks: count(0)?,
                        with_values: count(1)?,
                        measured: count(2)?,
                    })
                },
            )
        })
    }
}

const COLUMNS: &str = "l.track_id, l.version, l.measured_at, l.integrated, l.loudness_range,
     l.sample_peak, l.true_peak, l.album_measured, l.album_integrated,
     l.album_loudness_range, l.album_sample_peak, l.album_true_peak";

fn stored_from_row(row: &Row<'_>) -> rusqlite::Result<StoredLoudness> {
    let album = if row.get::<_, bool>(7)? {
        Some(AlbumMeasurement {
            integrated_lufs: row.get(8)?,
            range_lu: row.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
            sample_peak: row.get::<_, Option<f64>>(10)?.unwrap_or(0.0),
            true_peak: row.get::<_, Option<f64>>(11)?.unwrap_or(0.0),
        })
    } else {
        None
    };
    Ok(StoredLoudness {
        measured: MeasuredLoudness {
            track: track_id_from_sql(row.get(0)?),
            version: u32::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
            integrated_lufs: row.get(3)?,
            range_lu: row.get(4)?,
            sample_peak: row.get(5)?,
            true_peak: row.get(6)?,
            album,
        },
        measured_at: from_unix_seconds(row.get(2)?),
        current: row.get(12)?,
    })
}

fn store(c: &Connection, measurements: &[MeasuredLoudness], now: i64) -> rusqlite::Result<()> {
    let mut insert = c.prepare_cached(
        "INSERT OR REPLACE INTO loudness
             (track_id, version, measured_at, file_size, file_mtime_ns, start_frame, end_frame,
              integrated, loudness_range, sample_peak, true_peak, album_measured,
              album_integrated, album_loudness_range, album_sample_peak, album_true_peak)
         SELECT t.id, ?2, ?3, f.size, f.mtime_ns, t.start_frame, t.end_frame,
                ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12
         FROM tracks t JOIN files f ON f.id = t.file_id
         WHERE t.id = ?1",
    )?;
    for m in measurements {
        let album = m.album.as_ref();
        insert.execute(params![
            sql_track_id(m.track),
            i64::from(m.version),
            now,
            finite(m.integrated_lufs),
            m.range_lu,
            m.sample_peak,
            m.true_peak,
            album.is_some(),
            album.and_then(|a| finite(a.integrated_lufs)),
            album.map(|a| a.range_lu),
            album.map(|a| a.sample_peak),
            album.map(|a| a.true_peak),
        ])?;
    }
    Ok(())
}

/// `value` when it is a finite number.
fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite())
}

fn targets(c: &Connection, selection: &LoudnessSelection) -> rusqlite::Result<Vec<LoudnessTarget>> {
    let present = "f.missing_since IS NULL";
    let ids: Vec<i64> = match selection {
        LoudnessSelection::Tracks(tracks) => {
            let mut ids: Vec<i64> = tracks.iter().map(|t| sql_track_id(*t)).collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        }
        LoudnessSelection::Albums(albums) => {
            let mut statement = c.prepare_cached(
                "SELECT t.id FROM tracks t JOIN files f ON f.id = t.file_id
                 WHERE t.album_id = ?1 AND f.missing_since IS NULL",
            )?;
            let mut ids = Vec::new();
            for album in albums {
                let rows = statement.query_map([album.get()], |row| row.get::<_, i64>(0))?;
                for id in rows {
                    ids.push(id?);
                }
            }
            ids
        }
        LoudnessSelection::Unmeasured { version } => {
            let mut statement = c.prepare(&format!(
                "SELECT t.id, t.album_id
                 FROM tracks t
                 JOIN files f ON f.id = t.file_id
                 LEFT JOIN loudness l ON l.track_id = t.id
                 WHERE {present}
                   AND (l.track_id IS NULL OR l.version < ?1 OR NOT {CURRENT})"
            ))?;
            let mut ids = BTreeSet::new();
            let mut albums = BTreeSet::new();
            let rows = statement.query_map([i64::from(*version)], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
            })?;
            for row in rows {
                let (id, album) = row?;
                ids.insert(id);
                albums.extend(album);
            }
            let mut statement = c.prepare_cached(
                "SELECT t.id FROM tracks t JOIN files f ON f.id = t.file_id
                 WHERE t.album_id = ?1 AND f.missing_since IS NULL",
            )?;
            for album in albums {
                let rows = statement.query_map([album], |row| row.get::<_, i64>(0))?;
                for id in rows {
                    ids.insert(id?);
                }
            }
            ids.into_iter().collect()
        }
    };

    let mut statement = c.prepare_cached(&format!(
        "SELECT t.id, d.path, f.name, t.section, t.start_frame, t.end_frame, t.sample_rate,
                t.album_id, t.codec, t.disc_no, t.track_no,
                (SELECT count(*) FROM tracks a JOIN files af ON af.id = a.file_id
                 WHERE a.album_id = t.album_id AND af.missing_since IS NULL)
         FROM tracks t
         JOIN files f ON f.id = t.file_id
         JOIN dirs d ON d.id = f.dir_id
         WHERE t.id = ?1 AND {present}"
    ))?;
    // Albums in id order, then disc and track; tracks without an album last.
    type Key = (bool, i64, i64, i64, i64);
    let mut found: BTreeMap<Key, LoudnessTarget> = BTreeMap::new();
    for id in ids {
        let row = statement
            .query_row([id], |row| {
                let dir = path_from_blob(row.get(1)?);
                let name = path_from_blob(row.get(2)?);
                let section = TrackSection::from_sql(row.get(3)?);
                let span = (row.get(4)?, row.get(5)?, row.get(6)?);
                let album: Option<i64> = row.get(7)?;
                let key: Key = (
                    album.is_none(),
                    album.unwrap_or(0),
                    row.get::<_, Option<i64>>(9)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(10)?.unwrap_or(0),
                    id,
                );
                let target = LoudnessTarget {
                    track: track_id_from_sql(row.get(0)?),
                    source: TrackSource {
                        path: dir.join(name),
                        section: span_of(section, span),
                    },
                    album: album.map(AlbumId),
                    album_size: if album.is_some() {
                        usize::try_from(row.get::<_, i64>(11)?).unwrap_or(0)
                    } else {
                        0
                    },
                    codec: row.get(8)?,
                    whole_file: section == TrackSection::Whole,
                };
                Ok((key, target))
            })
            .optional()?;
        if let Some((key, target)) = row {
            found.insert(key, target);
        }
    }
    Ok(found.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(album: Option<AlbumMeasurement>) -> MeasuredLoudness {
        MeasuredLoudness {
            track: TrackId::new(1),
            version: 1,
            integrated_lufs: Some(-11.0),
            range_lu: 4.0,
            sample_peak: 0.9,
            true_peak: 1.05,
            album,
        }
    }

    #[test]
    fn measured_values_win_over_tags() {
        let tags = Loudness {
            track_gain_db: Some(-3.0),
            track_peak: Some(0.5),
            album_gain_db: Some(-4.0),
            album_peak: Some(0.6),
        };
        let alone = measured(None).over_tags(tags);
        assert_eq!(alone.track_gain_db, Some(-7.0));
        assert_eq!(alone.track_peak, Some(1.05));
        assert_eq!(alone.album_gain_db, Some(-4.0));
        assert_eq!(alone.album_peak, Some(0.6));

        let album = AlbumMeasurement {
            integrated_lufs: Some(-12.5),
            range_lu: 6.0,
            sample_peak: 0.95,
            true_peak: 1.1,
        };
        let with_album = measured(Some(album)).over_tags(tags);
        assert_eq!(with_album.album_gain_db, Some(-5.5));
        assert_eq!(with_album.album_peak, Some(1.1));

        let silent = MeasuredLoudness {
            integrated_lufs: None,
            ..measured(None)
        };
        assert_eq!(silent.over_tags(tags), tags);
    }
}
