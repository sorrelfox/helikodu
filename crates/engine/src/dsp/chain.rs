// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSP chain: the stages in their fixed order, and click-free changes.

use std::f64::consts::PI;
use std::time::Duration;

use helikodu_model::settings::DspSettings;

use super::plan::DspPlan;
use super::{ChannelMixer, Crossfeed, DspStage, Equalizer, Limiter, LimiterActivity, guard};

/// Frames processed at once; the crossfade's scratch buffer holds this many.
const CHUNK: usize = 512;
/// How much of the input the chain keeps, to bring new stages up to date.
const HISTORY: Duration = Duration::from_millis(100);

/// One set of stages built from one version of the settings.
#[derive(Debug)]
struct Stages {
    equalizer: Option<Equalizer>,
    crossfeed: Option<Crossfeed>,
    mixer: Option<ChannelMixer>,
    limiter: Option<Limiter>,
    plan: DspPlan,
}

impl Stages {
    fn new(settings: &DspSettings, rate: u32, channels: usize) -> Self {
        let eq = &settings.equalizer;
        let equalizer = eq
            .enabled
            .then(|| Equalizer::new(eq, rate, channels))
            .filter(|stage| !stage.is_identity());
        let crossfeed = (settings.crossfeed.enabled && channels == 2).then(|| {
            let (cutoff, level) = settings.crossfeed.effective();
            Crossfeed::new(cutoff, level, rate)
        });
        let mix = &settings.channel_mix;
        let mixer = (mix.enabled && channels >= 2)
            .then(|| ChannelMixer::new(mix.channels, mix.balance_db, channels))
            .filter(|stage| !stage.is_identity());
        let limiter = settings.limiter.enabled.then(|| {
            Limiter::new(
                settings.limiter.lookahead_ms,
                settings.limiter.ceiling_db,
                rate,
                channels,
            )
        });
        let plan = DspPlan {
            equalizer: equalizer.as_ref().map(Equalizer::plan),
            crossfeed: crossfeed.as_ref().map(Crossfeed::plan),
            mixer: mixer.as_ref().map(ChannelMixer::plan),
            limiter: limiter.as_ref().map(Limiter::plan),
        };
        Self {
            equalizer,
            crossfeed,
            mixer,
            limiter,
            plan,
        }
    }

    fn is_active(&self) -> bool {
        !self.plan.is_empty()
    }

    /// Whether `other` sounds the same: the same stages with the same values.
    fn same_as(&self, other: &Self) -> bool {
        let sections = |s: &Self| {
            s.equalizer
                .as_ref()
                .map(|eq| (eq.sections().to_vec(), eq.preamp()))
        };
        self.plan == other.plan && sections(self) == sections(other)
    }

    /// Carries on what `old` counted (the limiter's statistics).
    fn continue_from(&mut self, old: &Self) {
        if let (Some(new), Some(old)) = (&mut self.limiter, &old.limiter) {
            new.continue_activity(old);
        }
    }

    fn process(&mut self, block: &mut [f32]) {
        if !self.is_active() {
            return;
        }
        if let Some(stage) = &mut self.equalizer {
            stage.process(block);
        }
        if let Some(stage) = &mut self.crossfeed {
            stage.process(block);
        }
        if let Some(stage) = &mut self.mixer {
            stage.process(block);
        }
        if let Some(stage) = &mut self.limiter {
            stage.process(block);
        }
        guard(block);
    }

    fn reset(&mut self) {
        if let Some(stage) = &mut self.equalizer {
            stage.reset();
        }
        if let Some(stage) = &mut self.crossfeed {
            stage.reset();
        }
        if let Some(stage) = &mut self.limiter {
            stage.reset();
        }
    }

    fn latency(&self) -> usize {
        self.limiter.as_ref().map_or(0, DspStage::latency)
    }
}

/// A crossfade from the stages that were current to the new ones.
#[derive(Debug)]
struct Fade {
    from: Stages,
    done: usize,
}

/// The DSP chain for one rate and channel count.
///
/// [`process`](Self::process) runs the stages on blocks of any length and never
/// allocates. [`set_settings`](Self::set_settings) builds the new stages (it
/// allocates) and crossfades to them over [`CROSSFADE`](Self::CROSSFADE); a change
/// that arrives during a crossfade waits for it to end, and only the latest such
/// change is kept.
///
/// New stages start where the old ones are: the chain keeps the last 100 ms of its
/// input (even while it does nothing) and runs the new stages over it before they
/// are faded in, so that their filters and the limiter's delay line hold what they
/// would hold had they run all along. The crossfade then joins two settled
/// signals.
#[derive(Debug)]
pub struct DspChain {
    rate: u32,
    channels: usize,
    settings: DspSettings,
    current: Stages,
    fade: Option<Fade>,
    pending: Option<Stages>,
    /// Stages faded out, kept until the next change so that processing never frees.
    retired: Option<Stages>,
    fade_frames: usize,
    scratch: Box<[f32]>,
    /// The last input, a ring of whole frames.
    history: Box<[f32]>,
    history_pos: usize,
    history_frames: usize,
    flushed: bool,
}

