// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pitch shift: the `[dsp.pitch]` table.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use super::check_number;
use crate::settings::SettingsError;

/// Pitch shift that keeps the tempo: the `[dsp.pitch]` table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PitchSettings {
    /// Whether the stage runs.
    pub enabled: bool,
    /// The shift, in semitones (-12 to +12; fractions are cents).
    pub semitones: f64,
}

impl PitchSettings {
    /// The range of the shift, in semitones.
    pub const SEMITONE_RANGE: RangeInclusive<f64> = -12.0..=12.0;

    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        check_number("dsp.pitch.semitones", self.semitones, Self::SEMITONE_RANGE)
    }

    /// The frequency ratio of the shift.
    pub fn ratio(&self) -> f64 {
        (self.semitones / 12.0).exp2()
    }
}
