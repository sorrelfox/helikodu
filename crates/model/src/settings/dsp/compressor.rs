// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The dynamics compressor: the `[dsp.compressor]` table.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use super::check_number;
use crate::settings::SettingsError;

/// The values a compressor runs with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompressorValues {
    /// The level above which the gain goes down, in dBFS.
    pub threshold_db: f64,
    /// How many dB over the threshold go in for every dB that comes out.
    pub ratio: f64,
    /// How fast the gain goes down, in milliseconds.
    pub attack_ms: f64,
    /// How fast the gain comes back, in milliseconds.
    pub release_ms: f64,
    /// The width of the soft knee around the threshold, in dB.
    pub knee_db: f64,
    /// The gain added after the compression, in dB.
    pub makeup_db: f64,
}

/// Ready-made compressor settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompressorPreset {
    /// Night mode: strong compression that lifts quiet passages and holds loud ones
    /// down, for listening at a low volume.
    #[default]
    Night,
    /// Light compression that evens out the level a little.
    Gentle,
    /// The values of the table.
    Custom,
}

impl CompressorPreset {
    /// The preset's values; `None` for [`Custom`](Self::Custom).
    pub const fn values(self) -> Option<CompressorValues> {
        match self {
            Self::Night => Some(CompressorValues {
                threshold_db: -30.0,
                ratio: 4.0,
                attack_ms: 5.0,
                release_ms: 250.0,
                knee_db: 10.0,
                makeup_db: 12.0,
            }),
            Self::Gentle => Some(CompressorValues {
                threshold_db: -18.0,
                ratio: 2.0,
                attack_ms: 10.0,
                release_ms: 150.0,
                knee_db: 6.0,
                makeup_db: 3.0,
            }),
            Self::Custom => None,
        }
    }
}

/// The compressor: the `[dsp.compressor]` table. It comes before the limiter in the
/// default order of the chain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompressorSettings {
    /// Whether the stage runs.
    pub enabled: bool,
    /// The preset; the values below are used with `custom` only.
    pub preset: CompressorPreset,
    /// The threshold, in dBFS (-60 to 0).
    pub threshold_db: f64,
    /// The ratio (1 to 20).
    pub ratio: f64,
    /// The attack, in milliseconds (0.1 to 200).
    pub attack_ms: f64,
    /// The release, in milliseconds (10 to 3000).
    pub release_ms: f64,
    /// The soft knee's width, in dB (0 to 24).
    pub knee_db: f64,
    /// The makeup gain, in dB (0 to 24).
    pub makeup_db: f64,
    /// How far ahead the compressor looks, in milliseconds (1 to 5, in steps of
    /// 0.5), as the limiter does. Used with every preset.
    pub lookahead_ms: f64,
}

impl CompressorSettings {
    /// The range of the threshold, in dBFS.
    pub const THRESHOLD_RANGE: RangeInclusive<f64> = -60.0..=0.0;
    /// The range of the ratio.
    pub const RATIO_RANGE: RangeInclusive<f64> = 1.0..=20.0;
    /// The range of the attack, in milliseconds.
    pub const ATTACK_RANGE: RangeInclusive<f64> = 0.1..=200.0;
    /// The range of the release, in milliseconds.
    pub const RELEASE_RANGE: RangeInclusive<f64> = 10.0..=3000.0;
    /// The range of the knee, in dB.
    pub const KNEE_RANGE: RangeInclusive<f64> = 0.0..=24.0;
    /// The range of the makeup gain, in dB.
    pub const MAKEUP_RANGE: RangeInclusive<f64> = 0.0..=24.0;
    /// The range of the lookahead, in milliseconds.
    pub const LOOKAHEAD_RANGE: RangeInclusive<f64> = 1.0..=5.0;

    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        let key = |name: &str| format!("dsp.compressor.{name}");
        check_number(
            &key("threshold_db"),
            self.threshold_db,
            Self::THRESHOLD_RANGE,
        )?;
        check_number(&key("ratio"), self.ratio, Self::RATIO_RANGE)?;
        check_number(&key("attack_ms"), self.attack_ms, Self::ATTACK_RANGE)?;
        check_number(&key("release_ms"), self.release_ms, Self::RELEASE_RANGE)?;
        check_number(&key("knee_db"), self.knee_db, Self::KNEE_RANGE)?;
        check_number(&key("makeup_db"), self.makeup_db, Self::MAKEUP_RANGE)?;
        check_number(
            &key("lookahead_ms"),
            self.lookahead_ms,
            Self::LOOKAHEAD_RANGE,
        )?;
        if (self.lookahead_ms * 2.0).fract() != 0.0 {
            return Err(SettingsError {
                key: key("lookahead_ms"),
                problem: format!("is {}, but must be a multiple of 0.5", self.lookahead_ms),
            });
        }
        Ok(())
    }

    /// The values in use: the preset's, or the table's for `custom`.
    pub fn effective(&self) -> CompressorValues {
        self.preset.values().unwrap_or(CompressorValues {
            threshold_db: self.threshold_db,
            ratio: self.ratio,
            attack_ms: self.attack_ms,
            release_ms: self.release_ms,
            knee_db: self.knee_db,
            makeup_db: self.makeup_db,
        })
    }
}

impl Default for CompressorSettings {
    fn default() -> Self {
        let night = CompressorValues {
            threshold_db: -30.0,
            ratio: 4.0,
            attack_ms: 5.0,
            release_ms: 250.0,
            knee_db: 10.0,
            makeup_db: 12.0,
        };
        Self {
            enabled: false,
            preset: CompressorPreset::Night,
            threshold_db: night.threshold_db,
            ratio: night.ratio,
            attack_ms: night.attack_ms,
            release_ms: night.release_ms,
            knee_db: night.knee_db,
            makeup_db: night.makeup_db,
            lookahead_ms: 2.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_custom_values_start_as_night_mode() {
        let settings = CompressorSettings::default();
        assert_eq!(settings.validate(), Ok(()));
        assert_eq!(Some(settings.effective()), CompressorPreset::Night.values());
        let custom = CompressorSettings {
            preset: CompressorPreset::Custom,
            ..settings
        };
        assert_eq!(custom.effective(), settings.effective());
        let gentle = CompressorSettings {
            preset: CompressorPreset::Gentle,
            ratio: 9.0,
            ..settings
        };
        assert_eq!(gentle.effective().ratio, 2.0);
    }

    #[test]
    fn values_are_checked() {
        let bad = CompressorSettings {
            ratio: 0.5,
            ..CompressorSettings::default()
        };
        assert_eq!(bad.validate().unwrap_err().key, "dsp.compressor.ratio");
        let bad = CompressorSettings {
            lookahead_ms: 1.25,
            ..CompressorSettings::default()
        };
        assert!(bad.validate().is_err());
    }
}
