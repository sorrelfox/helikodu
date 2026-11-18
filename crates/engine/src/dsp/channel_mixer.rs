// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The channel mixer: mono, left/right swap and balance on the front pair (the
//! first two channels in canonical order). Other channels pass unchanged.

use helikodu_model::settings::ChannelMode;

use super::DspStage;
use super::plan::MixerPlan;

/// Mono, swap and balance for a stream of two or more channels.
#[derive(Debug, Clone)]
pub struct ChannelMixer {
    plan: MixerPlan,
    channels: usize,
    /// The gains of the left and right output.
    left_gain: f64,
    right_gain: f64,
}

impl ChannelMixer {
    /// A mixer for `channels` channels (at least two).
    pub fn new(mode: ChannelMode, balance_db: f64, channels: usize) -> Self {
        let balance_db = if balance_db.is_finite() {
            balance_db
        } else {
            0.0
        };
        let cut = 10f64.powf(-balance_db.abs() / 20.0);
        let (left_gain, right_gain) = if balance_db < 0.0 {
            (1.0, cut)
        } else {
            (cut, 1.0)
        };
        Self {
            plan: MixerPlan {
                channels: mode,
                balance_db,
            },
            channels: channels.max(2),
            left_gain,
            right_gain,
        }
    }

    /// Whether the mixer leaves every sample unchanged.
    pub fn is_identity(&self) -> bool {
        self.plan.channels == ChannelMode::Stereo && self.plan.balance_db == 0.0
    }

    /// What the signal path shows.
    pub fn plan(&self) -> MixerPlan {
        self.plan
    }
}

impl DspStage for ChannelMixer {
    fn name(&self) -> &'static str {
        "channel mix"
    }

    fn process(&mut self, block: &mut [f32]) {
        for frame in block.chunks_exact_mut(self.channels) {
            let (left, right) = (f64::from(frame[0]), f64::from(frame[1]));
            let (left, right) = match self.plan.channels {
                ChannelMode::Stereo => (left, right),
                ChannelMode::Mono => {
                    let mid = 0.5 * (left + right);
                    (mid, mid)
                }
                ChannelMode::Swap => (right, left),
            };
            frame[0] = (left * self.left_gain) as f32;
            frame[1] = (right * self.right_gain) as f32;
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(mixer: &mut ChannelMixer, block: &[f32]) -> Vec<f32> {
        let mut block = block.to_vec();
        mixer.process(&mut block);
        block
    }

    #[test]
    fn mono_swap_and_balance() {
        let input = [0.5, -0.25, 0.1, 0.2];
        let mut mono = ChannelMixer::new(ChannelMode::Mono, 0.0, 2);
        assert_eq!(run(&mut mono, &input), [0.125, 0.125, 0.15, 0.15]);
        let mut swap = ChannelMixer::new(ChannelMode::Swap, 0.0, 2);
        assert_eq!(run(&mut swap, &input), [-0.25, 0.5, 0.2, 0.1]);
        // 6 dB to the left halves (about) the right channel only.
        let mut left = ChannelMixer::new(ChannelMode::Stereo, -6.0206, 2);
        let out = run(&mut left, &input);
        assert_eq!(out[0], 0.5);
        assert!((out[1] + 0.125).abs() < 1e-6);
        let mut right = ChannelMixer::new(ChannelMode::Stereo, 6.0206, 2);
        let out = run(&mut right, &input);
        assert!((out[0] - 0.25).abs() < 1e-6);
        assert_eq!(out[1], -0.25);
        assert!(ChannelMixer::new(ChannelMode::Stereo, 0.0, 2).is_identity());
    }

    #[test]
    fn channels_beyond_the_front_pair_pass() {
        let mut swap = ChannelMixer::new(ChannelMode::Swap, 0.0, 3);
        assert_eq!(run(&mut swap, &[0.1, 0.2, 0.3]), [0.2, 0.1, 0.3]);
    }
}
