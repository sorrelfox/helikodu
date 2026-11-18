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
//! [`waveform_peaks`] decodes a source the same way and keeps the largest sample
//! of each stretch ([`PeakBuilder`]).
//!
//! [`SoundAnalyzer`] measures what an [`AudioTap`](crate::output::AudioTap)
//! copied: the spectrum and the level of each channel.
//!
//! [`DecoderRegistry`]: crate::decode::DecoderRegistry

mod loudness;
mod meter;
mod peaks;

pub use self::loudness::{
    AlbumLoudness, AnalysisError, LoudnessMeter, MEASUREMENT_VERSION, TrackLoudness,
    TrackMeasurement, album_loudness, measure_source,
};
pub use self::meter::{RMS_WINDOW_MS, SoundAnalyzer};
pub use self::peaks::{PeakBuilder, WAVEFORM_BINS, reduce as reduce_peaks, waveform_peaks};
