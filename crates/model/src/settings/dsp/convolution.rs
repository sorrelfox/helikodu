// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Convolution with an impulse response: the `[dsp.convolution]` table.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::check_number;
use crate::settings::SettingsError;

/// An impulse response for one sample rate: a `[[dsp.convolution.files]]` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImpulseFile {
    /// The sample rate the file is for, in Hz.
    pub rate_hz: u32,
    /// The WAV file.
    pub path: PathBuf,
}

/// Convolution: the `[dsp.convolution]` table.
///
/// The impulse response for the device's rate is the entry of `files` with that
/// rate, or else the WAV file in `folder` recorded at that rate. When there is
/// none, the file with the nearest rate is resampled to the device's rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConvolutionSettings {
    /// Whether the stage runs.
    pub enabled: bool,
    /// A folder of WAV impulse responses, one per sample rate.
    pub folder: Option<PathBuf>,
    /// Impulse responses by sample rate; they come before the folder's.
    pub files: Vec<ImpulseFile>,
    /// Scales the impulse response so that it boosts no frequency: the peak of its
    /// frequency response becomes 0 dB.
    pub normalize: bool,
    /// The gain of the processed signal, in dB (-24 to +12).
    pub gain_db: f64,
    /// How much of the output is the processed signal, in percent (0 to 100); the
    /// rest is the unprocessed signal, delayed to match.
    pub wet_percent: f64,
}

impl ConvolutionSettings {
    /// The range of the gain, in dB.
    pub const GAIN_RANGE: RangeInclusive<f64> = -24.0..=12.0;
    /// The range of the wet share, in percent.
    pub const WET_RANGE: RangeInclusive<f64> = 0.0..=100.0;
    /// The range of the sample rates of `files`, in Hz.
    pub const RATE_RANGE: RangeInclusive<u32> = 8_000..=768_000;

    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        check_number("dsp.convolution.gain_db", self.gain_db, Self::GAIN_RANGE)?;
        check_number(
            "dsp.convolution.wet_percent",
            self.wet_percent,
            Self::WET_RANGE,
        )?;
        for (i, file) in self.files.iter().enumerate() {
            let key = format!("dsp.convolution.files[{i}]");
            crate::settings::check_range(
                &format!("{key}.rate_hz"),
                file.rate_hz,
                Self::RATE_RANGE,
            )?;
            if file.path.as_os_str().is_empty() {
                return Err(SettingsError {
                    key: format!("{key}.path"),
                    problem: "is empty".into(),
                });
            }
            if self.files[..i].iter().any(|f| f.rate_hz == file.rate_hz) {
                return Err(SettingsError {
                    key: format!("{key}.rate_hz"),
                    problem: format!("is {}, which an earlier entry has too", file.rate_hz),
                });
            }
        }
        Ok(())
    }

    /// The file listed for `rate` Hz, if there is one.
    pub fn file_for(&self, rate: u32) -> Option<&Path> {
        self.files
            .iter()
            .find(|f| f.rate_hz == rate)
            .map(|f| f.path.as_path())
    }

    /// Whether an impulse response is named at all.
    pub fn has_source(&self) -> bool {
        self.folder.is_some() || !self.files.is_empty()
    }
}

impl Default for ConvolutionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            folder: None,
            files: Vec::new(),
            normalize: true,
            gain_db: 0.0,
            wet_percent: 100.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_found_by_rate_and_checked() {
        let mut settings = ConvolutionSettings::default();
        assert!(!settings.has_source());
        settings.files = vec![
            ImpulseFile {
                rate_hz: 44_100,
                path: "/ir/44.wav".into(),
            },
            ImpulseFile {
                rate_hz: 48_000,
                path: "/ir/48.wav".into(),
            },
        ];
        assert_eq!(settings.validate(), Ok(()));
        assert_eq!(settings.file_for(48_000), Some(Path::new("/ir/48.wav")));
        assert_eq!(settings.file_for(96_000), None);
        settings.files[1].rate_hz = 44_100;
        assert_eq!(
            settings.validate().unwrap_err().key,
            "dsp.convolution.files[1].rate_hz"
        );
        settings.files.pop();
        settings.wet_percent = 120.0;
        assert!(settings.validate().is_err());
    }
}
