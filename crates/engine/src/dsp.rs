// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSP chain: equalizer, crossfeed, channel mixer and limiter, in that fixed
//! order, on the mixed stream at the device's rate and layout.
//!
//! | Stage | Type | What it does |
//! |---|---|---|
//! | Equalizer | [`Equalizer`] | Preamp, then a graphic (10 or 31 bands, overlap-corrected) or parametric (up to 64 RBJ filters) equalizer |
//! | Crossfeed | [`Crossfeed`] | Bauer stereophonic-to-binaural crossfeed for headphones (stereo only) |
//! | Channel mixer | [`ChannelMixer`] | Mono, left/right swap and balance on the front pair |
//! | Limiter | [`Limiter`] | True-peak lookahead limiter (1 to 5 ms), the chain's safety net |
//!
//! [`DspChain`] builds the stages from the `[dsp]` settings for one rate and
//! channel count. A stage that is off, or whose values leave the samples unchanged
//! (a flat equalizer, centred stereo), is not built; when no stage is left the chain
//! does nothing at all, so a bit-perfect path stays bit-perfect. A change of the
//! settings builds a new set of stages, which takes over the old ones' filter state
//! and is crossfaded in over [`DspChain::CROSSFADE`], so that changes never click.
//!
//! Samples are interleaved 32-bit floats; filter state and arithmetic are 64-bit.
//! Every stage allocates when it is built and never while processing. The chain
//! ends with a guard that turns NaN, infinities and subnormal numbers into zero.
//!
//! [`DspPlan`] describes the active stages for the signal-path view. The AutoEQ
//! `ParametricEQ.txt` reader is [`autoeq`].

pub mod autoeq;
mod biquad;
mod chain;
mod channel_mixer;
mod crossfeed;
mod equalizer;
pub mod graphic;
mod limiter;
mod plan;

use std::fmt;

use helikodu_model::settings::{DspSettings, ResamplerQuality as QualitySetting};

use crate::pipeline::{ProcessingSettings, ResamplerQuality};

pub use self::biquad::{Cascade, Coefficients, MAX_RELATIVE_FREQUENCY};
pub use self::chain::DspChain;
pub use self::channel_mixer::ChannelMixer;
pub use self::crossfeed::Crossfeed;
pub use self::equalizer::Equalizer;
pub use self::limiter::{Limiter, LimiterActivity};
pub use self::plan::{CrossfeedPlan, DspPlan, EqualizerPlan, LimiterPlan, MixerPlan};

/// One stage of the DSP chain, built for a fixed rate and channel count.
pub trait DspStage: fmt::Debug + Send {
    /// The stage's name in the signal path.
    fn name(&self) -> &'static str;

    /// The delay the stage adds, in frames.
    fn latency(&self) -> usize {
        0
    }

    /// Processes interleaved frames in place. Never allocates.
    fn process(&mut self, block: &mut [f32]);

    /// Clears the stage's state, as at the start of a stream.
    fn reset(&mut self);
}

/// The processing settings with the parts the `[dsp]` table decides: the resampler
/// quality and whether surround downmixes include the LFE channel.
pub fn processing_settings(
    mut settings: ProcessingSettings,
    dsp: &DspSettings,
) -> ProcessingSettings {
    settings.resampler = match dsp.resampler {
        QualitySetting::Fast => ResamplerQuality::Low,
        QualitySetting::Medium => ResamplerQuality::Medium,
        QualitySetting::High => ResamplerQuality::High,
        QualitySetting::Maximum => ResamplerQuality::Maximum,
    };
    settings.mix.include_lfe = dsp.channel_mix.include_lfe;
    settings
}

/// Turns NaN, infinities and subnormal numbers into zero.
fn guard(block: &mut [f32]) {
    for sample in block {
        if !sample.is_finite() || sample.abs() < 1e-30 {
            *sample = 0.0;
        }
    }
}
