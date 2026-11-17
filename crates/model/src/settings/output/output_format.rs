// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sample format sent to the device.

use std::fmt;
use std::str::FromStr;

use crate::audio::{Endianness, SampleFormat};
use crate::text_form::{ParseError, serde_as_text};

/// The sample format to send to the device. The byte order is always the device's.
///
/// The text form is `auto`, `s16`, `s24`, `s24_3`, `s32` or `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OutputFormat {
    /// The track's own format if the device accepts it, otherwise the device's best.
    #[default]
    Auto,
    /// Signed 16-bit integer.
    S16,
    /// Signed 24-bit integer in a 4-byte word.
    S24,
    /// Signed 24-bit integer packed into 3 bytes.
    S24Packed,
    /// Signed 32-bit integer.
    S32,
    /// 32-bit float.
    F32,
}

impl OutputFormat {
    const ALL: [Self; 6] = [
        Self::Auto,
        Self::S16,
        Self::S24,
        Self::S24Packed,
        Self::S32,
        Self::F32,
    ];

    /// The text form.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::S16 => "s16",
            Self::S24 => "s24",
            Self::S24Packed => "s24_3",
            Self::S32 => "s32",
            Self::F32 => "f32",
        }
    }

    /// The sample format in the byte order `endianness`, or `None` for
    /// [`Auto`](Self::Auto).
    pub const fn sample_format(self, endianness: Endianness) -> Option<SampleFormat> {
        match self {
            Self::Auto => None,
            Self::S16 => Some(SampleFormat::S16(endianness)),
            Self::S24 => Some(SampleFormat::S24(endianness)),
            Self::S24Packed => Some(SampleFormat::S24Packed(endianness)),
            Self::S32 => Some(SampleFormat::S32(endianness)),
            Self::F32 => Some(SampleFormat::F32(endianness)),
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for OutputFormat {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|format| format.as_str() == s)
            .ok_or_else(|| ParseError::new("output format", s))
    }
}

serde_as_text!(OutputFormat);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form_round_trips() {
        for format in OutputFormat::ALL {
            assert_eq!(format.as_str().parse(), Ok(format));
        }
        assert!("s24le".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn maps_to_sample_formats() {
        assert_eq!(OutputFormat::Auto.sample_format(Endianness::Little), None);
        assert_eq!(
            OutputFormat::S24Packed.sample_format(Endianness::Big),
            Some(SampleFormat::S24Packed(Endianness::Big))
        );
    }
}
