// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A feed-forward dynamics compressor with a soft knee and lookahead.
//!
//! **Detection.** The level of a frame is its largest sample over all channels (the
//! channels are linked, so the stereo image does not move), in dBFS.
//!
//! **Gain.** The static curve leaves levels below the knee alone, divides what is
//! above the threshold by the ratio, and joins the two with a quadratic over the
//! knee's width. The gain reduction it asks for is smoothed in dB: it falls with the
//! attack time constant and rises with the release time constant. The makeup gain
//! is added afterwards.
//!
//! **Lookahead.** The audio is delayed by the lookahead, as in the limiter, so the
//! gain is already on its way down when a transient arrives.

use helikodu_model::player::CompressorActivity;
use helikodu_model::settings::{CompressorPreset, CompressorSettings};

use super::DspStage;
use super::plan::CompressorPlan;

/// The lowest level the detector works with, in dBFS.
const FLOOR_DB: f64 = -200.0;

/// The compressor for one rate and channel count.
#[derive(Debug, Clone)]
pub struct Compressor {
    plan: CompressorPlan,
    channels: usize,
    threshold: f64,
    slope: f64,
    knee: f64,
    makeup: f64,
    attack: f64,
    release: f64,
    /// The smoothed gain reduction, in dB (zero or below).
    reduction: f64,
    /// The largest reduction since the statistics started, in dB (zero or below).
    largest: f64,
    /// The delayed audio, frame-interleaved.
    delay: Box<[f32]>,
    delay_pos: usize,
}

impl Compressor {
    /// The compressor `settings` describe (whether or not it is enabled), for
    /// `channels` channels at `rate` Hz.
    pub fn new(settings: &CompressorSettings, rate: u32, channels: usize) -> Self {
        let values = settings.effective();
        let finite = |value: f64, default: f64| if value.is_finite() { value } else { default };
        let rate_hz = f64::from(rate.max(1));
        let channels = channels.max(1);
        let threshold = finite(values.threshold_db, -30.0).clamp(-90.0, 0.0);
        let ratio = finite(values.ratio, 4.0).clamp(1.0, 100.0);
        let knee = finite(values.knee_db, 0.0).clamp(0.0, 48.0);
        let makeup_db = finite(values.makeup_db, 0.0).clamp(-24.0, 48.0);
        let attack_ms = finite(values.attack_ms, 5.0).clamp(0.01, 1000.0);
        let release_ms = finite(values.release_ms, 250.0).clamp(1.0, 10_000.0);
        let lookahead_ms = finite(settings.lookahead_ms, 2.0).clamp(0.0, 20.0);
        let lookahead = (lookahead_ms / 1000.0 * rate_hz).round() as usize;
        let coefficient = |ms: f64| (-1.0 / (ms / 1000.0 * rate_hz)).exp();
        Self {
            plan: CompressorPlan {
                night: settings.preset == CompressorPreset::Night,
                threshold_db: threshold,
                ratio,
                makeup_db,
                attack_ms,
                release_ms,
                knee_db: knee,
                lookahead_ms,
            },
            channels,
            threshold,
            slope: 1.0 / ratio - 1.0,
            knee,
            makeup: makeup_db,
            attack: coefficient(attack_ms),
            release: coefficient(release_ms),
            reduction: 0.0,
            largest: 0.0,
            delay: vec![0.0; (lookahead + 1) * channels].into_boxed_slice(),
            delay_pos: 0,
        }
    }

    /// What the signal path shows.
    pub fn plan(&self) -> CompressorPlan {
        self.plan
    }

    /// The gain reduction the static curve asks for at `level` dBFS, in dB (zero or
    /// below).
    pub fn curve(&self, level: f64) -> f64 {
        let over = level - self.threshold;
        if 2.0 * over <= -self.knee {
            0.0
        } else if 2.0 * over.abs() < self.knee {
            let x = over + self.knee / 2.0;
            self.slope * x * x / (2.0 * self.knee)
        } else {
            self.slope * over
        }
    }

    /// The gain reduction applied now, in dB (zero or below).
    pub fn reduction_db(&self) -> f64 {
        self.reduction
    }

    /// What the compressor has done: the reduction now and the largest one since
    /// [`reset_activity`](Self::reset_activity), as positive dB.
    pub fn activity(&self) -> CompressorActivity {
        CompressorActivity {
            reduction_db: -self.reduction,
            largest_db: -self.largest,
        }
    }

    /// Starts the statistics anew, for example for a new track.
    pub fn reset_activity(&mut self) {
        self.largest = self.reduction;
    }

    /// Carries on the statistics of `other`, the compressor this one replaces.
    pub fn continue_activity(&mut self, other: &Self) {
        self.largest = self.largest.min(other.largest);
    }
}

