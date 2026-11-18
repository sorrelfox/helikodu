// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Headers that crash FFmpeg's own demuxers, refused before FFmpeg sees them.
//!
//! Fuzzing found these. Each check accepts every file the matching FFmpeg decoder
//! can play, so nothing playable is lost.
//!
//! - **ADX** (CRI): when the decoder refuses a header the demuxer accepted,
//!   `avformat_find_stream_info` goes on reading packets and the demuxer divides by
//!   a channel count of zero, which kills the process with `SIGFPE` (FFmpeg 9.0).
//!   A file that starts like ADX must therefore pass the decoder's own header
//!   checks.

use std::io::{Read, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::Error;
use crate::ffi::ff;
use crate::io::ByteSource;

/// The most of the file's start a check reads.
const HEAD_BYTES: usize = 64 * 1024;

/// Refuses a file whose start would crash an FFmpeg demuxer. Leaves the source at
/// its start. A source that cannot be read here is left to FFmpeg, whose reads
/// report the failure.
pub(crate) fn check_head(source: &mut dyn ByteSource) -> Result<(), Error> {
    let read = catch_unwind(AssertUnwindSafe(|| read_head(source)));
    match read {
        Ok(Ok(head)) if refused(&head) => Err(Error::ffmpeg(
            "avformat_open_input",
            ff::AVERROR_INVALIDDATA,
        )),
        _ => Ok(()),
    }
}

/// The start of the file, as far as a check needs it; the source is left at its
/// start.
fn read_head(source: &mut dyn ByteSource) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(64);
    source.seek(SeekFrom::Start(0))?;
    // The first bytes decide whether a check applies; most files end the read here.
    Read::take(&mut *source, 2).read_to_end(&mut head)?;
    if head.starts_with(&[0x80, 0x00]) {
        Read::take(&mut *source, HEAD_BYTES as u64 - 2).read_to_end(&mut head)?;
    }
    source.seek(SeekFrom::Start(0))?;
    Ok(head)
}

/// Whether `head`, the start of a file, is refused.
fn refused(head: &[u8]) -> bool {
    head.starts_with(&[0x80, 0x00]) && !adx_header_is_playable(head)
}

/// The checks of FFmpeg's ADX decoder (`ff_adx_decode_header`): standard ADX
/// (encoding 3, blocks of 18 bytes, 4 bits per sample), one or two channels, a
/// sample rate that fits, and the `(c)CRI` mark before the data when the file is
/// long enough to hold it.
fn adx_header_is_playable(head: &[u8]) -> bool {
    const BLOCK_SIZE: i64 = 18;
    if head.len() < 24 {
        return false;
    }
    let offset = usize::from(u16::from_be_bytes([head[2], head[3]])) + 4;
    if head.len() >= offset && offset >= 6 && &head[offset - 6..offset] != b"(c)CRI" {
        return false;
    }
    if head[4] != 3 || head[5] != 18 || head[6] != 4 {
        return false;
    }
    let channels = i64::from(head[7]);
    if !(1..=2).contains(&channels) {
        return false;
    }
    let rate = i64::from(i32::from_be_bytes([head[8], head[9], head[10], head[11]]));
    rate >= 1 && rate <= i64::from(i32::MAX) / (channels * BLOCK_SIZE * 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adx(encoding: u8, channels: u8, rate: u32) -> Vec<u8> {
        let mut b = vec![0x80, 0x00, 0x00, 0x20, encoding, 18, 4, channels];
        b.extend_from_slice(&rate.to_be_bytes());
        b.resize(0x1e, 0);
        b.extend_from_slice(b"(c)CRI");
        b.resize(64, 0);
        b
    }

    #[test]
    fn a_playable_adx_header_passes() {
        assert!(!refused(&adx(3, 2, 44_100)));
        assert!(!refused(&adx(3, 1, 22_050)));
    }

    #[test]
    fn adx_headers_the_decoder_refuses_are_refused() {
        assert!(refused(&adx(4, 2, 44_100)));
        assert!(refused(&adx(3, 0, 44_100)));
        assert!(refused(&adx(3, 3, 44_100)));
        assert!(refused(&adx(3, 2, 0)));
        assert!(refused(&adx(3, 2, u32::MAX)));
        let mut unmarked = adx(3, 2, 44_100);
        unmarked[0x1e] = b'x';
        assert!(refused(&unmarked));
        assert!(refused(&[0x80, 0x00, 0x00]));
    }

    #[test]
    fn other_files_pass() {
        assert!(!refused(b"fLaC\0\0\0\x22"));
        assert!(!refused(b""));
    }

    /// The input fuzzing found: an ADX header with 172 bits per sample.
    #[test]
    fn the_header_that_crashed_ffmpeg_is_refused() {
        let head = [
            0x80, 0x00, 0x00, 0x20, 0x03, 0x12, 0xac, 0x02, 0x00, 0x04, 0x00, 0x44, 0x00, 0x01,
            0x58, 0xa0, 0x01, 0xf4, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x28, 0x63, 0x29, 0x43, 0x52, 0x49, 0x00, 0x6f,
        ];
        assert!(refused(&head));
    }
}
