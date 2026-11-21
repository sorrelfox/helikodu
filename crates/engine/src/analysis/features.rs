// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sound features of the track analysis ([`SoundFeatures`]): a small vector
//! of hand-built statistics that tells how a track sounds, made in the same
//! decoding pass as the rest of the analysis.
//!
//! The choice follows three rules: no trained model (nothing to download or
//! bundle), a cost that disappears next to decoding, and robustness, so that
//! the same music at another gain, sample rate or encoding lands in the same
//! place.
//!
//! - The channels are mixed to mono and cut into frames of about 40 to 50 ms (a
//!   power of two of samples, without overlap), each weighted by a Hann window
//!   and turned into a power spectrum by one real FFT. That is a few dozen
//!   operations per sample, far less than decoding.
//! - Frames below the silence threshold of the analysis are left out of the
//!   statistics, so leading, trailing and inner silences do not count.
//! - Timbre: 32 mel bands from 40 Hz to 8 kHz, their logarithm and a DCT give the
//!   cepstral coefficients 1 to 12, kept as means and standard deviations. The
//!   coefficient 0 (the level) is dropped, and the logarithm's floor is relative
//!   to each frame's power, so a change of gain shifts nothing else. The spectral
//!   centroid, rolloff and flatness, over 20 Hz to 16 kHz so that high-resolution
//!   files are not set apart by inaudible content, and the zero-crossing rate
//!   (per second, not per sample) add brightness and noisiness.
//! - Rhythm: the spectral flux (the mean rise of the log mel bands from frame to
//!   frame) is picked for peaks that stand out of the last second; a sounding
//!   frame after a silent one counts as an onset too. Onsets per second and the
//!   mean flux describe how busy and percussive a track is without the
//!   fragility of a tempo estimate.
//! - Tonality: the power from 55 Hz to 4 kHz folded into 12 pitch classes gives
//!   the mean peakiness of each frame's chroma and the entropy of the whole
//!   track's profile: tonal music has a few strong pitch classes, noise and drums
//!   have none. Both are independent of the key, so a song and its transposed
//!   cover stay close.
//! - Dynamics: the spread of the per-second loudness that the analysis already
//!   measures (see [`loudness_spread`]).

use std::collections::VecDeque;
use std::sync::Arc;

use helikodu_model::analysis::{SOUND_FEATURES_LEN, SoundFeatures};
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

/// The mel bands.
const MEL_BANDS: usize = 32;
/// The cepstral coefficients kept (1 to 12).
const CEPSTRA: usize = 12;
const MEL_LOW_HZ: f64 = 40.0;
const MEL_HIGH_HZ: f64 = 8_000.0;
/// The range of the centroid, rolloff and flatness.
const SPECTRUM_LOW_HZ: f64 = 20.0;
const SPECTRUM_HIGH_HZ: f64 = 16_000.0;
/// The range folded into pitch classes.
const CHROMA_LOW_HZ: f64 = 55.0;
const CHROMA_HIGH_HZ: f64 = 4_000.0;
/// The share of the energy below the rolloff.
const ROLLOFF: f64 = 0.85;
/// The floor of a frame's power spectrum, relative to its mean power.
const RELATIVE_FLOOR: f64 = 1e-5;
/// The flux frames on each side of a peak that it must exceed.
const PEAK_REACH: usize = 2;
/// The flux frames (about one second) a peak must stand out of.
const FLUX_HISTORY: usize = 22;
/// How far above the mean of the history a peak must rise, in nats per band.
const ONSET_DELTA: f64 = 0.35;
/// The floor of the spectral flatness, in dB.
const FLATNESS_FLOOR_DB: f64 = -100.0;

/// A mel band: its first FFT bin and the weights of the bins from there, which
/// sum to one.
#[derive(Debug, Clone)]
struct MelBand {
    first: usize,
    weights: Vec<f64>,
}

