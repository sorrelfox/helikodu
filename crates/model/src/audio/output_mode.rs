// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared and exclusive use of an output device.

use std::fmt;
use std::str::FromStr;

use crate::text_form::{ParseError, serde_as_text};

/// Whether the player shares the device with other applications.
///
/// The text form is `shared` or `exclusive`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OutputMode {
    /// Other applications play at the same time; the sound server may convert.
    #[default]
    Shared,
    /// The player holds the device alone, for bit-perfect output where the backend
    /// supports it.
    Exclusive,
}

impl OutputMode {
    /// The text form.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Exclusive => "exclusive",
        }
    }
}

impl fmt::Display for OutputMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for OutputMode {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "shared" => Ok(Self::Shared),
            "exclusive" => Ok(Self::Exclusive),
            _ => Err(ParseError::new("output mode", s)),
        }
    }
}

serde_as_text!(OutputMode);
