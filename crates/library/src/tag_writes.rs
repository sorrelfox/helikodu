// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tag edits and ratings: files written through `meta`, then read again at once so
//! that the library shows the new tags without waiting for a rescan.
//!
//! The written files are re-read the way a scan reads changed files: their
//! folders are compared with the database and the files whose size, modification
//! time or inode changed are read by the worker processes and written as an
//! update of the tracks already at their paths, which keep their ids, identities,
//! statistics and ratings.
//!
//! Ratings are kept in the database. With [`TagWriteSettings::write_ratings_to_tags`]
//! (off by default) a rating is also written into the file of a whole-file track:
//! ID3v2 `POPM` with the e-mail field `Helikodu` (1, 64, 128, 196 or 255 for one
//! to five stars; no frame for zero stars) and `TXXX:FMPS_Rating`; Vorbis
//! comments and APEv2 `FMPS_RATING` (0.0 to 1.0) and `RATING` (0 to 100; left out
//! for zero stars); MP4 freeform `FMPS_Rating` and `RATING`. A CUE sheet track or
//! a chapter shares its file with other tracks, so its rating stays in the
//! database only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use helikodu_meta::tags::writing::{
    BatchItem, BatchProgress, BatchReport, Id3Version, TagChanges, WriteError, WriteMode,
    WriteOptions, WriteOutcome, write_batch, write_tags,
};
use helikodu_model::library::TrackId;
use helikodu_model::settings::{self, AdvancedSettings};

use crate::Library;
use crate::db::DbError;
use crate::scan::{self, CancelToken, ScanError, ScanOptions, ScanSummary, ScanTarget, Scope};
use crate::stats::Rating;

/// How the library writes tags, from the `[advanced]` settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TagWriteSettings {
    /// The ID3v2 version of new tags and the size above which files are changed
    /// in place.
    pub write: WriteOptions,
    /// Whether ratings are also written into the files. Off by default.
    pub write_ratings_to_tags: bool,
}

impl TagWriteSettings {
    /// The settings of the `[advanced]` table.
    pub fn from_settings(advanced: &AdvancedSettings) -> Self {
        Self {
            write: WriteOptions {
                id3_version: match advanced.id3_version {
                    settings::Id3Version::V2_3 => Id3Version::V2_3,
                    settings::Id3Version::V2_4 => Id3Version::V2_4,
                },
                in_place_above: advanced.in_place_tag_write_above(),
            },
            write_ratings_to_tags: advanced.write_ratings_to_tags,
        }
    }
}

/// The result of [`Library::write_tags`].
#[derive(Debug)]
pub struct TagWriteReport {
    /// Each file's write.
    pub batch: BatchReport,
    /// The re-read of the files written; `None` when none was.
    pub refresh: Option<ScanSummary>,
}

/// Why a rating could not be set.
#[derive(Debug, thiserror::Error)]
pub enum RateError {
    /// A rating outside 0 to 5 stars.
    #[error("{0} is not a rating from 0 to 5 stars")]
    InvalidStars(u8),
    /// The database failed, or there is no such track.
    #[error(transparent)]
    Db(#[from] DbError),
}

/// What [`Library::rate_track`] did besides storing the rating.
#[derive(Debug)]
pub struct RatingOutcome {
    /// The rating stored.
    pub rating: Option<Rating>,
    /// The write into the track's file, when ratings go to files and the track
    /// is a whole file.
    pub file: Option<Result<WriteOutcome, WriteError>>,
    /// The re-read of the file after the write; `Err` when it could not run.
    pub refresh: Option<Result<ScanSummary, ScanError>>,
}

impl Library {
    /// Writes `items` into their files (see
    /// [`write_batch`](helikodu_meta::tags::writing::write_batch)) and then re-reads
    /// the files written, so that the library has their new tags at once.
    ///
    /// `scan` gives the worker program and the reading rules, as for a scan.
    /// A file that fails does not stop the others; its error is in the report. The
    /// re-read fails only as a scan does, when the database fails or no worker
    /// can be started.
    pub fn write_tags(
        &self,
        items: &[BatchItem],
        settings: &TagWriteSettings,
        scan: &ScanOptions,
        progress: &mut dyn FnMut(&BatchProgress),
        cancel: &CancelToken,
    ) -> Result<TagWriteReport, ScanError> {
        let batch = write_batch(items, &settings.write, progress, &|| cancel.is_cancelled());
        let written: Vec<PathBuf> = batch
            .results
            .iter()
            .filter(|(_, r)| r.as_ref().is_ok_and(|o| o.mode != WriteMode::Unchanged))
            .map(|(path, _)| path.clone())
            .collect();
        let refresh = if written.is_empty() {
            None
        } else {
            Some(self.refresh_files(&written, scan)?)
        };
        Ok(TagWriteReport { batch, refresh })
    }

