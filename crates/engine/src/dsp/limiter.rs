// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A true-peak lookahead limiter, the chain's safety net.
//!
//! **Detection.** Every frame's peak is the largest of its samples and of the
//! values between them, interpolated at [`OVERSAMPLE`] times the rate with a
//! Kaiser-windowed sinc, over all channels (the channels are linked, so the stereo
//! image does not move). The gain each frame needs is `ceiling / peak`, or 1.
//!
//! **Gain.** The needed gains pass a sliding minimum, an exponential release and a
//! moving average as long as the lookahead. The audio is delayed so that the
//! average has fully settled on a peak's gain before the peak, and stays there for
//! [`HOLD`] frames on either side, the reach of the interpolation filter. Every gain
//! applied is therefore at most the gain every frame near it needs, so the output
//! stays under the ceiling between the samples too. The limiter aims
//! [`MARGIN_DB`] below the ceiling, for what interpolation at a finite rate cannot
//! see.
//!
//! The attack is a smooth ramp as long as the lookahead; the release is
//! exponential with a time constant of [`RELEASE`] seconds.

use std::f64::consts::PI;

use super::DspStage;
use super::plan::LimiterPlan;

/// The oversampling factor of the peak detector.
const OVERSAMPLE: usize = 8;
/// The half-length of the interpolation filter, in frames.
const TAPS_HALF: usize = 16;
/// Frames on either side of a peak that are given its gain.
const HOLD: usize = TAPS_HALF;
/// How far below the ceiling the limiter aims, in dB.
const MARGIN_DB: f64 = 0.2;
/// The release time constant, in seconds.
const RELEASE: f64 = 0.06;
/// The Kaiser window's shape parameter.
const KAISER_BETA: f64 = 7.0;

/// What the limiter has done, for the activity readout.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LimiterActivity {
    /// The gain reduction applied to the last frame, in dB (0 or positive).
    pub reduction_db: f64,
    /// The largest reduction since the statistics were reset, in dB.
    pub largest_db: f64,
    /// How often limiting has started since the statistics were reset.
    pub times: u64,
}

/// The lookahead limiter for one rate and channel count.
#[derive(Debug, Clone)]
pub struct Limiter {
    plan: LimiterPlan,
    channels: usize,
    ceiling: f64,
    /// The interpolation filter: `OVERSAMPLE - 1` phases of `2 · TAPS_HALF` taps.
    kernel: Box<[f64]>,
    /// The detector's input per channel, each stored twice so that the last
    /// `2 · TAPS_HALF` samples are always one contiguous slice.
    history: Box<[f64]>,
    history_pos: usize,
    /// The sliding minimum of the needed gains, over `lookahead + 2 · HOLD` frames.
    minimum: SlidingMinimum,
    /// The gain after the release.
    envelope: f64,
    release: f64,
    /// The moving average over `lookahead` frames.
    average: Box<[f64]>,
    average_pos: usize,
    average_sum: f64,
    /// The delayed audio, frame-interleaved.
    delay: Box<[f32]>,
    delay_pos: usize,
    frame: u64,
    activity: LimiterActivity,
    limiting: bool,
}

impl Limiter {
    /// A limiter with `lookahead_ms` of lookahead and its ceiling at `ceiling_db`
    /// dBTP, for `channels` channels at `rate` Hz.
    pub fn new(lookahead_ms: f64, ceiling_db: f64, rate: u32, channels: usize) -> Self {
        let rate_hz = f64::from(rate);
        let channels = channels.max(1);
        let lookahead_ms = if lookahead_ms.is_finite() {
            lookahead_ms.clamp(0.5, 20.0)
        } else {
            2.0
        };
        let ceiling_db = if ceiling_db.is_finite() {
            ceiling_db.min(0.0)
        } else {
            -1.0
        };
        let lookahead = ((lookahead_ms / 1000.0 * rate_hz).round() as usize).max(1);
        let delay_frames = TAPS_HALF + lookahead - 1 + HOLD;
        let mut limiter = Self {
            plan: LimiterPlan {
                lookahead_ms,
                ceiling_db,
            },
            channels,
            ceiling: 10f64.powf((ceiling_db - MARGIN_DB) / 20.0),
            kernel: kernel(),
            history: vec![0.0; channels * 4 * TAPS_HALF].into_boxed_slice(),
            history_pos: 0,
            minimum: SlidingMinimum::new(lookahead + 2 * HOLD),
            envelope: 1.0,
            release: 1.0 - (-1.0 / (RELEASE * rate_hz)).exp(),
            average: vec![1.0; lookahead].into_boxed_slice(),
            average_pos: 0,
            average_sum: lookahead as f64,
            delay: vec![0.0; (delay_frames + 1) * channels].into_boxed_slice(),
            delay_pos: 0,
            frame: 0,
            activity: LimiterActivity::default(),
            limiting: false,
        };
        limiter.reset();
        limiter
    }

