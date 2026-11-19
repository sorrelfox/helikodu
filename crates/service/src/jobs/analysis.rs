// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The track analysis as a job: every library track without a current analysis is
//! decoded once and analysed (silence and loudness envelope, see
//! [`helikodu_engine::analysis::analyze_source`]), one track at a time.
//!
//! - Each track's result goes into the library as soon as it is made, so a job that
//!   is stopped, or killed, loses at most the track it was working on; the next
//!   run picks up the tracks still pending. A track that cannot be decoded is
//!   recorded as failed with its file's stamp and is not tried again until the
//!   file changes.
//! - When [`AnalysisOptions::measure_loudness`] is set, the same pass takes the
//!   EBU R128 measurement of the loudness scanner for tracks that have no
//!   loudness values at all (no ReplayGain or R128 tags, no current measurement):
//!   a lone track gets its track values, an album none of whose tracks has values
//!   is analysed whole and gets album values too. Tracks with values keep them,
//!   so playback gains never change behind the owner's back.
//! - [`AnalysisGate`] lets the caller pause before a track and slow the job down
//!   between decoded blocks; the background analysis of the library service uses
//!   it to stay out of playback's way.

use std::path::{Path, PathBuf};

use helikodu_engine::analysis::{
    ANALYSIS_VERSION, AnalysisError, MEASUREMENT_VERSION, TrackMeasurement, album_loudness,
    analyze_source,
};
use helikodu_engine::decode::DecoderRegistry;
use helikodu_library::{
    AlbumMeasurement, AnalysisCoverage, AnalysisOutcome, AnalysisPlan, AnalysisUnit, CancelToken,
    MeasuredLoudness,
};
use helikodu_model::library::TrackId;

use crate::library::{LibraryError, MusicLibrary};

/// How many pending tracks are looked up at a time.
const PAGE: usize = 64;

/// What the job does besides the analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisOptions {
    /// Whether the loudness is measured in the same pass for tracks without
    /// loudness values.
    pub measure_loudness: bool,
    /// The largest album measured as one programme.
    pub max_album_tracks: usize,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            measure_loudness: true,
            max_album_tracks: 200,
        }
    }
}

/// Lets the caller pause and slow the job.
pub trait AnalysisGate {
    /// Asked before each track, with its file: may wait while the job should
    /// pause; `false` stops the job.
    fn before_track(&self, path: &Path) -> bool;
    /// Asked between decoded blocks: may wait to slow the job down; `false` stops
    /// the job (the track being decoded is left for the next run).
    fn between_blocks(&self) -> bool;
}

/// A gate that only stops when `CancelToken` is cancelled.
impl AnalysisGate for CancelToken {
    fn before_track(&self, _path: &Path) -> bool {
        !self.is_cancelled()
    }

    fn between_blocks(&self) -> bool {
        !self.is_cancelled()
    }
}

/// What the job announces.
#[derive(Debug, Clone, PartialEq)]
pub enum AnalysisEvent {
    /// The job started; how far the library is.
    Started(AnalysisCoverage),
    /// A track was analysed, or failed.
    Track {
        /// The track.
        track: TrackId,
        /// Its file.
        path: PathBuf,
        /// Whether it counted as pending (`false` for a track decoded again only
        /// for the loudness of its album).
        pending: bool,
        /// Why it failed.
        result: Result<(), String>,
    },
    /// The job ended.
    Finished(AnalysisSummary),
}

/// How the job ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnalysisSummary {
    /// The tracks analysed.
    pub analyzed: usize,
    /// The tracks that could not be analysed.
    pub failed: usize,
    /// The tracks whose loudness was measured and stored.
    pub loudness_measured: usize,
    /// Whether the job was stopped before it finished.
    pub cancelled: bool,
    /// Why the job stopped early, when the library failed.
    pub error: Option<String>,
}

/// Analyses the pending tracks of `library` on the calling thread, until none is
/// left or `gate` stops it. Files are decoded with the default decoder backends.
pub fn run(
    library: &MusicLibrary,
    options: &AnalysisOptions,
    gate: &dyn AnalysisGate,
    mut events: impl FnMut(AnalysisEvent),
) -> AnalysisSummary {
    let mut summary = AnalysisSummary::default();
    let registry = DecoderRegistry::with_default_backends();
    if let Err(error) = work(library, options, &registry, gate, &mut events, &mut summary) {
        summary.error = Some(error.to_string());
    }
    events(AnalysisEvent::Finished(summary.clone()));
    summary
}