/// The tables for one sample rate.
struct Tables {
    rate: u32,
    size: usize,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    /// The frequency of each bin.
    bin_hz: f64,
    mel: Vec<MelBand>,
    /// The DCT rows for the coefficients 1 to 12.
    dct: Vec<[f64; MEL_BANDS]>,
    spectrum_bins: std::ops::Range<usize>,
    /// The pitch class of each bin in the chroma range, from `chroma_first`.
    chroma_first: usize,
    chroma_classes: Vec<u8>,
}

impl Tables {
    fn new(rate: u32) -> Self {
        let size = frame_size(rate);
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(size);
        #[allow(clippy::cast_precision_loss)]
        let window = (0..size)
            .map(|i| {
                let x = std::f32::consts::TAU * i as f32 / size as f32;
                0.5 - 0.5 * x.cos()
            })
            .collect();
        let bins = size / 2 + 1;
        #[allow(clippy::cast_precision_loss)]
        let bin_hz = f64::from(rate) / size as f64;
        let nyquist = f64::from(rate) / 2.0;
        let bin_of = |hz: f64| -> usize {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let bin = (hz / bin_hz).round().max(0.0) as usize;
            bin.min(bins - 1)
        };
        let mel = mel_bands(bin_hz, bins, MEL_HIGH_HZ.min(nyquist * 0.95));
        let dct = (1..=CEPSTRA)
            .map(|k| {
                let mut row = [0.0; MEL_BANDS];
                #[allow(clippy::cast_precision_loss)]
                for (b, value) in row.iter_mut().enumerate() {
                    *value = (2.0 / MEL_BANDS as f64).sqrt()
                        * (std::f64::consts::PI * k as f64 * (b as f64 + 0.5) / MEL_BANDS as f64)
                            .cos();
                }
                row
            })
            .collect();
        let spectrum_bins =
            bin_of(SPECTRUM_LOW_HZ).max(1)..bin_of(SPECTRUM_HIGH_HZ.min(nyquist)) + 1;
        let chroma_first = bin_of(CHROMA_LOW_HZ).max(1);
        let chroma_end = bin_of(CHROMA_HIGH_HZ.min(nyquist)) + 1;
        #[allow(clippy::cast_precision_loss)]
        let chroma_classes = (chroma_first..chroma_end.max(chroma_first))
            .map(|bin| {
                let semitones = 12.0 * (bin as f64 * bin_hz / 440.0).log2();
                #[allow(clippy::cast_possible_truncation)]
                let class = (semitones.round() as i64).rem_euclid(12);
                u8::try_from(class).unwrap_or(0)
            })
            .collect();
        Self {
            rate,
            size,
            fft,
            window,
            bin_hz,
            mel,
            dct,
            spectrum_bins,
            chroma_first,
            chroma_classes,
        }
    }
}

/// The frame length at `rate`: the largest power of two not above 1/20 s, at
/// least 256 samples.
fn frame_size(rate: u32) -> usize {
    let target = usize::try_from(rate / 20).unwrap_or(2048).max(256);
    1 << (usize::BITS - 1 - target.leading_zeros())
}

fn hz_to_mel(hz: f64) -> f64 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz(mel: f64) -> f64 {
    700.0 * (10f64.powf(mel / 2595.0) - 1.0)
}

/// Triangular mel bands from [`MEL_LOW_HZ`] to `high`, each normalised so that
/// it is the mean power of its bins; a band narrower than a bin takes its
/// nearest bin.
fn mel_bands(bin_hz: f64, bins: usize, high: f64) -> Vec<MelBand> {
    let (low, high) = (hz_to_mel(MEL_LOW_HZ), hz_to_mel(high.max(MEL_LOW_HZ * 2.0)));
    #[allow(clippy::cast_precision_loss)]
    let edges: Vec<f64> = (0..MEL_BANDS + 2)
        .map(|i| mel_to_hz(low + (high - low) * i as f64 / (MEL_BANDS + 1) as f64))
        .collect();
    edges
        .windows(3)
        .map(|edge| {
            let (left, centre, right) = (edge[0], edge[1], edge[2]);
            let mut first = None;
            let mut weights = Vec::new();
            for bin in 1..bins {
                #[allow(clippy::cast_precision_loss)]
                let hz = bin as f64 * bin_hz;
                let weight = if hz > left && hz <= centre {
                    (hz - left) / (centre - left)
                } else if hz > centre && hz < right {
                    (right - hz) / (right - centre)
                } else {
                    0.0
                };
                if weight > 0.0 {
                    first.get_or_insert(bin);
                    weights.push(weight);
                } else if first.is_some() {
                    break;
                }
            }
            let sum: f64 = weights.iter().sum();
            match first {
                Some(first) if sum > 0.0 => MelBand {
                    first,
                    weights: weights.into_iter().map(|w| w / sum).collect(),
                },
                _ => {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let bin = ((centre / bin_hz).round() as usize).clamp(1, bins - 1);
                    MelBand {
                        first: bin,
                        weights: vec![1.0],
                    }
                }
            }
        })
        .collect()
}

