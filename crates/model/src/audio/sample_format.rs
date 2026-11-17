// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sample formats: how one sample of one channel is stored.

use std::fmt;
use std::str::FromStr;

use crate::text_form::{ParseError, serde_as_text};

/// The byte order of a sample that is wider than one byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endianness {
    /// Least significant byte first.
    Little,
    /// Most significant byte first.
    Big,
}

impl Endianness {
    /// The byte order of the machine the program runs on.
    pub const NATIVE: Self = if cfg!(target_endian = "big") {
        Self::Big
    } else {
        Self::Little
    };
}

/// How one sample of one channel is stored.
///
/// The text form follows ALSA's names in lower case: `u8`, `s16le`, `s24le`, `s24_3le`,
/// `s32be`, `f32le`, `dsd_u16be` and so on.
///
/// For the DSD formats, one sample is a word of 1-bit DSD samples of one channel, and
/// the sample rate counts 1-bit samples (2 822 400 Hz for DSD64), not words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SampleFormat {
    /// Unsigned 8-bit integer.
    U8,
    /// Signed 16-bit integer.
    S16(Endianness),
    /// Signed 24-bit integer in the low three bytes of a 4-byte word.
    S24(Endianness),
    /// Signed 24-bit integer packed into 3 bytes.
    S24Packed(Endianness),
    /// Signed 32-bit integer.
    S32(Endianness),
    /// 32-bit IEEE 754 float, nominally between -1.0 and 1.0.
    F32(Endianness),
    /// 64-bit IEEE 754 float, nominally between -1.0 and 1.0.
    F64(Endianness),
    /// 8 DSD bits in one byte, oldest bit in the most significant position.
    DsdU8,
    /// 16 DSD bits in a 2-byte word.
    DsdU16(Endianness),
    /// 32 DSD bits in a 4-byte word.
    DsdU32(Endianness),
}

impl SampleFormat {
    /// Every sample format, in a fixed order.
    pub const ALL: [Self; 18] = {
        use Endianness::{Big, Little};
        [
            Self::U8,
            Self::S16(Little),
            Self::S16(Big),
            Self::S24(Little),
            Self::S24(Big),
            Self::S24Packed(Little),
            Self::S24Packed(Big),
            Self::S32(Little),
            Self::S32(Big),
            Self::F32(Little),
            Self::F32(Big),
            Self::F64(Little),
            Self::F64(Big),
            Self::DsdU8,
            Self::DsdU16(Little),
            Self::DsdU16(Big),
            Self::DsdU32(Little),
            Self::DsdU32(Big),
        ]
    };

    /// The number of significant bits of one sample. For DSD, the number of 1-bit
    /// samples in one word.
    pub const fn bits(self) -> u32 {
        match self {
            Self::U8 | Self::DsdU8 => 8,
            Self::S16(_) | Self::DsdU16(_) => 16,
            Self::S24(_) | Self::S24Packed(_) => 24,
            Self::S32(_) | Self::F32(_) | Self::DsdU32(_) => 32,
            Self::F64(_) => 64,
        }
    }

    /// The number of bytes one sample occupies.
    pub const fn bytes(self) -> usize {
        match self {
            Self::U8 | Self::DsdU8 => 1,
            Self::S16(_) | Self::DsdU16(_) => 2,
            Self::S24Packed(_) => 3,
            Self::S24(_) | Self::S32(_) | Self::F32(_) | Self::DsdU32(_) => 4,
            Self::F64(_) => 8,
        }
    }

    /// The byte order, or `None` for the one-byte formats.
    pub const fn endianness(self) -> Option<Endianness> {
        match self {
            Self::U8 | Self::DsdU8 => None,
            Self::S16(e)
            | Self::S24(e)
            | Self::S24Packed(e)
            | Self::S32(e)
            | Self::F32(e)
            | Self::F64(e)
            | Self::DsdU16(e)
            | Self::DsdU32(e) => Some(e),
        }
    }

    /// Whether this is an integer PCM format.
    pub const fn is_integer(self) -> bool {
        matches!(
            self,
            Self::U8 | Self::S16(_) | Self::S24(_) | Self::S24Packed(_) | Self::S32(_)
        )
    }

    /// Whether this is a floating-point PCM format.
    pub const fn is_float(self) -> bool {
        matches!(self, Self::F32(_) | Self::F64(_))
    }

    /// Whether this is a DSD (1-bit) format.
    pub const fn is_dsd(self) -> bool {
        matches!(self, Self::DsdU8 | Self::DsdU16(_) | Self::DsdU32(_))
    }

    /// The text form, for example `s24_3le`.
    pub const fn as_str(self) -> &'static str {
        use Endianness::{Big, Little};
        match self {
            Self::U8 => "u8",
            Self::S16(Little) => "s16le",
            Self::S16(Big) => "s16be",
            Self::S24(Little) => "s24le",
            Self::S24(Big) => "s24be",
            Self::S24Packed(Little) => "s24_3le",
            Self::S24Packed(Big) => "s24_3be",
            Self::S32(Little) => "s32le",
            Self::S32(Big) => "s32be",
            Self::F32(Little) => "f32le",
            Self::F32(Big) => "f32be",
            Self::F64(Little) => "f64le",
            Self::F64(Big) => "f64be",
            Self::DsdU8 => "dsd_u8",
            Self::DsdU16(Little) => "dsd_u16le",
            Self::DsdU16(Big) => "dsd_u16be",
            Self::DsdU32(Little) => "dsd_u32le",
            Self::DsdU32(Big) => "dsd_u32be",
        }
    }
}

impl fmt::Display for SampleFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SampleFormat {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|format| format.as_str() == s)
            .ok_or_else(|| ParseError::new("sample format", s))
    }
}

serde_as_text!(SampleFormat);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form_round_trips_for_every_format() {
        for format in SampleFormat::ALL {
            assert_eq!(format.as_str().parse::<SampleFormat>(), Ok(format));
        }
        assert!("s24".parse::<SampleFormat>().is_err());
    }

    #[test]
    fn text_forms_are_unique() {
        let mut names: Vec<_> = SampleFormat::ALL.iter().map(|f| f.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SampleFormat::ALL.len());
    }

    #[test]
    fn sizes_and_kinds() {
        let s24 = SampleFormat::S24(Endianness::Little);
        assert_eq!((s24.bits(), s24.bytes()), (24, 4));
        let packed = SampleFormat::S24Packed(Endianness::Big);
        assert_eq!((packed.bits(), packed.bytes()), (24, 3));
        assert_eq!(SampleFormat::U8.endianness(), None);
        for format in SampleFormat::ALL {
            let kinds = [format.is_integer(), format.is_float(), format.is_dsd()];
            assert_eq!(kinds.iter().filter(|k| **k).count(), 1, "{format}");
            assert!(format.bits() as usize <= format.bytes() * 8);
        }
    }

    #[test]
    fn serializes_as_text() {
        let json = serde_json::to_string(&SampleFormat::F32(Endianness::Little)).unwrap();
        assert_eq!(json, "\"f32le\"");
        let back: SampleFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SampleFormat::F32(Endianness::Little));
    }
}
