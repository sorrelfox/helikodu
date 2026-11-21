// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The stages of a [`PathPlan`], built and wired together.
//!
//! [`SourceChain`] runs per source, ahead of playback: channel mixing, loudness gain
//! (with the leveller for tracks without loudness values), the playback speed and
//! resampling, from the decoder's `f32` frames to frames in the device's rate and
//! layout. [`OutputChain`] runs on the mixed stream just before it reaches the
//! device: software volume, anti-click fades and dither. Both allocate only when
//! built.

use std::sync::Arc;

use helikodu_model::analysis::TrackAnalysis;

use super::{
    FadeDurations, FadeState, Fader, GainStage, Leveller, LevellerPlan, MixMatrix, PathPlan,
    PipelineError, ResampleStage, SoftwareVolume, TempoStage, TpdfDither, Transition,
};

/// Channel mixing, gain, playback speed and resampling for one source.
#[derive(Debug)]
pub struct SourceChain {
    rate: u32,
    input_channels: usize,
    output_channels: usize,
    max_block: usize,
    mix: Option<MixMatrix>,
    gain: Option<GainStage>,
    leveller: Option<Leveller>,
    /// The track analysis the leveller reads its envelope from.
    analysis: Option<Arc<TrackAnalysis>>,
    /// The source frame of the next input frame, as far as the chain knows.
    position: u64,
    /// The tempo change that keeps the pitch, at another speed.
    tempo: Option<TempoStage>,
    resample: Option<ResampleStage>,
    /// Mixed frames on their way to the tempo stage or the resampler.
    scratch: Box<[f32]>,
}

impl SourceChain {
    /// Builds the source stages of `plan` for blocks of up to `max_block` frames.
    pub fn new(plan: &PathPlan, max_block: usize) -> Result<Self, PipelineError> {
        Self::with_analysis(plan, max_block, None)
    }

    /// [`new`](Self::new) for a track with the analysis `analysis`, whose loudness
    /// envelope the leveller reads ahead in.
    pub fn with_analysis(
        plan: &PathPlan,
        max_block: usize,
        analysis: Option<Arc<TrackAnalysis>>,
    ) -> Result<Self, PipelineError> {
        let input_channels = usize::from(plan.source.spec.channel_count());
        let output_channels = usize::from(plan.device.channel_count());
        if input_channels == 0 || output_channels == 0 {
            return Err(PipelineError::InvalidShape("no channels"));
        }
        if max_block == 0 {
            return Err(PipelineError::InvalidShape("block size of zero"));
        }
        let rate = plan.source.spec.rate.hz();
        // At another speed the pitch either stays (a time stretch, then the usual
        // resampling) or follows (the source resampled as if its rate were the
        // played one).
        let tempo = plan
            .speed
            .filter(|speed| speed.keep_pitch)
            .map(|speed| TempoStage::new(&speed, output_channels, max_block));
        let resample = match plan.speed.filter(|speed| !speed.keep_pitch) {
            Some(tape) => (tape.played != plan.device.rate)
                .then(|| {
                    ResampleStage::new(tape.played, plan.device.rate, output_channels, tape.quality)
                })
                .transpose()?,
            None => plan
                .resample
                .map(|r| ResampleStage::new(r.from, r.to, output_channels, r.quality))
                .transpose()?,
        };
        let scratch_len = if resample.is_some() || tempo.is_some() {
            max_block * output_channels
        } else {
            0
        };
        Ok(Self {
            rate,
            input_channels,
            output_channels,
            max_block,
            mix: plan.channel_mix.map(|mix| mix.matrix()),
            gain: plan
                .gain
                .map(|gain| GainStage::new(rate, output_channels, gain.linear())),
            leveller: plan
                .leveller
                .map(|l| Leveller::new(l, rate, output_channels, analysis.clone())),
            analysis,
            position: 0,
            tempo,
            resample,
            scratch: vec![0.0; scratch_len].into_boxed_slice(),
        })
    }

    /// Channels per input frame.
    pub fn input_channels(&self) -> usize {
        self.input_channels
    }

    /// Channels per output frame.
    pub fn output_channels(&self) -> usize {
        self.output_channels
    }

    /// The most input frames one [`process`](Self::process) call takes.
    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// The output space, in frames, that lets [`process`](Self::process) take a full
    /// block, and lets [`finish`](Self::finish) make progress.
    pub fn max_output_frames(&self) -> usize {
        let frames = self
            .tempo
            .as_ref()
            .map_or(self.max_block, TempoStage::capacity);
        self.resample
            .as_ref()
            .map_or(frames, |r| r.max_output_frames(frames))
    }

    /// The gain stage, to change the gain while playing.
    pub fn gain_mut(&mut self) -> Option<&mut GainStage> {
        self.gain.as_mut()
    }