/// Running sums for a mean and a standard deviation.
#[derive(Debug, Clone, Copy, Default)]
struct Moments {
    n: f64,
    sum: f64,
    squares: f64,
}

impl Moments {
    fn add(&mut self, x: f64) {
        self.n += 1.0;
        self.sum += x;
        self.squares += x * x;
    }

    fn mean(&self) -> f64 {
        if self.n > 0.0 { self.sum / self.n } else { 0.0 }
    }

    fn std(&self) -> f64 {
        if self.n > 0.0 {
            let mean = self.mean();
            (self.squares / self.n - mean * mean).max(0.0).sqrt()
        } else {
            0.0
        }
    }
}

/// Picks onsets from the flux of successive frames, two frames late.
#[derive(Debug, Default)]
struct OnsetPicker {
    flux: VecDeque<f64>,
    onsets: u64,
}

impl OnsetPicker {
    fn add(&mut self, flux: f64) {
        self.flux.push_back(flux);
        if self.flux.len() > FLUX_HISTORY + PEAK_REACH + 1 {
            self.flux.pop_front();
        }
        let len = self.flux.len();
        if len < 2 * PEAK_REACH + 1 {
            return;
        }
        let candidate = len - 1 - PEAK_REACH;
        let value = self.flux[candidate];
        let around = candidate - PEAK_REACH..=candidate + PEAK_REACH;
        let is_peak = around.clone().all(|i| {
            i == candidate || self.flux[i] < value || (i > candidate && self.flux[i] <= value)
        });
        if !is_peak {
            return;
        }
        let history: Vec<f64> = self.flux.range(..candidate).copied().collect();
        #[allow(clippy::cast_precision_loss)]
        let mean = if history.is_empty() {
            0.0
        } else {
            history.iter().sum::<f64>() / history.len() as f64
        };
        if value > mean + ONSET_DELTA {
            self.onsets += 1;
        }
    }

    /// A silent frame: the next sounding frame is an onset, and the flux starts
    /// afresh.
    fn silence(&mut self) {
        self.flux.clear();
    }
}

/// Collects the sound features of interleaved audio as it arrives; see the
/// module documentation.
pub(crate) struct FeatureExtractor {
    tables: Tables,
    channels: usize,
    frame: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    power: Vec<f64>,
    previous: Option<[f64; MEL_BANDS]>,
    last_sample: f32,

    /// All frames, silent or not, and their samples.
    frames: u64,
    seconds: f64,
    sounding: u64,
    cepstra: [Moments; CEPSTRA],
    centroid: Moments,
    rolloff: Moments,
    flatness: Moments,
    flux: Moments,
    chroma_peak: Moments,
    chroma_profile: [f64; 12],
    crossings: u64,
    crossing_seconds: f64,
    onsets: OnsetPicker,
    after_silence: bool,
}

impl std::fmt::Debug for FeatureExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeatureExtractor")
            .field("rate", &self.tables.rate)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

