// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The analyzer of the sound being played: a spectrum in narrow bands on a
//! logarithmic axis, and the RMS and peak level of each channel.
//!
//! [`SoundAnalyzer::feed`] takes the samples that arrived since the last call
//! (from an [`AudioTap`](crate::output::AudioTap)) and returns one
//! [`AnalyzerFrame`]. It runs on an ordinary thread, never on the real-time side.
//!
//! - The spectrum is the FFT of the last few thousand frames of the channels'
//!   mean, Hann-windowed, its power summed into [`SPECTRUM_BANDS`] bands. A band
//!   narrower than the FFT's bins takes the power at its centre, interpolated
//!   between the two nearest bins. A full-scale sine reads 0 dB in its band.
//! - The RMS level of each channel is over the last 300 ms; the peak is the
//!   largest sample since the frame before.

use std::collections::VecDeque;
use std::sync::Arc;

use helikodu_model::audio::{ChannelLayout, ChannelPosition, SampleRate};
use helikodu_model::player::{
    AnalyzerFrame, ChannelLevel, LEVEL_FLOOR_DB, SPECTRUM_BANDS, amplitude_db, band_centre_hz,
    band_edge_hz,
};
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

/// The window of the RMS level.
pub const RMS_WINDOW_MS: u32 = 300;

/// The FFT length for `rate`: about a sixth of a second of sound, a power of two.
fn fft_len(rate: u32) -> usize {
    (rate as usize / 6).next_power_of_two().clamp(1024, 32_768)
}

/// Measures the sound being played; see the module documentation.
pub struct SoundAnalyzer {
    rate: u32,
    positions: Vec<ChannelPosition>,
    /// The last `fft.len()` frames of the channels' mean, oldest first.
    history: VecDeque<f32>,
    fft: Option<Arc<dyn RealToComplex<f32>>>,
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// For each band, the FFT bins it covers, or the fractional bin of its centre
    /// when it covers none.
    bands: Vec<BandBins>,
    /// The sums of squares of recent calls, each with its number of frames.
    recent: VecDeque<(Vec<f64>, usize)>,
    recent_frames: usize,
}

#[derive(Debug, Clone, Copy)]
enum BandBins {
    Range(usize, usize),
    Between(f32),
}

impl std::fmt::Debug for SoundAnalyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoundAnalyzer")
            .field("rate", &self.rate)
            .field("positions", &self.positions)
            .finish_non_exhaustive()
    }
}

impl Default for SoundAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl SoundAnalyzer {
    /// An analyzer that has heard nothing.
    pub fn new() -> Self {
        Self {
            rate: 0,
            positions: Vec::new(),
            history: VecDeque::new(),
            fft: None,
            window: Vec::new(),
            input: Vec::new(),
            output: Vec::new(),
            scratch: Vec::new(),
            bands: Vec::new(),
            recent: VecDeque::new(),
            recent_frames: 0,
        }
    }

    /// Forgets what it has heard, so the next sound starts from silence.
    pub fn reset(&mut self) {
        self.history.iter_mut().for_each(|x| *x = 0.0);
        self.recent.clear();
        self.recent_frames = 0;
    }

    /// The channels of the last format fed, as a silent frame.
    pub fn silent_frame(&self) -> AnalyzerFrame {
        AnalyzerFrame::silent(self.positions.iter().copied())
    }

