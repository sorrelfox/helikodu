// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSP chain: the stages in the order of the settings, and click-free changes.

use std::f64::consts::PI;
use std::sync::Arc;
use std::time::Duration;

use helikodu_model::player::CompressorActivity;
use helikodu_model::settings::{ChainStage, ConvolutionSettings, DspSettings, EqualizerSettings};

use super::impulse::{self, Impulse};
use super::plan::DspPlan;
use super::{
    ChannelMixer, Compressor, Convolver, Crossfeed, DspStage, Equalizer, Limiter, LimiterActivity,
    PitchShifter, guard,
};

/// Frames processed at once; the crossfade's scratch buffer holds this many.
const CHUNK: usize = 512;
/// How much of the input the chain keeps, to bring new stages up to date.
const HISTORY: Duration = Duration::from_millis(100);

/// One built stage.
#[derive(Debug)]
enum Stage {
    Pitch(PitchShifter),
    Equalizer(Equalizer),
    Convolution(Convolver),
    Crossfeed(Crossfeed),
    Mixer(ChannelMixer),
    Compressor(Compressor),
    Limiter(Limiter),
}

impl Stage {
    fn as_dyn(&mut self) -> &mut dyn DspStage {
        match self {
            Self::Pitch(s) => s,
            Self::Equalizer(s) => s,
            Self::Convolution(s) => s,
            Self::Crossfeed(s) => s,
            Self::Mixer(s) => s,
            Self::Compressor(s) => s,
            Self::Limiter(s) => s,
        }
    }

    fn latency(&self) -> usize {
        match self {
            Self::Pitch(s) => s.latency(),
            Self::Convolution(s) => s.latency(),
            Self::Compressor(s) => s.latency(),
            Self::Limiter(s) => s.latency(),
            Self::Equalizer(_) | Self::Crossfeed(_) | Self::Mixer(_) => 0,
        }
    }
}

/// What names an impulse response: the folder, the files by rate, and the rate.
type ImpulseKey = (
    Option<std::path::PathBuf>,
    Vec<(u32, std::path::PathBuf)>,
    u32,
);

/// The impulse response last read, so that other changes do not read it again.
#[derive(Debug, Default)]
pub(super) struct ImpulseCache {
    key: Option<ImpulseKey>,
    found: Option<Result<Arc<Impulse>, String>>,
}

impl ImpulseCache {
    /// The impulse response `settings` give for `rate` Hz, or why there is none.
    fn get(&mut self, settings: &ConvolutionSettings, rate: u32) -> Result<Arc<Impulse>, String> {
        let key = (
            settings.folder.clone(),
            settings
                .files
                .iter()
                .map(|f| (f.rate_hz, f.path.clone()))
                .collect(),
            rate,
        );
        if self.key.as_ref() != Some(&key) || self.found.is_none() {
            self.found = Some(
                impulse::find(settings, rate)
                    .map(Arc::new)
                    .map_err(|e| e.to_string()),
            );
            self.key = Some(key);
        }
        self.found.clone().unwrap_or_else(|| Err(String::new()))
    }
}

/// One set of stages built from one version of the settings.
#[derive(Debug)]
struct Stages {
    stages: Vec<Stage>,
    plan: DspPlan,
    /// The equalizer's sections and preamp, to tell equal equalizers apart.
    equalizer: Option<(Vec<super::Coefficients>, f64)>,
    impulse: Option<Arc<Impulse>>,
    /// Why convolution is switched on but does not run.
    problem: Option<String>,
}

