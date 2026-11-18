// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The stages of a [`PathPlan`], built and wired together.
//!
//! [`SourceChain`] runs per source, ahead of playback: channel mixing, loudness gain
//! and resampling, from the decoder's `f32` frames to frames in the device's rate and
//! layout. [`OutputChain`] runs on the mixed stream just before it reaches the
//! device: software volume, anti-click fades and dither. Both allocate only when
//! built.

use super::{
    FadeDurations, FadeState, Fader, GainStage, MixMatrix, PathPlan, PipelineError, ResampleStage,
    SoftwareVolume, TpdfDither, Transition,
};

/// Channel mixing, gain and resampling for one source.
#[derive(Debug)]
pub struct SourceChain {
    rate: u32,
    input_channels: usize,
    output_channels: usize,
    max_block: usize,
    mix: Option<MixMatrix>,
    gain: Option<GainStage>,
    resample: Option<ResampleStage>,
    /// Mixed frames on their way to the resampler.
    scratch: Box<[f32]>,
}

impl SourceChain {
    /// Builds the source stages of `plan` for blocks of up to `max_block` frames.
    pub fn new(plan: &PathPlan, max_block: usize) -> Result<Self, PipelineError> {
        let input_channels = usize::from(plan.source.spec.channel_count());
        let output_channels = usize::from(plan.device.channel_count());
        if input_channels == 0 || output_channels == 0 {
            return Err(PipelineError::InvalidShape("no channels"));
        }
        if max_block == 0 {
            return Err(PipelineError::InvalidShape("block size of zero"));
        }
        let rate = plan.source.spec.rate.hz();
        let resample = plan
            .resample
            .map(|r| ResampleStage::new(r.from, r.to, output_channels, r.quality))
            .transpose()?;
        let scratch_len = if resample.is_some() {
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
        self.resample
            .as_ref()
            .map_or(self.max_block, |r| r.max_output_frames(self.max_block))
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

    /// Processes the interleaved frames of `input` into `output`. Returns the number of
    /// input frames taken and output frames written. Frames not taken (beyond
    /// [`max_block`](Self::max_block), or for lack of output room) must be passed again.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> (usize, usize) {
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

    /// Ends the source: writes what is still inside the stages into `output` and
    /// returns the number of frames written. Call until it returns 0.
    pub fn finish(&mut self, output: &mut [f32]) -> usize {
        self.resample.as_mut().map_or(0, |r| r.finish(output))
    }

    /// Clears the stages' state, for a seek or a new stream.
    pub fn reset(&mut self) {
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
