// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The processing path: everything between the decoder and the output backend.
//!
//! For every track a [`PathPlan`] decides, from the source format, the loudness
//! information, the settings and the device's format, which stages are needed, or
//! that none are and the track plays bit-perfect. The plan is data, so the
//! signal-path view can show it.
//!
//! The stages work on interleaved 32-bit float frames in canonical (WAVE) channel
//! order, with gains and filter state in 64-bit float:
//!
//! | Stage | Type | Where |
//! |---|---|---|
//! | Conversion to float | [`to_float`] | per source |
//! | Channel mapping, downmix, upmix | [`MixMatrix`] | per source |
//! | ReplayGain / R128 gain | [`GainStage`], [`resolve_gain`] | per source |
//! | Leveller (tracks without loudness values) | [`Leveller`] | per source |
//! | Resampling | [`ResampleStage`] | per source |
//! | Crossfade (equal power) | [`equal_power`] | where two sources are mixed |
//! | Software volume (`a = p³`) | [`SoftwareVolume`] | on the mixed stream |
//! | Anti-click fades | [`Fader`] | on the mixed stream |
//! | TPDF dither | [`TpdfDither`] | on the mixed stream |
//! | Conversion to the device format | [`Encoder`] | in the output backend |
//!
//! [`SourceChain`] and [`OutputChain`] build these from a plan. Every stage
//! allocates its buffers when it is built; processing a block never allocates, locks
//! or blocks.

mod chain;
mod channel_mix;
mod convert;
mod dither;
mod error;
pub mod leveller;
mod loudness;
mod path_plan;
mod ramp;
mod resample;
mod volume;

pub use self::chain::{OutputChain, SourceChain};
pub use self::channel_mix::{MixKind, MixMatrix, MixOptions};
pub use self::convert::{Encoder, PcmSample, to_float};
pub(crate) use self::convert::{f32_to_s32, s32_to_f32};
pub use self::dither::TpdfDither;
pub use self::error::PipelineError;
pub use self::leveller::{Leveller, LevellerPlan};
pub use self::loudness::{
    ClippingPrevention, GainDecision, GainSource, GainStage, LoudnessInfo, LoudnessMode,
    LoudnessSettings, R128_REFERENCE_LUFS, REFERENCE_LUFS, db_to_linear, linear_to_db,
    parse_gain_db, parse_peak, r128_to_replaygain_db, replaygain_db_to_r128, resolve_gain,
    resolve_track_gain,
};
pub use self::path_plan::{
    ChannelMixPlan, PathPlan, PathRequest, PlannedStage, ProcessingSettings, ResamplePlan,
    SourceFormat, VolumePlan, stream_config,
};
pub use self::ramp::{FadeDurations, FadeState, Fader, SmoothedGain, Transition, equal_power};
pub use self::resample::{ResampleMethod, ResampleStage, ResamplerQuality};
pub use self::volume::{SharedVolume, SoftwareVolume};
