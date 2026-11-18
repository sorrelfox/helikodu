// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Analysis of whole sources, away from playback: the loudness scanner.
//!
//! [`measure_source`] decodes a source through the [`DecoderRegistry`] (sections of
//! CUE sheets included) and measures it with a [`LoudnessMeter`] following EBU R128
//! and ITU-R BS.1770-4: integrated loudness, loudness range, sample peak and true
//! peak. [`album_loudness`] measures several such tracks as one programme.
//!
//! [`DecoderRegistry`]: crate::decode::DecoderRegistry

mod loudness;

pub use self::loudness::{
    AlbumLoudness, AnalysisError, LoudnessMeter, MEASUREMENT_VERSION, TrackLoudness,
    TrackMeasurement, album_loudness, measure_source,
};
