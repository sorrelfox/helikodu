// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Errors of building the processing path.

use helikodu_model::audio::SampleRate;
use thiserror::Error;

/// A processing stage that could not be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PipelineError {
    /// The resampler refused the rates or sizes.
    #[error("cannot resample from {from} to {to}: {reason}")]
    Resampler {
        /// The source rate.
        from: SampleRate,
        /// The target rate.
        to: SampleRate,
        /// What the resampler reported.
        reason: String,
    },
    /// A stream without channels, or a block size of zero.
    #[error("invalid stream shape: {0}")]
    InvalidShape(&'static str),
}
