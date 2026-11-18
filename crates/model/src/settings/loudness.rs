// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The loudness settings: the `[loudness]` table.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use super::{SettingsError, check_range};

/// Loudness normalization by ReplayGain and Opus R128 values: the `[loudness]` table.
///
/// ```toml
/// [loudness]
/// mode = "auto"                # off | track | album | auto
/// preamp_db = 0.0              # files with loudness values
/// untagged_preamp_db = -6.0    # files without them
/// clipping = "lower-gain"      # lower-gain | limiter
/// write_measured_to_tags = false  # measured values also go into the files' tags
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoudnessSettings {
    /// Which gain is applied.
    pub mode: LoudnessMode,
    /// Added to the gain of files with loudness values, in dB (−12 to +12, in steps of
    /// 0.5).
    pub preamp_db: f64,
    /// The gain of files without loudness values, in dB (−12 to +12, in steps of 0.5).
    pub untagged_preamp_db: f64,
    /// How clipping is prevented when a gain raises the level.
    pub clipping: ClippingPrevention,
    /// Whether the loudness scanner also writes what it measures into the files'
    /// tags (ReplayGain 2.0). Off: the values are kept in the library only.
    pub write_measured_to_tags: bool,
}

impl LoudnessSettings {
    /// The range of both preamps, in dB.
    pub const PREAMP_RANGE: RangeInclusive<f64> = -12.0..=12.0;

    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        for (key, value) in [
            ("loudness.preamp_db", self.preamp_db),
            ("loudness.untagged_preamp_db", self.untagged_preamp_db),
        ] {
            if value.is_nan() {
                return Err(SettingsError {
                    key: key.into(),
                    problem: "is not a number".into(),
                });
            }
            check_range(key, value, Self::PREAMP_RANGE)?;
            if (value * 2.0).fract() != 0.0 {
                return Err(SettingsError {
                    key: key.into(),
                    problem: format!("is {value}, but must be a multiple of 0.5"),
                });
            }
        }
        Ok(())
    }
}

impl Default for LoudnessSettings {
    fn default() -> Self {
        Self {
            mode: LoudnessMode::Auto,
            preamp_db: 0.0,
            untagged_preamp_db: -6.0,
            clipping: ClippingPrevention::LowerGain,
            write_measured_to_tags: false,
        }
    }
}

/// Which loudness gain is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoudnessMode {
    /// None.
    Off,
    /// The track gain.
    Track,
    /// The album gain.
    Album,
    /// The album gain while an album plays in order, the track gain otherwise.
    #[default]
    Auto,
}

/// How clipping is prevented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClippingPrevention {
    /// The gain is lowered so that the peak stays at full scale.
    #[default]
    LowerGain,
    /// A limiter catches the peaks.
    Limiter,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_design_defaults() {
        let loudness = LoudnessSettings::default();
        assert_eq!(loudness.mode, LoudnessMode::Auto);
        assert_eq!(loudness.untagged_preamp_db, -6.0);
        assert!(!loudness.write_measured_to_tags);
        assert_eq!(loudness.validate(), Ok(()));
    }

    #[test]
    fn preamps_are_checked() {
        for value in [-12.5, 12.5, 1.25, f64::NAN] {
            let loudness = LoudnessSettings {
                preamp_db: value,
                ..LoudnessSettings::default()
            };
            assert!(loudness.validate().is_err(), "{value}");
        }
        let loudness = LoudnessSettings {
            preamp_db: -11.5,
            untagged_preamp_db: 12.0,
            ..LoudnessSettings::default()
        };
        assert_eq!(loudness.validate(), Ok(()));
    }
}
