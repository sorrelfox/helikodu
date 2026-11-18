// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! DSD over PCM (DoP 1.1): the raw stream packed into 24-bit PCM frames.
//!
//! Each PCM sample carries a marker byte in its top eight bits and sixteen DSD bits
//! of its channel below it, the earlier byte first and the earliest bit most
//! significant. The marker alternates between `0x05` and `0xFA` from one frame to
//! the next, the same for every channel of a frame; frame 0 of the stream has
//! `0x05`, so the alternation is the same wherever playback starts. The PCM rate is
//! the DSD rate divided by 16: DSD64 becomes 176.4 kHz, DSD128 352.8 kHz.
//!
//! The samples are 24-bit values left-justified in 32-bit words, as `i32`. A stream
//! with an odd number of bytes per channel ends with the DSD idle pattern.

use std::path::Path;

use super::error::DsdError;
use super::reader::{DsdInfo, DsdReader, IDLE_BYTE};

/// The two DoP markers, for even and odd frames.
pub const DOP_MARKERS: [u8; 2] = [0x05, 0xFA];

/// Bytes read per channel at a time.
const CHUNK: usize = 8192;

/// One DoP sample: `marker`, then the DSD bytes `first` and `second`, as a 24-bit
/// value left-justified in a 32-bit word.
pub fn dop_word(marker: u8, first: u8, second: u8) -> i32 {
    i32::from_be_bytes([marker, first, second, 0])
}

/// The DoP sample of DSD silence (the idle pattern) for frame `frame`.
pub fn dop_silence(frame: u64) -> i32 {
    dop_word(DOP_MARKERS[(frame & 1) as usize], IDLE_BYTE, IDLE_BYTE)
}

/// A DSF or DFF file packed into DoP frames.
#[derive(Debug)]
pub struct DopDecoder {
    reader: DsdReader,
    /// The next frame.
    frame: u64,
    bytes: Vec<Vec<u8>>,
}

impl DopDecoder {
    /// Opens the DSD file at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DsdError> {
        Ok(Self::new(DsdReader::open(path)?))
    }

    /// Packs the stream of `reader` from its current position, rounded down to a
    /// whole frame.
    pub fn new(mut reader: DsdReader) -> Self {
        let frame = reader.position() / 2;
        reader.seek(frame * 2);
        let channels = reader.info().channels();
        Self {
            reader,
            frame,
            bytes: vec![Vec::new(); channels],
        }
    }

    /// What the file holds.
    pub fn info(&self) -> &DsdInfo {
        self.reader.info()
    }

    /// The PCM rate: the DSD rate divided by 16.
    pub fn rate(&self) -> u32 {
        self.reader.info().dsd_rate / 16
    }

    /// The number of frames.
    pub fn frames(&self) -> u64 {
        self.reader.bytes_per_channel().div_ceil(2)
    }

    /// The next frame [`read`](Self::read) gives.
    pub fn position(&self) -> u64 {
        self.frame
    }

    /// Moves to frame `frame` (clamped to the end).
    pub fn seek(&mut self, frame: u64) {
        self.frame = frame.min(self.frames());
        self.reader.seek(self.frame * 2);
    }

    /// Appends up to `max_frames` interleaved frames to `out` (cleared first) and
    /// returns how many; 0 at the end.
    pub fn read(&mut self, max_frames: usize, out: &mut Vec<i32>) -> Result<usize, DsdError> {
        out.clear();
        let channels = self.bytes.len();
        let want = max_frames.min(CHUNK / 2) * 2;
        for bytes in &mut self.bytes {
            bytes.clear();
        }
        let got = self.reader.read(want, &mut self.bytes)?;
        if got == 0 {
            return Ok(0);
        }
        let frames = got.div_ceil(2);
        out.reserve(frames * channels);
        for i in 0..frames {
            let marker = DOP_MARKERS[((self.frame + i as u64) & 1) as usize];
            for bytes in &self.bytes {
                let first = bytes[2 * i];
                let second = bytes.get(2 * i + 1).copied().unwrap_or(IDLE_BYTE);
                out.push(dop_word(marker, first, second));
            }
        }
        self.frame += frames as u64;
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_hold_the_marker_and_two_bytes() {
        assert_eq!(dop_word(0x05, 0x12, 0x34) as u32, 0x0512_3400);
        assert_eq!(dop_word(0xFA, 0xFF, 0x01) as u32, 0xFAFF_0100);
        assert_eq!(dop_silence(0) as u32, 0x0569_6900);
        assert_eq!(dop_silence(1) as u32, 0xFA69_6900);
    }
}
