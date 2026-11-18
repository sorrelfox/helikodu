// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the analyzer measures of the sound being played: the spectrum in narrow
//! bands on a logarithmic axis and the level of each channel.

use crate::audio::ChannelPosition;

/// The number of spectrum bands, evenly spaced on a logarithmic axis from
/// [`SPECTRUM_LOW_HZ`] to [`SPECTRUM_HIGH_HZ`] (each about a fifth of an octave).
pub const SPECTRUM_BANDS: usize = 48;
/// The lower edge of the lowest band, in Hz.
pub const SPECTRUM_LOW_HZ: f32 = 25.0;
/// The upper edge of the highest band, in Hz.
pub const SPECTRUM_HIGH_HZ: f32 = 20_000.0;
/// The lowest level reported, in dBFS: anything quieter is reported as this.
pub const LEVEL_FLOOR_DB: f32 = -96.0;

/// Where on the analyzer's axis `hz` lies: 0 at [`SPECTRUM_LOW_HZ`], 1 at
/// [`SPECTRUM_HIGH_HZ`].
pub fn axis_fraction(hz: f32) -> f32 {
    (hz / SPECTRUM_LOW_HZ).ln() / (SPECTRUM_HIGH_HZ / SPECTRUM_LOW_HZ).ln()
}

/// The lower edge of spectrum band `band` in Hz; `band_edge_hz(SPECTRUM_BANDS)` is
/// the upper edge of the last one.
pub fn band_edge_hz(band: usize) -> f32 {
    SPECTRUM_LOW_HZ * (SPECTRUM_HIGH_HZ / SPECTRUM_LOW_HZ).powf(band as f32 / SPECTRUM_BANDS as f32)
}

/// The centre frequency of spectrum band `band`, in Hz (the geometric middle of
/// its edges).
pub fn band_centre_hz(band: usize) -> f32 {
    (band_edge_hz(band) * band_edge_hz(band + 1)).sqrt()
}

/// The level of one channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelLevel {
    /// The speaker the channel feeds.
    pub position: ChannelPosition,
    /// The RMS level over the last 300 ms, in dBFS.
    pub rms_db: f32,
    /// The largest sample since the frame before, in dBFS (0 is full scale).
    pub peak_db: f32,
}

/// One measurement of the analyzer, sent up to 30 times a second while a view
/// asks for it and sound is played.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalyzerFrame {
    /// The level of each spectrum band, in dBFS, from the lowest band up
    /// ([`SPECTRUM_BANDS`] of them).
    pub bands: Vec<f32>,
    /// The level of each channel, in the stream's channel order.
    pub channels: Vec<ChannelLevel>,
    /// Whether sound arrived for this frame. The frame after the sound stopped
    /// (a pause, the end) has every level at the floor and this `false`; no more
    /// frames come until sound arrives again.
    pub live: bool,
}

impl AnalyzerFrame {
    /// A frame of silence for `channels`.
    pub fn silent(channels: impl IntoIterator<Item = ChannelPosition>) -> Self {
        Self {
            bands: vec![LEVEL_FLOOR_DB; SPECTRUM_BANDS],
            channels: channels
                .into_iter()
                .map(|position| ChannelLevel {
                    position,
                    rms_db: LEVEL_FLOOR_DB,
                    peak_db: LEVEL_FLOOR_DB,
                })
                .collect(),
            live: false,
        }
    }
}

/// Linear amplitude to dBFS, never below [`LEVEL_FLOOR_DB`].
pub fn amplitude_db(amplitude: f32) -> f32 {
    if amplitude <= 0.0 || !amplitude.is_finite() {
        return LEVEL_FLOOR_DB;
    }
    (20.0 * amplitude.log10()).max(LEVEL_FLOOR_DB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_span_25_hz_to_20_khz() {
        assert!((band_edge_hz(0) - 25.0).abs() < 1e-3);
        assert!((band_edge_hz(SPECTRUM_BANDS) - 20_000.0).abs() < 0.5);
        assert!(band_centre_hz(0) > 25.0 && band_centre_hz(0) < band_edge_hz(1));
        assert!(axis_fraction(25.0).abs() < 1e-6);
        assert!((axis_fraction(20_000.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn levels_in_db() {
        assert_eq!(amplitude_db(1.0), 0.0);
        assert!((amplitude_db(0.5) + 6.0206).abs() < 1e-3);
        assert_eq!(amplitude_db(0.0), LEVEL_FLOOR_DB);
        assert_eq!(amplitude_db(f32::NAN), LEVEL_FLOOR_DB);
        let frame = AnalyzerFrame::silent([ChannelPosition::FrontLeft]);
        assert_eq!(frame.bands.len(), SPECTRUM_BANDS);
        assert!(!frame.live);
    }
}