impl FeatureExtractor {
    /// An extractor for audio at `rate` Hz with `channels` channels.
    pub(crate) fn new(rate: u32, channels: usize) -> Self {
        let tables = Tables::new(rate.max(1));
        let input = tables.fft.make_input_vec();
        let output = tables.fft.make_output_vec();
        let scratch = tables.fft.make_scratch_vec();
        Self {
            channels: channels.max(1),
            frame: Vec::with_capacity(tables.size),
            power: vec![0.0; output.len()],
            input,
            output,
            scratch,
            tables,
            previous: None,
            last_sample: 0.0,
            frames: 0,
            seconds: 0.0,
            sounding: 0,
            cepstra: [Moments::default(); CEPSTRA],
            centroid: Moments::default(),
            rolloff: Moments::default(),
            flatness: Moments::default(),
            flux: Moments::default(),
            chroma_peak: Moments::default(),
            chroma_profile: [0.0; 12],
            crossings: 0,
            crossing_seconds: 0.0,
            onsets: OnsetPicker::default(),
            after_silence: true,
        }
    }

    /// Follows a change of rate or channels; a partial frame is dropped.
    pub(crate) fn follow(&mut self, rate: u32, channels: usize) {
        self.channels = channels.max(1);
        self.frame.clear();
        if rate.max(1) != self.tables.rate {
            self.tables = Tables::new(rate.max(1));
            self.input = self.tables.fft.make_input_vec();
            self.output = self.tables.fft.make_output_vec();
            self.scratch = self.tables.fft.make_scratch_vec();
            self.power = vec![0.0; self.output.len()];
            self.previous = None;
        }
    }

    /// Adds interleaved samples, full scale at ±1.0.
    pub(crate) fn add(&mut self, interleaved: &[f32]) {
        #[allow(clippy::cast_precision_loss)]
        let scale = 1.0 / self.channels as f32;
        for frame in interleaved.chunks_exact(self.channels) {
            self.frame.push(frame.iter().sum::<f32>() * scale);
            if self.frame.len() == self.tables.size {
                self.close_frame();
                self.frame.clear();
            }
        }
    }