    /// What the signal path shows.
    pub fn plan(&self) -> LimiterPlan {
        self.plan
    }

    /// What the limiter has done.
    pub fn activity(&self) -> LimiterActivity {
        self.activity
    }

    /// Starts the statistics anew, for example for a new track.
    pub fn reset_activity(&mut self) {
        self.activity = LimiterActivity {
            reduction_db: self.activity.reduction_db,
            ..LimiterActivity::default()
        };
    }

    /// Carries on the statistics of `other`, the limiter this one replaces.
    pub fn continue_activity(&mut self, other: &Self) {
        self.activity.largest_db = other.activity.largest_db;
        self.activity.times = other.activity.times;
    }

    /// The peak of the frame `TAPS_HALF - 1` frames back, interpolated between its
    /// samples, over all channels.
    fn detect(&self) -> f64 {
        let taps = 2 * TAPS_HALF;
        let mut peak: f64 = 0.0;
        for channel in 0..self.channels {
            let start = channel * 2 * taps + self.history_pos;
            let window = &self.history[start..start + taps];
            peak = peak.max(window[TAPS_HALF - 1].abs());
            for phase in self.kernel.chunks_exact(taps) {
                let value: f64 = phase.iter().zip(window).map(|(h, x)| h * x).sum();
                peak = peak.max(value.abs());
            }
        }
        peak
    }

    fn tick(&mut self, frame: &mut [f32]) {
        let taps = 2 * TAPS_HALF;
        for (channel, sample) in frame.iter().enumerate() {
            let base = channel * 2 * taps;
            let x = f64::from(*sample);
            self.history[base + self.history_pos] = x;
            self.history[base + self.history_pos + taps] = x;
        }
        self.history_pos = (self.history_pos + 1) % taps;
        let peak = self.detect();
        let needed = if peak > self.ceiling {
            self.ceiling / peak
        } else {
            1.0
        };
        let minimum = self.minimum.push(self.frame, needed);
        self.frame += 1;
        self.envelope = minimum.min(self.envelope + (1.0 - self.envelope) * self.release);
        let old = std::mem::replace(&mut self.average[self.average_pos], self.envelope);
        self.average_sum += self.envelope - old;
        self.average_pos += 1;
        if self.average_pos == self.average.len() {
            self.average_pos = 0;
            // Recomputed once per round, so that rounding cannot accumulate.
            self.average_sum = self.average.iter().sum();
        }
        let gain = (self.average_sum / self.average.len() as f64).min(1.0);

        let channels = self.channels;
        let slots = self.delay.len() / channels;
        let write = self.delay_pos * channels;
        self.delay[write..write + channels].copy_from_slice(frame);
        self.delay_pos = (self.delay_pos + 1) % slots;
        let read = self.delay_pos * channels;
        for (out, delayed) in frame.iter_mut().zip(&self.delay[read..read + channels]) {
            *out = (f64::from(*delayed) * gain) as f32;
        }

        let limiting = gain < 1.0 - 1e-9;
        if limiting {
            let db = -20.0 * gain.log10();
            self.activity.reduction_db = db;
            self.activity.largest_db = self.activity.largest_db.max(db);
            if !self.limiting {
                self.activity.times += 1;
            }
        } else {
            self.activity.reduction_db = 0.0;
        }
        self.limiting = limiting;
    }
}

impl DspStage for Limiter {
    fn name(&self) -> &'static str {
        "limiter"
    }

    fn latency(&self) -> usize {
        self.delay.len() / self.channels - 1
    }

    fn process(&mut self, block: &mut [f32]) {
        let channels = self.channels;
        for frame in block.chunks_exact_mut(channels) {
            self.tick(frame);
        }
    }

    fn reset(&mut self) {
        self.history.fill(0.0);
        self.history_pos = 0;
        self.minimum.clear();
        self.envelope = 1.0;
        self.average.fill(1.0);
        self.average_pos = 0;
        self.average_sum = self.average.len() as f64;
        self.delay.fill(0.0);
        self.delay_pos = 0;
        self.frame = 0;
        self.limiting = false;
        self.activity.reduction_db = 0.0;
    }
}

