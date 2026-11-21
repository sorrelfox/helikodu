// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSP chain: pitch shift, equalizer, convolution, crossfeed, channel mixer,
//! compressor and limiter, by default in that order (the settings may reorder
//! them), on the mixed stream at the device's rate and layout.
//!
//! | Stage | Type | What it does |
//! |---|---|---|
//! | Pitch shift | [`PitchShifter`] | Moves the pitch, keeping the tempo (WSOLA time stretch, then resampling) |
//! | Equalizer | [`Equalizer`] | Preamp, then a graphic (10 or 31 bands, overlap-corrected) or parametric (up to 64 RBJ filters) equalizer |
//! | Convolution | [`Convolver`] | Uniformly partitioned FFT convolution with an impulse response per sample rate |
//! | Crossfeed | [`Crossfeed`] | Bauer stereophonic-to-binaural crossfeed for headphones (stereo only) |
//! | Channel mixer | [`ChannelMixer`] | Mono, left/right swap and balance on the front pair |
//! | Compressor | [`Compressor`] | Feed-forward compressor with a soft knee and lookahead, and a night mode |
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
//! `ParametricEQ.txt` reader is [`autoeq`]; impulse responses are read by
//! [`impulse`].

pub mod autoeq;
mod biquad;
mod chain;
mod channel_mixer;
mod compressor;
mod convolver;
mod crossfeed;
mod equalizer;
pub mod impulse;
mod limiter;
mod pitch;
mod plan;
mod wsola;

use std::fmt;

use helikodu_model::settings::{DspSettings, ResamplerQuality as QualitySetting};

use crate::pipeline::{PlaybackSpeed, ProcessingSettings, ResamplerQuality};

pub use self::biquad::{Cascade, Coefficients, MAX_RELATIVE_FREQUENCY};
pub use self::chain::DspChain;
pub use self::channel_mixer::ChannelMixer;
pub use self::compressor::Compressor;
pub use self::convolver::{Convolver, partition_for};
pub use self::crossfeed::Crossfeed;
pub use self::equalizer::Equalizer;
pub use self::limiter::{Limiter, LimiterActivity};
pub use self::pitch::PitchShifter;
pub use self::plan::{
    CompressorPlan, ConvolutionPlan, CrossfeedPlan, DspPlan, EqualizerPlan, LimiterPlan, MixerPlan,
    PitchPlan, StagePlan,
};
pub use self::wsola::Wsola;
/// The graphic equalizer's bands, with overlap-corrected gains.
pub use helikodu_model::eq::graphic;

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
/// quality, whether surround downmixes include the LFE channel, and the playback
/// speed.
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
    settings.speed = PlaybackSpeed {
        rate: dsp.speed.effective_rate(),
        keep_pitch: dsp.speed.keep_pitch,
    };
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