    fn close_frame(&mut self) {
        let size = self.tables.size;
        self.frames += 1;
        #[allow(clippy::cast_precision_loss)]
        let frame_seconds = size as f64 / f64::from(self.tables.rate);
        self.seconds += frame_seconds;
        #[allow(clippy::cast_precision_loss)]
        let mean_square = self
            .frame
            .iter()
            .map(|&x| f64::from(x) * f64::from(x))
            .sum::<f64>()
            / size as f64;
        if mean_square < super::track::threshold_mean_square() {
            self.previous = None;
            self.onsets.silence();
            self.after_silence = true;
            self.last_sample = 0.0;
            return;
        }
        self.sounding += 1;

        // Zero crossings, per second.
        let mut last = self.last_sample;
        for &x in &self.frame {
            if (x >= 0.0) != (last >= 0.0) {
                self.crossings += 1;
            }
            last = x;
        }
        self.last_sample = last;
        self.crossing_seconds += frame_seconds;

        for ((dst, &x), &w) in self
            .input
            .iter_mut()
            .zip(&self.frame)
            .zip(&self.tables.window)
        {
            *dst = x * w;
        }
        if self
            .tables
            .fft
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .is_err()
        {
            return;
        }
        // A full-scale sine's bin is N/4 high with a Hann window.
        #[allow(clippy::cast_precision_loss)]
        let norm = 1.0 / (size as f64 / 4.0).powi(2);
        for (p, c) in self.power.iter_mut().zip(&self.output) {
            *p = f64::from(c.norm_sqr()) * norm;
        }
        self.power[0] = 0.0;
        #[allow(clippy::cast_precision_loss)]
        let mean_power = self.power.iter().sum::<f64>() / self.power.len() as f64;
        let floor = (mean_power * RELATIVE_FLOOR).max(1e-30);

        // Timbre.
        let mut log_mel = [0.0; MEL_BANDS];
        for (value, band) in log_mel.iter_mut().zip(&self.tables.mel) {
            let energy: f64 = band
                .weights
                .iter()
                .zip(&self.power[band.first..])
                .map(|(w, p)| w * p)
                .sum();
            *value = (energy + floor).ln();
        }
        for (moments, row) in self.cepstra.iter_mut().zip(&self.tables.dct) {
            moments.add(row.iter().zip(&log_mel).map(|(a, b)| a * b).sum());
        }
        let bins = &self.power[self.tables.spectrum_bins.clone()];
        let first = self.tables.spectrum_bins.start;
        let total: f64 = bins.iter().sum();
        if total > 0.0 {
            #[allow(clippy::cast_precision_loss)]
            let centroid = bins
                .iter()
                .enumerate()
                .map(|(i, p)| (first + i) as f64 * self.tables.bin_hz * p)
                .sum::<f64>()
                / total;
            self.centroid.add(centroid.max(1.0).log2());
            let mut acc = 0.0;
            let mut rolloff = bins.len() - 1;
            for (i, p) in bins.iter().enumerate() {
                acc += p;
                if acc >= ROLLOFF * total {
                    rolloff = i;
                    break;
                }
            }
            #[allow(clippy::cast_precision_loss)]
            let rolloff_hz = (first + rolloff) as f64 * self.tables.bin_hz;
            self.rolloff.add(rolloff_hz.max(1.0).log2());
            #[allow(clippy::cast_precision_loss)]
            let n = bins.len() as f64;
            let log_mean = bins.iter().map(|p| (p + floor).ln()).sum::<f64>() / n;
            let flatness_db = 10.0 * (log_mean.exp() / (total / n + floor)).log10();
            self.flatness.add(flatness_db.max(FLATNESS_FLOOR_DB));
        }

        // Rhythm.
        if let Some(previous) = &self.previous {
            #[allow(clippy::cast_precision_loss)]
            let flux = log_mel
                .iter()
                .zip(previous)
                .map(|(now, before)| (now - before).max(0.0))
                .sum::<f64>()
                / MEL_BANDS as f64;
            self.flux.add(flux);
            self.onsets.add(flux);
        }
        if self.after_silence {
            self.onsets.onsets += 1;
            self.after_silence = false;
        }
        self.previous = Some(log_mel);

        // Tonality.
        let mut chroma = [0.0; 12];
        for (offset, &class) in self.tables.chroma_classes.iter().enumerate() {
            chroma[usize::from(class)] += self.power[self.tables.chroma_first + offset];
        }
        let sum: f64 = chroma.iter().sum();
        if sum > 0.0 {
            let peak = chroma.iter().copied().fold(0.0, f64::max);
            self.chroma_peak.add(peak / sum);
            for (profile, value) in self.chroma_profile.iter_mut().zip(chroma) {
                *profile += value / sum;
            }
        }
    }

    /// The features of everything added, with the loudness spread measured by
    /// the rest of the analysis; `None` when no frame had sound.
    pub(crate) fn finish(self, loudness_spread: f64) -> Option<SoundFeatures> {
        if self.sounding == 0 {
            return None;
        }
        let mut values = [0.0f64; SOUND_FEATURES_LEN];
        for (i, moments) in self.cepstra.iter().enumerate() {
            values[i] = moments.mean();
            values[CEPSTRA + i] = moments.std();
        }
        values[24] = self.centroid.mean();
        values[25] = self.centroid.std();
        values[26] = self.rolloff.mean();
        values[27] = self.flatness.mean();
        #[allow(clippy::cast_precision_loss)]
        let crossings_per_second = self.crossings as f64 / self.crossing_seconds.max(1e-9);
        values[28] = (crossings_per_second + 1.0).log2();
        #[allow(clippy::cast_precision_loss)]
        let onset_rate = self.onsets.onsets as f64 / self.seconds.max(1e-9);
        values[29] = onset_rate;
        values[30] = self.flux.mean();
        values[31] = self.chroma_peak.mean();
        let total: f64 = self.chroma_profile.iter().sum();
        values[32] = if total > 0.0 {
            let entropy: f64 = self
                .chroma_profile
                .iter()
                .map(|v| v / total)
                .filter(|p| *p > 0.0)
                .map(|p| -p * p.ln())
                .sum();
            entropy / 12f64.ln()
        } else {
            1.0
        };
        values[33] = loudness_spread;
        #[allow(clippy::cast_possible_truncation)]
        let values = values.map(|v| if v.is_finite() { v as f32 } else { 0.0 });
        Some(SoundFeatures(values))
    }
}