    fn set_format(&mut self, rate: SampleRate, layout: ChannelLayout) {
        let channels = usize::from(layout.channel_count().max(1));
        let mut positions: Vec<ChannelPosition> = layout.positions().collect();
        if positions.len() != channels {
            positions = ChannelPosition::ALL
                .iter()
                .copied()
                .take(channels)
                .collect();
        }
        if rate.hz() == self.rate && positions == self.positions {
            return;
        }
        self.positions = positions;
        self.recent.clear();
        self.recent_frames = 0;
        if rate.hz() == self.rate {
            return;
        }
        self.rate = rate.hz();
        let len = fft_len(self.rate);
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(len);
        self.input = fft.make_input_vec();
        self.output = fft.make_output_vec();
        self.scratch = fft.make_scratch_vec();
        self.fft = Some(fft);
        self.history = std::iter::repeat_n(0.0, len).collect();
        // Hann.
        self.window = (0..len)
            .map(|i| {
                let x = std::f32::consts::TAU * i as f32 / len as f32;
                0.5 - 0.5 * x.cos()
            })
            .collect();
        let bin_hz = self.rate as f32 / len as f32;
        let nyquist_bin = len / 2;
        self.bands = (0..SPECTRUM_BANDS)
            .map(|band| {
                let low = (band_edge_hz(band) / bin_hz).ceil() as usize;
                let high = (band_edge_hz(band + 1) / bin_hz).ceil() as usize;
                let (low, high) = (low.min(nyquist_bin + 1), high.min(nyquist_bin + 1));
                if high > low {
                    BandBins::Range(low, high)
                } else {
                    BandBins::Between(band_centre_hz(band) / bin_hz)
                }
            })
            .collect();
    }

    /// Takes the interleaved `samples` (whole frames, in `layout`'s order) heard
    /// since the last call, and measures. Without samples the frame is silent and
    /// not live.
    pub fn feed(
        &mut self,
        rate: SampleRate,
        layout: ChannelLayout,
        samples: &[f32],
    ) -> AnalyzerFrame {
        self.set_format(rate, layout);
        let channels = self.positions.len().max(1);
        let frames = samples.len() / channels;
        if frames == 0 {
            return self.silent_frame();
        }
        let samples = &samples[..frames * channels];

        // Levels.
        let mut squares = vec![0.0f64; channels];
        let mut peaks = vec![0.0f32; channels];
        for frame in samples.chunks_exact(channels) {
            for (c, &x) in frame.iter().enumerate() {
                squares[c] += f64::from(x) * f64::from(x);
                peaks[c] = peaks[c].max(x.abs());
            }
        }
        self.recent.push_back((squares, frames));
        self.recent_frames += frames;
        let window = (self.rate as usize * RMS_WINDOW_MS as usize / 1000).max(1);
        while self.recent.len() > 1 && self.recent_frames - self.recent[0].1 >= window {
            if let Some((_, n)) = self.recent.pop_front() {
                self.recent_frames -= n;
            }
        }
        let levels = (0..channels)
            .map(|c| {
                let sum: f64 = self.recent.iter().map(|(s, _)| s[c]).sum();
                let rms = (sum / self.recent_frames.max(1) as f64).sqrt() as f32;
                ChannelLevel {
                    position: self.positions[c],
                    rms_db: amplitude_db(rms),
                    peak_db: amplitude_db(peaks[c]),
                }
            })
            .collect();

        // Spectrum.
        let len = self.history.len();
        let scale = 1.0 / channels as f32;
        let fresh = samples
            .chunks_exact(channels)
            .skip(frames.saturating_sub(len))
            .map(|frame| frame.iter().sum::<f32>() * scale);
        for x in fresh {
            self.history.pop_front();
            self.history.push_back(x);
        }
        let bands = self.spectrum();
        AnalyzerFrame {
            bands,
            channels: levels,
            live: true,
        }
    }