impl Stages {
    fn new(settings: &DspSettings, rate: u32, channels: usize, cache: &mut ImpulseCache) -> Self {
        let mut stages = Vec::new();
        let mut plan = DspPlan::default();
        let order = settings.effective_order();
        for (slot, stage) in plan.order.iter_mut().zip(&order) {
            *slot = *stage;
        }
        let mut equalizer = None;
        let mut impulse = None;
        let mut problem = None;
        for stage in order {
            if !settings.is_enabled(stage) {
                continue;
            }
            match stage {
                ChainStage::Pitch => {
                    let s = PitchShifter::new(&settings.pitch, rate, channels);
                    if !s.is_identity() {
                        plan.pitch = Some(s.plan());
                        stages.push(Stage::Pitch(s));
                    }
                }
                ChainStage::Equalizer => {
                    let s = Equalizer::new(&settings.equalizer, rate, channels);
                    if !s.is_identity() {
                        plan.equalizer = Some(s.plan());
                        equalizer = Some((s.sections().to_vec(), s.preamp()));
                        stages.push(Stage::Equalizer(s));
                    }
                }
                ChainStage::Convolution => match cache.get(&settings.convolution, rate) {
                    Ok(found) => {
                        let s = Convolver::new(&found, &settings.convolution, rate, channels);
                        plan.convolution = Some(s.plan());
                        impulse = Some(found);
                        stages.push(Stage::Convolution(s));
                    }
                    Err(why) => {
                        // Named files that cannot be used leave the stage out;
                        // with none named it simply has nothing to do.
                        plan.convolution_left_out = settings.convolution.has_source();
                        problem = Some(why);
                    }
                },
                ChainStage::Crossfeed => {
                    if channels == 2 {
                        let (cutoff, level) = settings.crossfeed.effective();
                        let s = Crossfeed::new(cutoff, level, rate);
                        plan.crossfeed = Some(s.plan());
                        stages.push(Stage::Crossfeed(s));
                    }
                }
                ChainStage::ChannelMix => {
                    let mix = &settings.channel_mix;
                    if channels >= 2 {
                        let s = ChannelMixer::new(mix.channels, mix.balance_db, channels);
                        if !s.is_identity() {
                            plan.mixer = Some(s.plan());
                            stages.push(Stage::Mixer(s));
                        }
                    }
                }
                ChainStage::Compressor => {
                    let s = Compressor::new(&settings.compressor, rate, channels);
                    plan.compressor = Some(s.plan());
                    stages.push(Stage::Compressor(s));
                }
                ChainStage::Limiter => {
                    let s = Limiter::new(
                        settings.limiter.lookahead_ms,
                        settings.limiter.ceiling_db,
                        rate,
                        channels,
                    );
                    plan.limiter = Some(s.plan());
                    stages.push(Stage::Limiter(s));
                }
            }
        }
        Self {
            stages,
            plan,
            equalizer,
            impulse,
            problem,
        }
    }

    fn is_active(&self) -> bool {
        !self.stages.is_empty()
    }

    /// Whether `other` sounds the same: the same stages with the same values.
    fn same_as(&self, other: &Self) -> bool {
        let impulse = match (&self.impulse, &other.impulse) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        (self.plan == other.plan || (self.plan.is_empty() && other.plan.is_empty()))
            && self.equalizer == other.equalizer
            && impulse
    }

    fn limiter(&self) -> Option<&Limiter> {
        self.stages.iter().find_map(|s| match s {
            Stage::Limiter(l) => Some(l),
            _ => None,
        })
    }

    fn limiter_mut(&mut self) -> Option<&mut Limiter> {
        self.stages.iter_mut().find_map(|s| match s {
            Stage::Limiter(l) => Some(l),
            _ => None,
        })
    }

    fn compressor(&self) -> Option<&Compressor> {
        self.stages.iter().find_map(|s| match s {
            Stage::Compressor(c) => Some(c),
            _ => None,
        })
    }

    fn compressor_mut(&mut self) -> Option<&mut Compressor> {
        self.stages.iter_mut().find_map(|s| match s {
            Stage::Compressor(c) => Some(c),
            _ => None,
        })
    }

    /// Carries on what `old` counted (the limiter's and the compressor's
    /// statistics).
    fn continue_from(&mut self, old: &Self) {
        if let (Some(new), Some(old)) = (self.limiter_mut(), old.limiter()) {
            new.continue_activity(old);
        }
        if let (Some(new), Some(old)) = (self.compressor_mut(), old.compressor()) {
            new.continue_activity(old);
        }
    }

    fn process(&mut self, block: &mut [f32]) {
        if !self.is_active() {
            return;
        }
        for stage in &mut self.stages {
            stage.as_dyn().process(block);
        }
        guard(block);
    }

    fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.as_dyn().reset();
        }
    }

    fn latency(&self) -> usize {
        self.stages.iter().map(Stage::latency).sum()
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
    /// The equalizer of the track that plays, which replaces the one of the settings.
    track_equalizer: Option<EqualizerSettings>,
    impulses: ImpulseCache,
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
        let mut impulses = ImpulseCache::default();
        Self {
            rate,
            channels,
            settings: settings.clone(),
            track_equalizer: None,
            current: Stages::new(settings, rate, channels, &mut impulses),
            impulses,
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
        self.current.limiter().map(Limiter::activity)
    }

    /// Starts the limiter's statistics anew, for example for a new track.
    pub fn reset_limiter_activity(&mut self) {
        if let Some(limiter) = self.current.limiter_mut() {
            limiter.reset_activity();
        }
        if let Some(compressor) = self.current.compressor_mut() {
            compressor.reset_activity();
        }
    }

    /// What the compressor has done, if it runs.
    pub fn compressor_activity(&self) -> Option<CompressorActivity> {
        self.current.compressor().map(Compressor::activity)
    }

    /// Why convolution is switched on but does not run (no impulse response for the
    /// rate, a file that cannot be read), as the latest settings make it.
    pub fn convolution_problem(&self) -> Option<&str> {
        self.newest().problem.as_deref()
    }

    /// The equalizer of the track that plays now, which replaces the equalizer of
    /// the settings while `dsp.track_equalizer` is on; `None` for the settings' own.
    /// A change crossfades in like any other.
    pub fn set_track_equalizer(&mut self, equalizer: Option<&EqualizerSettings>) {
        if self.track_equalizer.as_ref() == equalizer {
            return;
        }
        self.track_equalizer = equalizer.cloned();
        self.rebuild();
    }

    /// The stages the settings alone make, without the track's equalizer.
    pub fn plan_without_track_equalizer(&mut self) -> DspPlan {
        if self.track_equalizer().is_none() {
            return self.plan();
        }
        Stages::new(&self.settings, self.rate, self.channels, &mut self.impulses).plan
    }

    /// The track's equalizer, if one replaces the settings' now.
    pub fn track_equalizer(&self) -> Option<&EqualizerSettings> {
        self.track_equalizer
            .as_ref()
            .filter(|_| self.settings.track_equalizer)
    }

    /// The settings with the track's equalizer in place of their own.
    fn effective(&self) -> DspSettings {
        let mut settings = self.settings.clone();
        if let Some(equalizer) = self.track_equalizer() {
            settings.equalizer = equalizer.clone();
        }
        settings
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
        self.rebuild();
    }

    /// Builds the stages the settings and the track's equalizer make, and fades to
    /// them unless they sound as the current ones do.
    fn rebuild(&mut self) {
        let settings = self.effective();
        let mut stages = Stages::new(&settings, self.rate, self.channels, &mut self.impulses);
        if self.track_equalizer().is_some()
            && let Some(equalizer) = &mut stages.plan.equalizer
        {
            equalizer.track = true;
        }
        if stages.same_as(self.newest()) {
            let (problem, plan) = (stages.problem, stages.plan);
            match &mut self.pending {
                Some(pending) => (pending.problem, pending.plan) = (problem, plan),
                None => (self.current.problem, self.current.plan) = (problem, plan),
            }
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
    fn the_plan_says_what_replaces_or_leaves_out_a_stage() {
        let mut settings = DspSettings::default();
        settings.convolution.enabled = true;
        // Nothing named: the stage has nothing to do and is not left out.
        let mut chain = DspChain::new(&settings, 48_000, 2);
        assert!(!chain.plan().convolution_left_out);
        assert!(chain.convolution_problem().is_some());
        // A file that is missing leaves it out.
        settings.convolution.files = vec![helikodu_model::settings::ImpulseFile {
            rate_hz: 48_000,
            path: "/nonexistent/helikodu/room.wav".into(),
        }];
        chain.set_settings(&settings);
        assert!(chain.plan().convolution_left_out);
        assert!(chain.plan().convolution.is_none());
        // The equalizer of a track is marked as such.
        let mut track = EqualizerSettings {
            enabled: true,
            ..EqualizerSettings::default()
        };
        track.mode = helikodu_model::settings::EqMode::Graphic10;
        track.graphic_10[3] = 4.0;
        chain.set_track_equalizer(Some(&track));
        let equalizer = chain.plan().equalizer.unwrap();
        assert!(equalizer.track);
        assert!(equalizer.to_string().starts_with("track EQ"), "{equalizer}");
        settings.track_equalizer = false;
        chain.set_settings(&settings);
        assert!(chain.plan().equalizer.is_none());
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