    /// Reads the files at `paths` again and updates their tracks, after their tags
    /// were written. Only the folders of these files are compared with the
    /// database; paths outside every library folder are left out.
    pub fn refresh_files(
        &self,
        paths: &[PathBuf],
        scan: &ScanOptions,
    ) -> Result<ScanSummary, ScanError> {
        let roots = self.roots()?;
        let mut targets: BTreeMap<PathBuf, Vec<Scope>> = BTreeMap::new();
        for path in paths {
            let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            let Some(root) = roots
                .iter()
                .filter(|root| path.starts_with(root))
                .max_by_key(|root| root.as_os_str().len())
            else {
                continue;
            };
            let Some(folder) = path.parent() else {
                continue;
            };
            targets
                .entry(root.clone())
                .or_default()
                .push(Scope::Folder(folder.to_path_buf()));
        }
        let targets: Vec<ScanTarget> = targets
            .into_iter()
            .map(|(root, scopes)| ScanTarget { root, scopes })
            .collect();
        scan::run_targets(self.database(), scan, &targets, None, &CancelToken::new())
    }

    /// Rates `track` in whole stars (0 to 5; stored as 0, 2, …, 10 half stars), or
    /// clears its rating with `None`.
    ///
    /// With `settings.write_ratings_to_tags`, the rating is also written into the
    /// track's file when the track is the whole file, and the file is read again
    /// (see the module documentation for the frames and values). The database
    /// keeps the rating either way; a failed file write is reported, not an error.
    pub fn rate_track(
        &self,
        track: TrackId,
        stars: Option<u8>,
        settings: &TagWriteSettings,
        scan: &ScanOptions,
    ) -> Result<RatingOutcome, RateError> {
        let rating = stars
            .map(|s| Rating::from_stars(s).ok_or(RateError::InvalidStars(s)))
            .transpose()?;
        self.set_rating(track, rating)?;
        let mut outcome = RatingOutcome {
            rating,
            file: None,
            refresh: None,
        };
        if !settings.write_ratings_to_tags {
            return Ok(outcome);
        }
        let Some(details) = self.track_details(track)? else {
            return Err(DbError::UnknownTrack(track).into());
        };
        if details.sub_index != 0 || details.missing_since.is_some() {
            return Ok(outcome);
        }
        let written = write_rating(&details.path, rating, &settings.write);
        if written.is_ok() {
            outcome.refresh = Some(self.refresh_files(std::slice::from_ref(&details.path), scan));
        }
        outcome.file = Some(written);
        Ok(outcome)
    }
}

fn write_rating(
    path: &Path,
    rating: Option<Rating>,
    options: &WriteOptions,
) -> Result<WriteOutcome, WriteError> {
    let mut changes = TagChanges::new();
    changes.rating =
        Some(rating.and_then(|r| helikodu_meta::tags::Rating::from_half_stars(r.half_stars())));
    write_tags(path, &changes, None, options)
}
