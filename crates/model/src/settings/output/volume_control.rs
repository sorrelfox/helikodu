// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where the volume is applied.

use std::fmt;
use std::str::FromStr;

use crate::text_form::{ParseError, serde_as_text};

/// Where the volume is applied.
///
/// The text form is `system`, `software`, `hardware:<mixer element>` or `fixed`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum VolumeControl {
    /// The sound server's stream volume, so the desktop's mixer shows the same level.
    /// Backends without one use software volume.
    #[default]
    System,
    /// The player scales the samples itself.
    Software,
    /// An ALSA mixer element of the device, such as `PCM`; the samples are untouched.
    Hardware(String),
    /// Always full volume, for bit-perfect output.
    Fixed,
}

impl fmt::Display for VolumeControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::System => f.write_str("system"),
            Self::Software => f.write_str("software"),
            Self::Hardware(element) => write!(f, "hardware:{element}"),
            Self::Fixed => f.write_str("fixed"),
        }
    }
}

impl FromStr for VolumeControl {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "system" => Ok(Self::System),
            "software" => Ok(Self::Software),
            "fixed" => Ok(Self::Fixed),
            _ => match s.strip_prefix("hardware:") {
                Some(element) if !element.is_empty() => Ok(Self::Hardware(element.to_owned())),
                _ => Err(ParseError::new("volume control", s)),
            },
        }
    }
}

serde_as_text!(VolumeControl);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form() {
        for control in [
            VolumeControl::System,
            VolumeControl::Software,
            VolumeControl::Hardware("Master Playback".into()),
            VolumeControl::Fixed,
        ] {
            assert_eq!(control.to_string().parse(), Ok(control));
        }
        assert!("hardware:".parse::<VolumeControl>().is_err());
        assert!("hardware".parse::<VolumeControl>().is_err());
    }
}
