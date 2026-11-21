// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Analysis of whole sources, away from playback: the loudness scanner and the
//! waveform peaks of the seek bar; and the analyzer of the sound being played.
//!
//! [`measure_source`] decodes a source through the [`DecoderRegistry`] (sections of
//! CUE sheets included) and measures it with a [`LoudnessMeter`] following EBU R128
//! and ITU-R BS.1770-4: integrated loudness, loudness range, sample peak and true
//! peak. [`album_loudness`] measures several such tracks as one programme.
//!
//! [`analyze_source`] decodes a source once for the track analysis: where its
//! sound starts and ends, its silent stretches and its loudness envelope
//! ([`TrackAnalyzer`]), with the loudness measurement in the same pass when asked,
//! and the sound features that tell how alike two tracks sound.
//!
//! [`waveform_peaks`] decodes a source the same way and keeps the largest sample
//! of each stretch ([`PeakBuilder`]).
//!
//! [`SoundAnalyzer`] measures what an [`AudioTap`](crate::output::AudioTap)
//! copied: the spectrum and the level of each channel.
//!
//! [`DecoderRegistry`]: crate::decode::DecoderRegistry

mod features;
mod loudness;
mod meter;
mod peaks;
mod track;

pub use self::loudness::{
    AlbumLoudness, AnalysisError, LoudnessMeter, MEASUREMENT_VERSION, TrackLoudness,
    TrackMeasurement, album_loudness, measure_source,
};
pub use self::meter::{RMS_WINDOW_MS, SoundAnalyzer};
pub use self::peaks::{PeakBuilder, WAVEFORM_BINS, reduce as reduce_peaks, waveform_peaks};
pub use self::track::{
    ANALYSIS_VERSION, BODY_DROP_LU, EDGE_MS, EDGE_SEARCH_MS, MAX_ENVELOPE_STEPS,
    MAX_INNER_SILENCES, MIN_INNER_SILENCE_MS, SILENCE_THRESHOLD_DBFS, SILENCE_WINDOW_MS,
    SourceAnalysis, TrackAnalyzer, analyze_source,
};
