// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Playback speed: the `[dsp.speed]` table.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use super::check_number;
use crate::settings::SettingsError;

/// Playback speed: the `[dsp.speed]` table.
///
/// The speed changes how long a track takes to play; positions and lengths stay
/// those of the track. With `keep_pitch` the tempo changes and the pitch stays (a
/// time stretch); without it the pitch follows the speed, as on a tape.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeedSettings {
    /// Whether the speed applies.
    pub enabled: bool,
    /// The speed, as a factor of the normal one (0.5 to 2).
    pub rate: f64,
    /// Whether the pitch stays where it is (`true`) or follows the speed (`false`).
    pub keep_pitch: bool,
}

impl Default for SpeedSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            rate: 1.0,
            keep_pitch: true,
        }
    }
}

impl SpeedSettings {
    /// The range of the speed.
    pub const RATE_RANGE: RangeInclusive<f64> = 0.5..=2.0;

    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        check_number("dsp.speed.rate", self.rate, Self::RATE_RANGE)
    }

    /// The speed playback runs at: the rate when the table is on, otherwise 1.
    pub fn effective_rate(&self) -> f64 {
        if self.enabled && self.rate.is_finite() {
            self.rate
                .clamp(*Self::RATE_RANGE.start(), *Self::RATE_RANGE.end())
        } else {
            1.0
        }
    }

    /// Whether playback runs at another speed than the normal one.
    pub fn is_active(&self) -> bool {
        self.effective_rate() != 1.0
    }

    /// The table for playing at `rate`, keeping the pitch or not: off at 1.
    pub fn at(rate: f64, keep_pitch: bool) -> Self {
        Self {
            enabled: rate != 1.0,
            rate,
            keep_pitch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_speed_applies_only_when_on_and_in_range() {
        assert_eq!(SpeedSettings::default().effective_rate(), 1.0);
        assert!(!SpeedSettings::default().is_active());
        let fast = SpeedSettings::at(1.25, false);
        assert!(fast.enabled && fast.is_active());
        assert_eq!(fast.effective_rate(), 1.25);
        assert!(!SpeedSettings::at(1.0, true).enabled);
        assert!(SpeedSettings::at(3.0, true).validate().is_err());
        assert!(SpeedSettings::at(f64::NAN, true).validate().is_err());
        assert_eq!(SpeedSettings::at(3.0, true).effective_rate(), 2.0);
    }
}