    /// Moves the loudness gain to the linear factor `linear`, ramping over
    /// [`GainStage::RAMP`] from the gain applied so far. A chain built without a gain
    /// stage gets one, starting at unity, so the change is as smooth as any other.
    /// The resampler's state is kept: the audio goes on without a seam.
    pub fn set_gain(&mut self, linear: f64) {
        match &mut self.gain {
            Some(gain) => gain.set_gain(linear),
            None if linear == 1.0 => {}
            None => {
                let mut gain = GainStage::new(self.rate, self.output_channels, 1.0);
                gain.set_gain(linear);
                self.gain = Some(gain);
            }
        }
    }

    /// The leveller, when the chain has one.
    pub fn leveller(&self) -> Option<&Leveller> {
        self.leveller.as_ref()
    }

    /// Starts, changes or ends the leveller. A new one starts at 0 dB; one that
    /// ends goes back to 0 dB over a short ramp first.
    pub fn set_leveller(&mut self, plan: Option<LevellerPlan>) {
        match (plan, &mut self.leveller) {
            (Some(plan), Some(leveller)) => leveller.set_plan(plan),
            (Some(plan), None) => {
                let mut leveller =
                    Leveller::new(plan, self.rate, self.output_channels, self.analysis.clone());
                leveller.locate(self.position);
                self.leveller = Some(leveller);
            }
            (None, Some(leveller)) => leveller.release(),
            (None, None) => {}
        }
    }

    /// The next input frame is the source frame `frame`: after a seek, or a jump
    /// over silence. Only the leveller cares.
    pub fn locate(&mut self, frame: u64) {
        self.position = frame;
        if let Some(leveller) = &mut self.leveller {
            leveller.locate(frame);
        }
    }

    /// Processes the interleaved frames of `input` into `output`. Returns the number of
    /// input frames taken and output frames written. Frames not taken (beyond
    /// [`max_block`](Self::max_block), or for lack of output room) must be passed again.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> (usize, usize) {
        if self.tempo.is_some() {
            return self.process_stretched(input, output);
        }
        let out_room = output.len() / self.output_channels;
        let mut frames = (input.len() / self.input_channels).min(self.max_block);
        frames = match &self.resample {
            Some(resample) => frames.min(resample.accepts(out_room)),
            None => frames.min(out_room),
        };
        let input = &input[..frames * self.input_channels];
        let mixed = match self.resample {
            Some(_) => &mut self.scratch[..frames * self.output_channels],
            None => &mut output[..frames * self.output_channels],
        };
        match &self.mix {
            Some(mix) => {
                mix.apply(input, mixed);
            }
            None => mixed.copy_from_slice(input),
        }
        if let Some(gain) = &mut self.gain {
            gain.apply(mixed);
        }
        if let Some(leveller) = &mut self.leveller {
            leveller.apply(mixed);
        }
        self.position += frames as u64;
        match &mut self.resample {
            Some(resample) => {
                let (taken, written) =
                    resample.process(&self.scratch[..frames * self.output_channels], output);
                debug_assert_eq!(taken, frames);
                (frames, written)
            }
            None => (frames, frames),
        }
    }

    /// Mixing, gain and the leveller on `frames` frames of `input`, into `scratch`.
    fn prepare(&mut self, input: &[f32], frames: usize) {
        let input = &input[..frames * self.input_channels];
        let mixed = &mut self.scratch[..frames * self.output_channels];
        match &self.mix {
            Some(mix) => {
                mix.apply(input, mixed);
            }
            None => mixed.copy_from_slice(input),
        }
        if let Some(gain) = &mut self.gain {
            gain.apply(mixed);
        }
        if let Some(leveller) = &mut self.leveller {
            leveller.apply(mixed);
        }
        self.position += frames as u64;
    }

    /// [`process`](Self::process) with the tempo stage: what it holds goes on
    /// first; a new block is taken only once it has passed everything on.
    fn process_stretched(&mut self, input: &[f32], output: &mut [f32]) -> (usize, usize) {
        let mut written = self.pass_stretched(output);
        if self.tempo.as_ref().is_none_or(|t| t.pending() > 0) {
            return (0, written);
        }
        let frames = (input.len() / self.input_channels).min(self.max_block);
        if frames == 0 {
            return (0, written);
        }
        self.prepare(input, frames);
        if let Some(tempo) = &mut self.tempo {
            tempo.push(&self.scratch[..frames * self.output_channels]);
        }
        written += self.pass_stretched(&mut output[written * self.output_channels..]);
        (frames, written)
    }

    /// Passes what the tempo stage holds on to the resampler, or to `output`.
    /// Returns the frames written to `output`.
    fn pass_stretched(&mut self, output: &mut [f32]) -> usize {
        let Some(tempo) = &mut self.tempo else {
            return 0;
        };
        let channels = self.output_channels;
        let ready = tempo.ready();
        let (taken, written) = match &mut self.resample {
            Some(resample) => resample.process(ready, output),
            None => {
                let n = (ready.len() / channels).min(output.len() / channels);
                output[..n * channels].copy_from_slice(&ready[..n * channels]);
                (n, n)
            }
        };
        tempo.consume(taken);
        written
    }

    /// Ends the source: writes what is still inside the stages into `output` and
    /// returns the number of frames written. Call until it returns 0.
    pub fn finish(&mut self, output: &mut [f32]) -> usize {
        if let Some(tempo) = &mut self.tempo
            && !tempo.is_done()
        {
            // What waits goes on, then the stretcher's tail.
            loop {
                let pending = self.tempo.as_ref().map_or(0, TempoStage::pending);
                if pending == 0 {
                    break;
                }
                let written = self.pass_stretched(output);
                let left = self.tempo.as_ref().map_or(0, TempoStage::pending);
                if written > 0 || left == pending {
                    return written;
                }
            }
            if let Some(tempo) = &mut self.tempo {
                tempo.finish();
            }
            loop {
                let pending = self.tempo.as_ref().map_or(0, TempoStage::pending);
                let written = self.pass_stretched(output);
                let done = self.tempo.as_ref().is_none_or(TempoStage::is_done);
                let left = self.tempo.as_ref().map_or(0, TempoStage::pending);
                if written > 0 {
                    return written;
                }
                if done || left == pending {
                    break;
                }
            }
        }
        self.resample.as_mut().map_or(0, |r| r.finish(output))
    }

    /// Clears the stages' state, for a seek or a new stream.
    pub fn reset(&mut self) {
        if let Some(tempo) = &mut self.tempo {
            tempo.reset();
        }
        if let Some(resample) = &mut self.resample {
            resample.reset();
        }
    }
}