impl DspChain {
    /// How long a change of the settings is crossfaded.
    pub const CROSSFADE: Duration = Duration::from_millis(20);

    /// The chain `settings` describe, for `channels` channels in canonical order at
    /// `rate` Hz.
    pub fn new(settings: &DspSettings, rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        let fade_frames =
            ((f64::from(rate) * Self::CROSSFADE.as_secs_f64()).round() as usize).max(1);
        Self {
            rate,
            channels,
            settings: settings.clone(),
            current: Stages::new(settings, rate, channels),
            fade: None,
            pending: None,
            retired: None,
            fade_frames,
            scratch: vec![0.0; CHUNK * channels].into_boxed_slice(),
            history: vec![
                0.0;
                (f64::from(rate) * HISTORY.as_secs_f64()).ceil() as usize * channels
            ]
            .into_boxed_slice(),
            history_pos: 0,
            history_frames: 0,
            flushed: false,
        }
    }

    /// Whether the chain was built for `rate` Hz and `channels` channels.
    pub fn fits(&self, rate: u32, channels: usize) -> bool {
        self.rate == rate && self.channels == channels
    }

    /// The settings the chain follows.
    pub fn settings(&self) -> &DspSettings {
        &self.settings
    }

    /// The active stages, as the latest settings make them. Empty when the chain
    /// leaves the samples unchanged (once a crossfade to that has ended).
    pub fn plan(&self) -> DspPlan {
        self.newest().plan
    }

    /// Whether the chain changes the samples now: an active stage, or a crossfade.
    pub fn is_active(&self) -> bool {
        self.fade.is_some() || self.current.is_active()
    }

    /// The delay the chain adds, in frames.
    pub fn latency(&self) -> usize {
        self.current.latency()
    }

    /// What the limiter has done, if it runs.
    pub fn limiter_activity(&self) -> Option<LimiterActivity> {
        self.current.limiter.as_ref().map(Limiter::activity)
    }

    /// Starts the limiter's statistics anew, for example for a new track.
    pub fn reset_limiter_activity(&mut self) {
        if let Some(limiter) = &mut self.current.limiter {
            limiter.reset_activity();
        }
    }

    fn newest(&self) -> &Stages {
        self.pending.as_ref().unwrap_or(&self.current)
    }

    /// Follows new settings. Stages that sound the same are kept; otherwise new ones
    /// are built, take over the filter state, and are crossfaded in.
    pub fn set_settings(&mut self, settings: &DspSettings) {
        if *settings == self.settings {
            return;
        }
        self.settings = settings.clone();
        let stages = Stages::new(settings, self.rate, self.channels);
        if stages.same_as(self.newest()) {
            return;
        }
        self.retired = None;
        if !stages.is_active() && !self.is_active() {
            self.current = stages;
            self.pending = None;
            return;
        }
        if self.fade.is_some() {
            self.pending = Some(stages);
        } else {
            self.start_fade(stages);
        }
    }

    /// Brings `stages` up to date with the kept input and crossfades to them.
    fn start_fade(&mut self, mut stages: Stages) {
        self.warm(&mut stages);
        stages.continue_from(&self.current);
        let from = std::mem::replace(&mut self.current, stages);
        self.fade = Some(Fade { from, done: 0 });
    }

    /// Runs `stages` from rest over the kept input, discarding what they give.
    fn warm(&mut self, stages: &mut Stages) {
        stages.reset();
        let channels = self.channels;
        let slots = self.history.len() / channels;
        let mut at = (self.history_pos + slots - self.history_frames) % slots;
        let mut left = self.history_frames;
        while left > 0 {
            let frames = left.min(CHUNK).min(slots - at);
            let scratch = &mut self.scratch[..frames * channels];
            scratch.copy_from_slice(&self.history[at * channels..(at + frames) * channels]);
            stages.process(scratch);
            at = (at + frames) % slots;
            left -= frames;
        }
    }

    /// Keeps the input `block` in the history ring.
    fn remember(&mut self, block: &[f32]) {
        let channels = self.channels;
        let slots = self.history.len() / channels;
        if slots == 0 {
            return;
        }
        let frames = block.len() / channels;
        let skip = frames.saturating_sub(slots);
        let mut rest = &block[skip * channels..frames * channels];
        while !rest.is_empty() {
            let take = (rest.len() / channels).min(slots - self.history_pos);
            let at = self.history_pos * channels;
            self.history[at..at + take * channels].copy_from_slice(&rest[..take * channels]);
            self.history_pos = (self.history_pos + take) % slots;
            rest = &rest[take * channels..];
        }
        self.history_frames = (self.history_frames + frames).min(slots);
    }

