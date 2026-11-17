// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What happens when the output device disappears.

use std::fmt;
use std::str::FromStr;

use crate::text_form::{ParseError, serde_as_text};

/// What happens when the chosen device disappears, or is missing at start-up.
///
/// The text form is `pause`, `fallback-default` or `stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DeviceLostPolicy {
    /// Pause playback until the device returns or another is chosen.
    #[default]
    Pause,
    /// Continue on the backend's default device.
    FallbackDefault,
    /// Stop playback.
    Stop,
}

impl DeviceLostPolicy {
    /// The text form.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::FallbackDefault => "fallback-default",
            Self::Stop => "stop",
        }
    }
}

impl fmt::Display for DeviceLostPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DeviceLostPolicy {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        [Self::Pause, Self::FallbackDefault, Self::Stop]
            .into_iter()
            .find(|policy| policy.as_str() == s)
            .ok_or_else(|| ParseError::new("device-lost policy", s))
    }
}

serde_as_text!(DeviceLostPolicy);