/// The interpolation filter's phases `1 / OVERSAMPLE` to `(OVERSAMPLE - 1) /
/// OVERSAMPLE` of the way from one frame to the next, each normalized to unity gain
/// at DC. Tap `j` of a phase weighs the frame `j - (TAPS_HALF - 1)` from the
/// frame being interpolated from.
fn kernel() -> Box<[f64]> {
    let taps = 2 * TAPS_HALF;
    let mut kernel = vec![0.0; (OVERSAMPLE - 1) * taps];
    for (p, phase) in kernel.chunks_exact_mut(taps).enumerate() {
        let t = (p + 1) as f64 / OVERSAMPLE as f64;
        for (j, h) in phase.iter_mut().enumerate() {
            let u = t - (j as f64 - (TAPS_HALF as f64 - 1.0));
            *h = sinc(u) * kaiser(u / TAPS_HALF as f64);
        }
        let sum: f64 = phase.iter().sum();
        for h in phase.iter_mut() {
            *h /= sum;
        }
    }
    kernel.into_boxed_slice()
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

/// The Kaiser window at `x` in -1..1.
fn kaiser(x: f64) -> f64 {
    let inside = (1.0 - x * x).max(0.0);
    bessel_i0(KAISER_BETA * inside.sqrt()) / bessel_i0(KAISER_BETA)
}

/// The modified Bessel function of the first kind, order zero.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..50 {
        term *= half / k as f64;
        let add = term * term;
        sum += add;
        if add < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// The minimum of the last `window` values, in constant amortized time and with no
/// allocation after it is built: a monotonic queue in a ring.
#[derive(Debug, Clone)]
struct SlidingMinimum {
    window: u64,
    entries: Box<[(u64, f64)]>,
    head: usize,
    len: usize,
}

impl SlidingMinimum {
    fn new(window: usize) -> Self {
        Self {
            window: window as u64,
            entries: vec![(0, 0.0); window + 1].into_boxed_slice(),
            head: 0,
            len: 0,
        }
    }

    fn slot(&self, i: usize) -> usize {
        (self.head + i) % self.entries.len()
    }

    /// Adds `value` as the entry at `index` and returns the minimum of the window
    /// that ends there.
    fn push(&mut self, index: u64, value: f64) -> f64 {
        while self.len > 0 && self.entries[self.slot(self.len - 1)].1 >= value {
            self.len -= 1;
        }
        let tail = self.slot(self.len);
        self.entries[tail] = (index, value);
        self.len += 1;
        while self.entries[self.head].0 + self.window <= index {
            self.head = (self.head + 1) % self.entries.len();
            self.len -= 1;
        }
        self.entries[self.head].1
    }

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sliding_minimum() {
        let mut minimum = SlidingMinimum::new(3);
        let values = [5.0, 3.0, 4.0, 6.0, 7.0, 1.0, 2.0, 2.0, 9.0];
        for (i, value) in values.iter().enumerate() {
            let expected = values[i.saturating_sub(2)..=i]
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min);
            assert_eq!(minimum.push(i as u64, *value), expected, "at {i}");
        }
    }

    #[test]
    fn the_interpolator_reconstructs_a_sine() {
        let kernel = kernel();
        let taps = 2 * TAPS_HALF;
        let hz = 0.1;
        let x = |n: f64| (2.0 * PI * hz * n + 0.3).sin();
        for (p, phase) in kernel.chunks_exact(taps).enumerate() {
            let t = (p + 1) as f64 / OVERSAMPLE as f64;
            let value: f64 = phase
                .iter()
                .enumerate()
                .map(|(j, h)| h * x(j as f64 - (TAPS_HALF as f64 - 1.0)))
                .sum();
            assert!(
                (value - x(t)).abs() < 1e-3,
                "phase {p}: {value} vs {}",
                x(t)
            );
        }
    }

    #[test]
    fn quiet_audio_is_only_delayed() {
        let mut limiter = Limiter::new(2.0, -1.0, 48_000, 2);
        let latency = limiter.latency();
        assert_eq!(latency, TAPS_HALF + 96 - 1 + HOLD);
        let input: Vec<f32> = (0..4000).map(|i| ((i as f32) * 0.01).sin() * 0.5).collect();
        let mut block = input.clone();
        limiter.process(&mut block);
        assert_eq!(&block[2 * latency..], &input[..input.len() - 2 * latency]);
        assert!(block[..2 * latency].iter().all(|s| *s == 0.0));
        assert_eq!(limiter.activity().times, 0);
    }
}
