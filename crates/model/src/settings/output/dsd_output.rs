// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! How DSD files reach the device.

use serde::{Deserialize, Serialize};

/// How DSD files (DSF, DFF) reach the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DsdOutput {
    /// Converted to PCM, which every device plays.
    #[default]
    Pcm,
    /// DSD over PCM (DoP 1.1): the 1-bit stream packed into 24-bit PCM frames with
    /// marker bytes, for a DAC that recognises them. Used only when nothing on the way
    /// changes the samples (exclusive mode, fixed or hardware volume, no gain, DSP or
    /// resampling); otherwise the file is converted to PCM.
    Dop,
}
