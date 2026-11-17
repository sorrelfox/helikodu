// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sample rates.

use std::fmt;
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

/// A sample rate in hertz: frames per second. Never zero.
///
/// Serialized as the plain number of hertz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SampleRate(NonZeroU32);

impl SampleRate {
    /// 44 100 Hz, the rate of the compact disc.
    pub const HZ_44100: Self = Self(NonZeroU32::new(44_100).expect("not zero"));
    /// 48 000 Hz, the usual rate of sound servers and video.
    pub const HZ_48000: Self = Self(NonZeroU32::new(48_000).expect("not zero"));

    /// A rate of `hz` hertz, or `None` if `hz` is zero.
    pub const fn new(hz: u32) -> Option<Self> {
        match NonZeroU32::new(hz) {
            Some(hz) => Some(Self(hz)),
            None => None,
        }
    }

    /// The rate in hertz.
    pub const fn hz(self) -> u32 {
        self.0.get()
    }

    /// The rate in hertz, as a non-zero integer.
    pub const fn get(self) -> NonZeroU32 {
        self.0
    }
}

impl From<NonZeroU32> for SampleRate {
    fn from(hz: NonZeroU32) -> Self {
        Self(hz)
    }
}

impl fmt::Display for SampleRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} Hz", self.hz())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_rejected() {
        assert_eq!(SampleRate::new(0), None);
        assert!(serde_json::from_str::<SampleRate>("0").is_err());
    }

    #[test]
    fn serializes_as_a_number() {
        assert_eq!(
            serde_json::to_string(&SampleRate::HZ_44100).unwrap(),
            "44100"
        );
        let rate: SampleRate = serde_json::from_str("96000").unwrap();
        assert_eq!(rate.hz(), 96_000);
    }
}