    fn spectrum(&mut self) -> Vec<f32> {
        let Some(fft) = &self.fft else {
            return vec![LEVEL_FLOOR_DB; SPECTRUM_BANDS];
        };
        for ((dst, &x), &w) in self.input.iter_mut().zip(&self.history).zip(&self.window) {
            *dst = x * w;
        }
        if fft
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .is_err()
        {
            return vec![LEVEL_FLOOR_DB; SPECTRUM_BANDS];
        }
        // A full-scale sine's bin is N/4 high with a Hann window; its power is
        // spread over the bins as 1.5 times that bin's (the window's noise width).
        let full = (self.history.len() as f32 / 4.0).powi(2);
        let power = |bin: usize| self.output.get(bin).map_or(0.0, |c| c.norm_sqr());
        self.bands
            .iter()
            .map(|band| {
                let p = match *band {
                    BandBins::Range(low, high) => {
                        (low..high).map(power).sum::<f32>() / (1.5 * full)
                    }
                    BandBins::Between(at) => {
                        let below = at.floor() as usize;
                        let t = at - below as f32;
                        (power(below) * (1.0 - t) + power(below + 1) * t) / full
                    }
                };
                if p > 0.0 {
                    (10.0 * p.log10()).clamp(LEVEL_FLOOR_DB, 6.0)
                } else {
                    LEVEL_FLOOR_DB
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f32, amplitude: f32, frames: usize, channels: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let x = amplitude * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin();
                std::iter::repeat_n(x, channels)
            })
            .collect()
    }

    #[test]
    fn a_sine_lights_its_own_band() {
        let mut analyzer = SoundAnalyzer::new();
        let samples = sine(48_000, 1_000.0, 1.0, 12_000, 2);
        let frame = analyzer.feed(SampleRate::HZ_48000, ChannelLayout::STEREO, &samples);
        let loudest = (0..SPECTRUM_BANDS)
            .max_by(|&a, &b| frame.bands[a].total_cmp(&frame.bands[b]))
            .unwrap();
        assert!(
            band_edge_hz(loudest) <= 1_000.0 && 1_000.0 < band_edge_hz(loudest + 1),
            "band {loudest}"
        );
        assert!(
            frame.bands[loudest] > -3.0 && frame.bands[loudest] < 1.0,
            "{}",
            frame.bands[loudest]
        );
        // Far away from it, nothing.
        assert!(frame.bands[0] < -60.0, "{}", frame.bands[0]);
        assert!(frame.bands[SPECTRUM_BANDS - 1] < -60.0);
        assert!(frame.live);
    }

    #[test]
    fn levels_are_rms_over_300_ms_and_peaks_per_frame() {
        let mut analyzer = SoundAnalyzer::new();
        // Left at half scale, right silent.
        let samples: Vec<f32> = sine(48_000, 440.0, 0.5, 4_800, 1)
            .into_iter()
            .flat_map(|x| [x, 0.0])
            .collect();
        let frame = analyzer.feed(SampleRate::HZ_48000, ChannelLayout::STEREO, &samples);
        let [left, right] = [frame.channels[0], frame.channels[1]];
        assert_eq!(left.position, ChannelPosition::FrontLeft);
        // A sine's RMS is 3 dB below its peak.
        assert!((left.peak_db - (-6.02)).abs() < 0.1, "{}", left.peak_db);
        assert!((left.rms_db - (-9.03)).abs() < 0.2, "{}", left.rms_db);
        assert_eq!(right.rms_db, LEVEL_FLOOR_DB);

        // Silence for a while: the RMS falls once the sine is older than 300 ms.
        let quiet = vec![0.0; 1_600 * 2];
        let mut frame = frame;
        for _ in 0..10 {
            frame = analyzer.feed(SampleRate::HZ_48000, ChannelLayout::STEREO, &quiet);
        }
        assert_eq!(frame.channels[0].peak_db, LEVEL_FLOOR_DB);
        assert_eq!(frame.channels[0].rms_db, LEVEL_FLOOR_DB);
    }

    #[test]
    fn nothing_heard_is_a_silent_frame() {
        let mut analyzer = SoundAnalyzer::new();
        let frame = analyzer.feed(SampleRate::HZ_44100, ChannelLayout::SURROUND_5_1, &[]);
        assert!(!frame.live);
        assert_eq!(frame.channels.len(), 6);
        assert_eq!(frame.channels[3].position, ChannelPosition::LowFrequency);
        assert!(frame.bands.iter().all(|&b| b == LEVEL_FLOOR_DB));
    }
}