    /// Ends a crossfade at once, for a new start where nothing fades.
    fn settle(&mut self) {
        if let Some(fade) = self.fade.take() {
            self.retired = Some(fade.from);
        }
        if let Some(mut stages) = self.pending.take() {
            stages.continue_from(&self.current);
            self.current = stages;
        }
    }

    /// Processes the interleaved frames of `block` in place. With no active stage
    /// and no crossfade the samples are not touched.
    pub fn process(&mut self, block: &mut [f32]) {
        if !self.is_active() {
            self.remember(block);
            return;
        }
        self.flushed = false;
        let channels = self.channels;
        for chunk in block.chunks_mut(CHUNK * channels) {
            self.remember(chunk);
            let Some(fade) = &mut self.fade else {
                self.current.process(chunk);
                continue;
            };
            let old = &mut self.scratch[..chunk.len()];
            old.copy_from_slice(chunk);
            fade.from.process(old);
            self.current.process(chunk);
            let length = self.fade_frames;
            for (i, (new, old)) in chunk
                .chunks_exact_mut(channels)
                .zip(old.chunks_exact(channels))
                .enumerate()
            {
                let at = fade.done + i;
                let w = if at >= length {
                    1.0
                } else {
                    0.5 - 0.5 * (PI * at as f64 / length as f64).cos()
                };
                for (n, o) in new.iter_mut().zip(old) {
                    *n = (f64::from(*o) * (1.0 - w) + f64::from(*n) * w) as f32;
                }
            }
            fade.done += chunk.len() / channels;
            if fade.done >= length {
                let ended = self.fade.take();
                self.retired = ended.map(|fade| fade.from);
                if let Some(stages) = self.pending.take() {
                    self.start_fade(stages);
                }
            }
        }
    }

    /// At the end of the audio: writes what the stages still hold (the limiter's
    /// lookahead) into `out` and returns the number of frames written. Returns 0
    /// when nothing is held or it was already written.
    pub fn flush(&mut self, out: &mut [f32]) -> usize {
        let frames = self.latency().min(out.len() / self.channels);
        if self.flushed || frames == 0 || !self.is_active() {
            return 0;
        }
        let block = &mut out[..frames * self.channels];
        block.fill(0.0);
        self.process(block);
        self.flushed = true;
        frames
    }

    /// Clears the stages' state and ends any crossfade, for a new start (a seek, a
    /// new stream). Pending changes take effect at once.
    pub fn reset(&mut self) {
        self.settle();
        self.current.reset();
        self.history_frames = 0;
        self.flushed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switched_off_it_touches_nothing() {
        let mut chain = DspChain::new(&DspSettings::default(), 48_000, 2);
        assert!(!chain.is_active());
        assert!(chain.plan().is_empty());
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.37).sin() * 1.5).collect();
        let mut block = input.clone();
        chain.process(&mut block);
        assert_eq!(block, input);
        // Stages that are on but leave the sound alone do not count.
        let mut settings = DspSettings::default();
        settings.equalizer.enabled = true;
        settings.channel_mix.enabled = true;
        chain.set_settings(&settings);
        assert!(!chain.is_active());
        chain.process(&mut block);
        assert_eq!(block, input);
    }

    #[test]
    fn crossfeed_needs_two_channels() {
        let mut settings = DspSettings::default();
        settings.crossfeed.enabled = true;
        assert!(DspChain::new(&settings, 48_000, 1).plan().is_empty());
        assert!(
            DspChain::new(&settings, 48_000, 2)
                .plan()
                .crossfeed
                .is_some()
        );
        assert!(
            DspChain::new(&settings, 48_000, 6)
                .plan()
                .crossfeed
                .is_none()
        );
    }

    #[test]
    fn a_change_crossfades_then_settles() {
        let mut settings = DspSettings::default();
        let mut chain = DspChain::new(&settings, 48_000, 2);
        settings.channel_mix.enabled = true;
        settings.channel_mix.balance_db = -6.0;
        chain.set_settings(&settings);
        assert!(chain.is_active());
        assert!(chain.plan().mixer.is_some());
        let mut block = vec![1.0f32; 2 * 2000];
        chain.process(&mut block);
        // The right channel moves from 1 down to -6 dB, monotonically.
        let right: Vec<f32> = block.iter().skip(1).step_by(2).copied().collect();
        assert!(right.windows(2).all(|w| w[1] <= w[0]));
        assert!((right[1999] - 0.501).abs() < 1e-3);
        assert!(chain.fade.is_none());
        // Back to neutral: fades back, then touches nothing again.
        settings.channel_mix.balance_db = 0.0;
        chain.set_settings(&settings);
        chain.process(&mut block);
        assert!(!chain.is_active());
        let mut quiet = vec![0.25f32; 64];
        chain.process(&mut quiet);
        assert!(quiet.iter().all(|s| *s == 0.25));
    }
}