/// How the analysis of the library stands now.
pub fn coverage(library: &MusicLibrary) -> Result<AnalysisCoverage, LibraryError> {
    Ok(library.inner().analysis_coverage(ANALYSIS_VERSION)?)
}

fn work(
    library: &MusicLibrary,
    options: &AnalysisOptions,
    registry: &DecoderRegistry,
    gate: &dyn AnalysisGate,
    events: &mut dyn FnMut(AnalysisEvent),
    summary: &mut AnalysisSummary,
) -> Result<(), LibraryError> {
    let inner = library.inner();
    events(AnalysisEvent::Started(
        inner.analysis_coverage(ANALYSIS_VERSION)?,
    ));
    let plan = AnalysisPlan {
        version: ANALYSIS_VERSION,
        loudness_version: options.measure_loudness.then_some(MEASUREMENT_VERSION),
        max_album_tracks: options.max_album_tracks,
    };
    let mut after = None;
    loop {
        let page = inner.tracks_to_analyze(ANALYSIS_VERSION, after, PAGE)?;
        if page.is_empty() {
            return Ok(());
        }
        for track in page {
            after = Some(track);
            let Some(unit) = inner.analysis_unit(track, &plan)? else {
                continue;
            };
            if !analyze_unit(library, registry, gate, unit, events, summary)? {
                summary.cancelled = true;
                return Ok(());
            }
        }
    }
}

/// Analyses the tracks of `unit`; `false` when the gate stopped the job.
fn analyze_unit(
    library: &MusicLibrary,
    registry: &DecoderRegistry,
    gate: &dyn AnalysisGate,
    unit: AnalysisUnit,
    events: &mut dyn FnMut(AnalysisEvent),
    summary: &mut AnalysisSummary,
) -> Result<bool, LibraryError> {
    let inner = library.inner();
    let mut measured: Vec<(TrackId, TrackMeasurement)> = Vec::new();
    let mut all_measured = true;
    for target in unit.targets {
        if !gate.before_track(&target.source.path) {
            return Ok(false);
        }
        let source = target.source.media_source();
        let result = analyze_source(registry, &source, unit.measure_loudness, &mut || {
            !gate.between_blocks()
        });
        let outcome = match result {
            Err(AnalysisError::Cancelled) => return Ok(false),
            Ok(analyzed) => {
                if let Some(loudness) = analyzed.loudness {
                    measured.push((target.track, loudness));
                }
                AnalysisOutcome::Analyzed(analyzed.analysis)
            }
            Err(error) => {
                all_measured = false;
                AnalysisOutcome::Failed(error.to_string())
            }
        };
        let result = match &outcome {
            AnalysisOutcome::Analyzed(_) => Ok(()),
            AnalysisOutcome::Failed(reason) => Err(reason.clone()),
        };
        inner.store_analysis(target.track, ANALYSIS_VERSION, outcome)?;
        if target.needs_analysis {
            match result {
                Ok(()) => summary.analyzed += 1,
                Err(_) => summary.failed += 1,
            }
        }
        events(AnalysisEvent::Track {
            track: target.track,
            path: target.source.path,
            pending: target.needs_analysis,
            result,
        });
    }
    if !measured.is_empty() {
        // An album whose tracks all decoded is measured as one programme; with a
        // track that failed, the others get their track values, as the loudness
        // scanner gives them.
        let album = if unit.album.is_some() && all_measured {
            album_loudness(measured.iter().map(|(_, m)| m)).map(|a| AlbumMeasurement {
                integrated_lufs: a.integrated_lufs,
                range_lu: a.range_lu,
                sample_peak: a.sample_peak,
                true_peak: a.true_peak,
            })
        } else {
            None
        };
        let values: Vec<MeasuredLoudness> = measured
            .iter()
            .map(|(track, m)| MeasuredLoudness {
                track: *track,
                version: MEASUREMENT_VERSION,
                integrated_lufs: m.loudness.integrated_lufs,
                range_lu: m.loudness.range_lu,
                sample_peak: m.loudness.sample_peak,
                true_peak: m.loudness.true_peak,
                album,
            })
            .collect();
        summary.loudness_measured += values.len();
        inner.store_loudness(values)?;
    }
    Ok(true)
}
