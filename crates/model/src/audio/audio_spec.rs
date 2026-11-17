// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The full description of an interleaved audio stream.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{ChannelLayout, SampleFormat, SampleRate};

/// Sample format, sample rate and channel layout of an interleaved audio stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AudioSpec {
    /// How each sample is stored.
    pub format: SampleFormat,
    /// Frames per second.
    pub rate: SampleRate,
    /// The channels and their order within a frame.
    pub layout: ChannelLayout,
}

impl AudioSpec {
    /// A spec from its three parts.
    pub const fn new(format: SampleFormat, rate: SampleRate, layout: ChannelLayout) -> Self {
        Self {
            format,
            rate,
            layout,
        }
    }

    /// The number of channels.
    pub const fn channel_count(&self) -> u16 {
        self.layout.channel_count()
    }

    /// The number of bytes of one interleaved frame (one sample of every channel).
    pub const fn bytes_per_frame(&self) -> usize {
        self.format.bytes() * self.layout.channel_count() as usize
    }
}

impl fmt::Display for AudioSpec {
    /// For example `s16le 44100 Hz stereo`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.format, self.rate, self.layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Endianness;

    #[test]
    fn frame_size_and_display() {
        let spec = AudioSpec::new(
            SampleFormat::S24Packed(Endianness::Little),
            SampleRate::HZ_48000,
            ChannelLayout::SURROUND_5_1,
        );
        assert_eq!(spec.bytes_per_frame(), 18);
        assert_eq!(spec.to_string(), "s24_3le 48000 Hz 5.1");
    }
}
