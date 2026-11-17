// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings schema, one file per group, as stored in `settings.toml`.
//!
//! Every field has a default, so a missing file, table or key reads as the default.
//! Keys this version does not know are ignored.

mod output;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use output::{
    DeviceLostPolicy, DeviceProfile, OutputFormat, OutputSettings, SampleRatePolicy, VolumeControl,
};

/// All settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The `[output]` table: the audio interface.
    pub output: OutputSettings,
}

impl Settings {
    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        self.output.validate()
    }
}

/// A setting with a value outside its range.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{key} is {value}, but must be from {min} to {max}")]
pub struct SettingsError {
    /// The setting's key in `settings.toml`, for example `output.buffer_ms`.
    pub key: String,
    /// The value found.
    pub value: u64,
    /// The smallest value allowed.
    pub min: u64,
    /// The largest value allowed.
    pub max: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        let settings: Settings = toml::from_str("").unwrap();
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.validate(), Ok(()));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let settings: Settings =
            toml::from_str("[output]\nfuture_key = 1\n[later]\nx = 2\n").unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn defaults_round_trip() {
        let text = toml::to_string(&Settings::default()).unwrap();
        assert!(text.starts_with("[output]\n"), "{text}");
        assert_eq!(
            toml::from_str::<Settings>(&text).unwrap(),
            Settings::default()
        );
    }
}