/// Software volume, anti-click fades and dither, just before the device.
#[derive(Debug, Clone)]
pub struct OutputChain {
    channels: usize,
    volume: Option<SoftwareVolume>,
    fader: Fader,
    fades: FadeDurations,
    dither: Option<TpdfDither>,
    always_dither: bool,
}

impl OutputChain {
    /// Builds the output stages of `plan`. The chain starts silent; call
    /// [`fade_in`](Self::fade_in) with [`Transition::Start`] when playback begins.
    pub fn new(plan: &PathPlan, fades: FadeDurations) -> Self {
        let channels = usize::from(plan.device.channel_count());
        let rate = plan.device.rate.hz();
        Self {
            channels,
            volume: plan
                .software_volume()
                .map(|volume| SoftwareVolume::new(rate, channels, volume)),
            fader: Fader::new(rate, false),
            fades,
            dither: plan
                .dither
                .and_then(|bits| TpdfDither::new(bits, TpdfDither::DEFAULT_SEED)),
            always_dither: plan.always_dither,
        }
    }

    /// Replaces the volume stage, for example with one that follows a
    /// [`SharedVolume`](super::SharedVolume).
    pub fn set_volume_stage(&mut self, volume: Option<SoftwareVolume>) {
        self.volume = volume;
    }

    /// The software volume stage, if the engine applies the volume.
    pub fn volume_mut(&mut self) -> Option<&mut SoftwareVolume> {
        self.volume.as_mut()
    }

    /// The fader.
    pub fn fader(&self) -> &Fader {
        &self.fader
    }

    /// Starts the fade in of `transition` (start, resume, or the end of a seek or
    /// device switch).
    pub fn fade_in(&mut self, transition: Transition) {
        self.fader.fade_in(self.fades.of(transition));
    }

    /// Starts the fade out of `transition` (pause, stop, or the start of a seek or
    /// device switch). Playback may stop once [`Fader::is_silent`] holds.
    pub fn fade_out(&mut self, transition: Transition) {
        self.fader.fade_out(self.fades.of(transition));
    }

    /// Jumps the fader open or shut, without a fade.
    pub fn set_open(&mut self, open: bool) {
        self.fader.set_open(open);
    }

    /// Applies volume, fade and dither to the interleaved frames of `block`.
    ///
    /// Dither runs while anything changes the samples. Silence after a fade out stays
    /// digital silence.
    pub fn apply(&mut self, block: &mut [f32]) {
        let frames = block.len() / self.channels.max(1);
        // The frames that are not silenced by the fader.
        let audible = match self.fader.state() {
            FadeState::Silent => 0,
            FadeState::FadingOut => self.fader.remaining_frames().min(frames),
            FadeState::FadingIn | FadeState::Open => frames,
        };
        if let Some(volume) = &mut self.volume {
            volume.refresh();
        }
        let changing = self.always_dither
            || !self.fader.is_open()
            || self.volume.as_ref().is_some_and(|v| !v.is_transparent());
        let dithered = if changing { audible } else { 0 };
        if let Some(volume) = &mut self.volume {
            volume.apply(block);
        }
        self.fader.apply(block, self.channels);
        if let Some(dither) = &mut self.dither {
            dither.apply(&mut block[..dithered * self.channels]);
        }
    }
}