/// The loudness spread of a track from the loudness of each second (in LUFS,
/// `-inf` for silence): the 95th less the 10th percentile of the seconds above
/// −70 LUFS, in LU; 0 with fewer than two such seconds.
pub(crate) fn loudness_spread(seconds: impl IntoIterator<Item = f64>) -> f64 {
    let mut sounding: Vec<f64> = seconds
        .into_iter()
        .filter(|lufs| lufs.is_finite() && *lufs > -70.0)
        .collect();
    if sounding.len() < 2 {
        return 0.0;
    }
    sounding.sort_by(f64::total_cmp);
    let at = |q: f64| {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let index = ((sounding.len() - 1) as f64 * q).round() as usize;
        sounding[index.min(sounding.len() - 1)]
    };
    at(0.95) - at(0.10)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    /// A small deterministic noise source.
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            #[allow(clippy::cast_precision_loss)]
            let unit = (self.0 >> 40) as f32 / (1u64 << 24) as f32;
            unit * 2.0 - 1.0
        }
    }

    fn seconds(s: f32) -> usize {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = (s * RATE as f32) as usize;
        n
    }

    fn sine(hz: f32, gain: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| gain * (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin())
            .collect()
    }

    fn noise(gain: f32, len: usize, seed: u64) -> Vec<f32> {
        let mut noise = Noise(seed);
        (0..len).map(|_| gain * noise.next()).collect()
    }

    /// Short decaying noise bursts, `per_second` of them, silence between.
    fn clicks(gain: f32, per_second: f32, len: usize) -> Vec<f32> {
        let mut noise = Noise(7);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let period = (RATE as f32 / per_second) as usize;
        let burst = seconds(0.01);
        (0..len)
            .map(|i| {
                let at = i % period;
                if at < burst {
                    let decay = 1.0 - at as f32 / burst as f32;
                    gain * decay * noise.next()
                } else {
                    0.0
                }
            })
            .collect()
    }

    /// A chord of three sines with a little noise, swelling once a second.
    fn music(gain: f32, len: usize) -> Vec<f32> {
        let mut noise = Noise(11);
        (0..len)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let swell = 0.6 + 0.4 * (std::f32::consts::TAU * t).sin();
                let tones: f32 = [220.0, 277.18, 329.63]
                    .iter()
                    .map(|hz| (std::f32::consts::TAU * hz * t).sin())
                    .sum();
                gain * swell * (0.25 * tones + 0.05 * noise.next())
            })
            .collect()
    }

    fn features_of(samples: &[f32], channels: usize) -> SoundFeatures {
        let mut extractor = FeatureExtractor::new(RATE, channels);
        for chunk in samples.chunks(4096 * channels) {
            extractor.add(chunk);
        }
        extractor.finish(0.0).unwrap()
    }

    /// The distance of two feature vectors, each value scaled by a typical
    /// spread, so that no unit dominates.
    fn distance(a: &SoundFeatures, b: &SoundFeatures) -> f64 {
        let spread = |i: usize| match i {
            0..=11 => 2.0,
            12..=23 => 1.0,
            24..=26 => 1.0,
            27 => 10.0,
            28 => 1.0,
            29 => 2.0,
            30 => 0.5,
            31 | 32 => 0.2,
            _ => 3.0,
        };
        (0..SOUND_FEATURES_LEN)
            .map(|i| (f64::from(a.0[i] - b.0[i]) / spread(i)).powi(2))
            .sum::<f64>()
            .sqrt()
    }

    #[test]
    fn frame_sizes_are_powers_of_two_near_fifty_milliseconds() {
        assert_eq!(frame_size(44_100), 2048);
        assert_eq!(frame_size(48_000), 2048);
        assert_eq!(frame_size(96_000), 4096);
        assert_eq!(frame_size(22_050), 1024);
        assert_eq!(frame_size(8_000), 256);
    }

    #[test]
    fn kinds_of_sound_land_far_apart_and_gains_do_not_matter() {
        let len = seconds(6.0);
        let sine_loud = features_of(&sine(440.0, 0.5, len), 1);
        let sine_soft = features_of(&sine(440.0, 0.05, len), 1);
        let noise_loud = features_of(&noise(0.5, len, 3), 1);
        let noise_soft = features_of(&noise(0.1, len, 5), 1);
        let clicks_loud = features_of(&clicks(0.9, 4.0, len), 1);
        let clicks_soft = features_of(&clicks(0.2, 4.0, len), 1);
        let music_loud = features_of(&music(0.8, len), 1);
        let music_soft = features_of(&music(0.2, len), 1);

        let same = [
            (&sine_loud, &sine_soft),
            (&noise_loud, &noise_soft),
            (&clicks_loud, &clicks_soft),
            (&music_loud, &music_soft),
        ];
        let close = same.iter().map(|(a, b)| distance(a, b)).fold(0.0, f64::max);
        let kinds = [&sine_loud, &noise_loud, &clicks_loud, &music_loud];
        let mut far = f64::INFINITY;
        for (i, a) in kinds.iter().enumerate() {
            for b in &kinds[i + 1..] {
                far = far.min(distance(a, b));
            }
        }
        eprintln!("closest kinds {far}, farthest gains {close}");
        assert!(close < 0.5, "same sound at other gains: {close}");
        assert!(far > 4.0, "different kinds of sound: {far}");
        assert!(far > 10.0 * close, "{far} vs {close}");
    }

    #[test]
    fn the_values_tell_tones_noise_and_clicks_apart() {
        let len = seconds(6.0);
        let tone = features_of(&sine(1000.0, 0.5, len), 1);
        let hiss = features_of(&noise(0.5, len, 3), 1);
        let ticks = features_of(&clicks(0.9, 4.0, len), 1);
        // Flatness: noise is flat, a tone is not.
        assert!(hiss.0[27] > -6.0, "{}", hiss.0[27]);
        assert!(tone.0[27] < -30.0, "{}", tone.0[27]);
        // The centroid of a 1 kHz tone is near 1 kHz, that of white noise far
        // above.
        assert!((tone.0[24] - 1000f32.log2()).abs() < 0.3, "{}", tone.0[24]);
        assert!(hiss.0[24] > 12.5, "{}", hiss.0[24]);
        // Onsets: four clicks a second; steady sounds have hardly any.
        assert!((ticks.0[29] - 4.0).abs() < 0.8, "{}", ticks.0[29]);
        assert!(tone.0[29] < 0.5, "{}", tone.0[29]);
        assert!(hiss.0[29] < 1.0, "{}", hiss.0[29]);
        // Chroma: a tone has one strong pitch class.
        assert!(tone.0[31] > 0.6, "{}", tone.0[31]);
        assert!(hiss.0[31] < 0.4, "{}", hiss.0[31]);
        assert!(tone.0[32] < hiss.0[32]);
    }

    #[test]
    fn channels_are_mixed_down() {
        let len = seconds(3.0);
        let mono = sine(440.0, 0.5, len);
        let stereo: Vec<f32> = mono.iter().flat_map(|&x| [x, x]).collect();
        let a = features_of(&mono, 1);
        let b = features_of(&stereo, 2);
        assert!(distance(&a, &b) < 1e-3);
    }

    #[test]
    fn silence_has_no_features() {
        let mut extractor = FeatureExtractor::new(RATE, 2);
        extractor.add(&vec![0.0; seconds(2.0) * 2]);
        assert_eq!(extractor.finish(0.0), None);
    }

    #[test]
    fn the_loudness_spread_ignores_silence() {
        let seconds = [f64::NEG_INFINITY, -20.0, -10.0, -15.0, -80.0, -12.0];
        let spread = loudness_spread(seconds);
        assert!((spread - 10.0).abs() < 1e-9, "{spread}");
        assert_eq!(loudness_spread([-20.0]), 0.0);
    }
}
