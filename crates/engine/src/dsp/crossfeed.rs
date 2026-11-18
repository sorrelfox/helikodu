// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Crossfeed for headphones, after Benjamin Bauer's stereophonic-to-binaural
//! network as the bs2b library implements it.
//!
//! Each ear gets its own channel through a first-order high-frequency boost and the
//! opposite channel through a first-order low pass. The two are matched so that at
//! low frequencies a sound panned fully to one side is `level` dB quieter in the
//! other ear, while at high frequencies the channels stay apart, as with
//! loudspeakers. The output is scaled so that a centred (mono) low-frequency signal
//! keeps its level.

use std::f64::consts::PI;

use super::DspStage;
use super::plan::CrossfeedPlan;

/// The crossfeed filters for a stereo stream.
#[derive(Debug, Clone)]
pub struct Crossfeed {
    plan: CrossfeedPlan,
    /// Low pass of the crossfed signal: `lo = a0_lo·x + b1_lo·lo`.
    a0_lo: f64,
    b1_lo: f64,
    /// High boost of the direct signal: `hi = a0_hi·x + a1_hi·x[n-1] + b1_hi·hi`.
    a0_hi: f64,
    a1_hi: f64,
    b1_hi: f64,
    /// Overall gain that keeps a mono signal's low-frequency level.
    gain: f64,
    lo: [f64; 2],
    hi: [f64; 2],
    previous: [f64; 2],
}

impl Crossfeed {
    /// Crossfeed with its cut-off at `cutoff_hz` and a low-frequency separation of
    /// `level_db`, at `rate` Hz.
    pub fn new(cutoff_hz: u32, level_db: f64, rate: u32) -> Self {
        let rate = f64::from(rate);
        let level = if level_db.is_finite() {
            level_db.clamp(0.5, 20.0)
        } else {
            4.5
        };
        let fc_lo = f64::from(cutoff_hz).clamp(100.0, rate * 0.4);
        let gb_lo = level * -5.0 / 6.0 - 3.0;
        let gb_hi = level / 6.0 - 3.0;
        let g_lo = 10f64.powf(gb_lo / 20.0);
        let g_hi = 1.0 - 10f64.powf(gb_hi / 20.0);
        let fc_hi = (fc_lo * 2f64.powf((gb_lo - 20.0 * g_hi.log10()) / 12.0)).min(rate * 0.4);
        let x_lo = (-2.0 * PI * fc_lo / rate).exp();
        let x_hi = (-2.0 * PI * fc_hi / rate).exp();
        Self {
            plan: CrossfeedPlan {
                cutoff_hz,
                level_db,
            },
            a0_lo: g_lo * (1.0 - x_lo),
            b1_lo: x_lo,
            a0_hi: 1.0 - g_hi * (1.0 - x_hi),
            a1_hi: -x_hi,
            b1_hi: x_hi,
            gain: 1.0 / (1.0 - g_hi + g_lo),
            lo: [0.0; 2],
            hi: [0.0; 2],
            previous: [0.0; 2],
        }
    }

    /// What the signal path shows.
    pub fn plan(&self) -> CrossfeedPlan {
        self.plan
    }

    fn tick(&mut self, left: f64, right: f64) -> (f64, f64) {
        let input = [left, right];
        for c in 0..2 {
            self.lo[c] = self.a0_lo * input[c] + self.b1_lo * self.lo[c];
            self.hi[c] =
                self.a0_hi * input[c] + self.a1_hi * self.previous[c] + self.b1_hi * self.hi[c];
        }
        self.previous = input;
        (
            (self.hi[0] + self.lo[1]) * self.gain,
            (self.hi[1] + self.lo[0]) * self.gain,
        )
    }
}

impl DspStage for Crossfeed {
    fn name(&self) -> &'static str {
        "crossfeed"
    }

    fn process(&mut self, block: &mut [f32]) {
        for [left, right] in block.as_chunks_mut::<2>().0 {
            let (l, r) = self.tick(f64::from(*left), f64::from(*right));
            *left = l as f32;
            *right = r as f32;
        }
        for state in self.lo.iter_mut().chain(&mut self.hi) {
            if state.abs() < 1e-200 {
                *state = 0.0;
            }
        }
    }

    fn reset(&mut self) {
        self.lo = [0.0; 2];
        self.hi = [0.0; 2];
        self.previous = [0.0; 2];
    }
}

#[cfg(test)]
mod tests {
    use helikodu_model::settings::CrossfeedPreset;

    use super::*;

    const RATE: u32 = 48_000;

    /// The steady-state levels, in dB, in the left and right outputs of a sine at
    /// `hz` fed into the left input only (or both when `mono`).
    fn levels(crossfeed: &mut Crossfeed, hz: f64, mono: bool) -> (f64, f64) {
        crossfeed.reset();
        let (mut left, mut right): (f64, f64) = (0.0, 0.0);
        let frames = RATE as usize;
        for n in 0..frames {
            let x = (2.0 * PI * hz * n as f64 / f64::from(RATE)).sin();
            let (l, r) = crossfeed.tick(x, if mono { x } else { 0.0 });
            if n > frames / 2 {
                left = left.max(l.abs());
                right = right.max(r.abs());
            }
        }
        (20.0 * left.log10(), 20.0 * right.log10())
    }

    #[test]
    fn the_low_frequency_separation_is_the_level() {
        for preset in [
            CrossfeedPreset::Default,
            CrossfeedPreset::ChuMoy,
            CrossfeedPreset::JanMeier,
        ] {
            let (cutoff, level) = preset.values().unwrap();
            let mut crossfeed = Crossfeed::new(cutoff, level, RATE);
            let (left, right) = levels(&mut crossfeed, 30.0, false);
            let ild = left - right;
            assert!((ild - level).abs() < 0.2, "{preset:?}: {ild} dB");
            // High frequencies stay far apart.
            let (left, right) = levels(&mut crossfeed, 8000.0, false);
            assert!(left - right > 20.0, "{preset:?}: {left} {right}");
        }
    }

    #[test]
    fn a_centred_signal_keeps_its_low_frequency_level() {
        let mut crossfeed = Crossfeed::new(700, 4.5, RATE);
        let (left, right) = levels(&mut crossfeed, 30.0, true);
        assert!(left.abs() < 0.1 && right.abs() < 0.1, "{left} {right}");
        // And never rises above it.
        for hz in [100.0, 700.0, 2000.0, 10_000.0] {
            let (left, _) = levels(&mut crossfeed, hz, true);
            assert!(left < 0.1, "{hz} Hz: {left}");
        }
    }

    #[test]
    fn a_stronger_crossfeed_mixes_in_more() {
        let mut mild = Crossfeed::new(650, 9.5, RATE);
        let mut strong = Crossfeed::new(700, 4.5, RATE);
        let (l1, r1) = levels(&mut mild, 200.0, false);
        let (l2, r2) = levels(&mut strong, 200.0, false);
        assert!(l1 - r1 > l2 - r2);
    }
}