impl DspStage for Compressor {
    fn name(&self) -> &'static str {
        "compressor"
    }

    fn latency(&self) -> usize {
        self.delay.len() / self.channels - 1
    }

    fn process(&mut self, block: &mut [f32]) {
        let channels = self.channels;
        let slots = self.delay.len() / channels;
        for frame in block.chunks_exact_mut(channels) {
            let peak = frame
                .iter()
                .fold(0.0f64, |peak, sample| peak.max(f64::from(sample.abs())));
            let level = if peak > 0.0 {
                (20.0 * peak.log10()).max(FLOOR_DB)
            } else {
                FLOOR_DB
            };
            let target = self.curve(level);
            let coefficient = if target < self.reduction {
                self.attack
            } else {
                self.release
            };
            self.reduction = target + (self.reduction - target) * coefficient;
            if self.reduction > -1e-12 {
                // Keeps the smoothing from creeping into subnormal numbers.
                self.reduction = 0.0;
            }
            self.largest = self.largest.min(self.reduction);
            let gain = 10f64.powf((self.reduction + self.makeup) / 20.0);

            let write = self.delay_pos * channels;
            self.delay[write..write + channels].copy_from_slice(frame);
            self.delay_pos = (self.delay_pos + 1) % slots;
            let read = self.delay_pos * channels;
            for (out, delayed) in frame.iter_mut().zip(&self.delay[read..read + channels]) {
                *out = (f64::from(*delayed) * gain) as f32;
            }
        }
    }

    fn reset(&mut self) {
        self.reduction = 0.0;
        self.delay.fill(0.0);
        self.delay_pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn custom(threshold_db: f64, ratio: f64, knee_db: f64, makeup_db: f64) -> CompressorSettings {
        CompressorSettings {
            enabled: true,
            preset: CompressorPreset::Custom,
            threshold_db,
            ratio,
            knee_db,
            makeup_db,
            attack_ms: 1.0,
            release_ms: 50.0,
            lookahead_ms: 2.0,
        }
    }

    fn db(x: f32) -> f64 {
        20.0 * f64::from(x.abs()).log10()
    }

    #[test]
    fn the_static_curve_has_a_soft_knee() {
        let c = Compressor::new(&custom(-20.0, 4.0, 10.0, 0.0), RATE, 2);
        assert_eq!(c.curve(-40.0), 0.0);
        assert_eq!(c.curve(-25.0), 0.0);
        assert!((c.curve(0.0) - -15.0).abs() < 1e-12);
        assert!((c.curve(-10.0) - -7.5).abs() < 1e-12);
        // Inside the knee the reduction grows smoothly and stays continuous.
        let at = |l: f64| c.curve(l);
        assert!(at(-20.0) < 0.0 && at(-20.0) > -2.0);
        assert!((at(-15.0 - 1e-9) - at(-15.0 + 1e-9)).abs() < 1e-6);
        assert!((at(-25.0 - 1e-9) - at(-25.0 + 1e-9)).abs() < 1e-6);
        let hard = Compressor::new(&custom(-20.0, 2.0, 0.0, 0.0), RATE, 1);
        assert_eq!(hard.curve(-20.0), 0.0);
        assert!((hard.curve(-10.0) - -5.0).abs() < 1e-12);
    }

    #[test]
    fn a_step_settles_on_the_curve_after_the_lookahead() {
        let mut c = Compressor::new(&custom(-20.0, 4.0, 0.0, 6.0), RATE, 2);
        let lookahead = c.latency();
        assert_eq!(lookahead, 96);
        // A quiet signal passes with the makeup gain only, delayed.
        let mut block = vec![0.01f32; 2 * 4800];
        c.process(&mut block);
        assert_eq!(block[0], 0.0);
        assert_eq!(block[2 * (lookahead - 1)], 0.0);
        assert!((db(block[2 * lookahead]) - (-40.0 + 6.0)).abs() < 1e-3);
        // A loud step (0 dBFS) settles at -20 + 20/4 + 6 = -9 dBFS.
        let mut block = vec![1.0f32; 2 * 9600];
        c.process(&mut block);
        let last = *block.last().unwrap();
        assert!((db(last) - -9.0).abs() < 0.01, "{}", db(last));
        // The gain never rises above the makeup gain while it falls.
        assert!(block.iter().all(|s| db(*s) <= 6.0 + 1e-6));
    }

    #[test]
    fn the_release_brings_the_gain_back() {
        let mut c = Compressor::new(&custom(-30.0, 10.0, 0.0, 0.0), RATE, 1);
        let mut loud = vec![0.9f32; 9600];
        c.process(&mut loud);
        assert!(c.reduction_db() < -20.0);
        let mut quiet = vec![0.0f32; 96_000];
        c.process(&mut quiet);
        assert_eq!(c.reduction_db(), 0.0);
        assert!(quiet.iter().all(|s| s.is_finite() && !s.is_subnormal()));
        // The largest reduction is kept until the statistics start anew.
        let activity = c.activity();
        assert_eq!(activity.reduction_db, 0.0);
        assert!(activity.largest_db > 20.0, "{activity:?}");
        c.reset_activity();
        assert_eq!(c.activity().largest_db, 0.0);
    }

    #[test]
    fn night_mode_lifts_quiet_and_holds_loud_passages() {
        let settings = CompressorSettings {
            enabled: true,
            ..CompressorSettings::default()
        };
        let mut c = Compressor::new(&settings, RATE, 2);
        assert!(c.plan().night);
        let mut quiet = vec![0.003f32; 2 * 9600]; // about -50 dBFS
        c.process(&mut quiet);
        let mut loud = vec![0.9f32; 2 * 19_200];
        c.process(&mut loud);
        let quiet_out = db(*quiet.last().unwrap());
        let loud_out = db(*loud.last().unwrap());
        assert!(quiet_out > -40.0, "{quiet_out}");
        assert!(loud_out < -10.0, "{loud_out}");
        // 49 dB of input range become about 30.
        assert!(loud_out - quiet_out < 32.0);
    }
}
