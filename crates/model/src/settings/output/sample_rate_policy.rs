// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sample rate the device is opened at.

use std::fmt;
use std::str::FromStr;

use crate::audio::SampleRate;
use crate::text_form::{ParseError, serde_as_text};

/// Which sample rate to open the device at.
///
/// The text form is `match`, `device-default` or `fixed:<hertz>`, for example
/// `fixed:48000`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SampleRatePolicy {
    /// The rate of the track, where the device and backend allow it.
    #[default]
    MatchTrack,
    /// The device's own preferred rate; the engine resamples.
    DeviceDefault,
    /// Always this rate; the engine resamples.
    Fixed(SampleRate),
}

impl fmt::Display for SampleRatePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MatchTrack => f.write_str("match"),
            Self::DeviceDefault => f.write_str("device-default"),
            Self::Fixed(rate) => write!(f, "fixed:{}", rate.hz()),
        }
    }
}

impl FromStr for SampleRatePolicy {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "match" => Ok(Self::MatchTrack),
            "device-default" => Ok(Self::DeviceDefault),
            _ => s
                .strip_prefix("fixed:")
                .and_then(|hz| hz.parse().ok())
                .and_then(SampleRate::new)
                .map(Self::Fixed)
                .ok_or_else(|| ParseError::new("sample-rate policy", s)),
        }
    }
}

serde_as_text!(SampleRatePolicy);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form() {
        let fixed = SampleRatePolicy::Fixed(SampleRate::HZ_44100);
        assert_eq!(fixed.to_string(), "fixed:44100");
        assert_eq!("fixed:44100".parse(), Ok(fixed));
        assert_eq!("match".parse(), Ok(SampleRatePolicy::MatchTrack));
        assert_eq!(
            "device-default".parse(),
            Ok(SampleRatePolicy::DeviceDefault)
        );
        for bad in ["fixed:", "fixed:0", "fixed:-1", "fixed:48k", "48000"] {
            assert!(bad.parse::<SampleRatePolicy>().is_err(), "{bad}");
        }
    }
}
