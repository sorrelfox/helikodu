// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Exact positions for the first frame after a seek, where time stamps are not
//! exact.
//!
//! A seek decodes from a point before its target and needs the exact position of
//! the first frame it decodes. Time stamps give it for most formats, but not for
//! all:
//!
//! - **Ogg FLAC.** An Ogg granule position marks the end of the last packet that
//!   ends on a page, and FFmpeg has no packet-duration callback for FLAC in Ogg.
//!   After a seek it stamps the first packet with a time derived from the page's
//!   granule, which is up to a page of packets late (8192 frames for a page of two
//!   4096-frame packets).
//! - **Millisecond time stamps** (Matroska, WebM, ASF, FLV) are rounded or cut to
//!   the millisecond: tens of frames off.
//! - **ADPCM in WAV.** After a seek FFmpeg derives the time stamp from the byte
//!   position and the header's average byte rate, which is rounded: one or two
//!   frames off, more late in long files.
//! - **RealAudio 1.0 (`.ra`, 14.4)** has no time stamps at all: every packet says 0.
//!
//! Three kinds of anchors fix this, in this order:
//!
//! 1. [`Anchor::Flac`]: every FLAC frame header carries its frame or sample number.
//! 2. [`Anchor::Blocks`]: packets are whole blocks of a fixed byte size and frame
//!    count, stored back to back, so a packet's byte position gives its frame
//!    position. Old RealAudio files are also *seeked* by byte position this way.
//! 3. [`Grid`]: when every packet holds a whole number of codec frames of a fixed
//!    size, positions lie on a grid; a position from a coarse time stamp is
//!    snapped to the nearest grid point, which is exact as long as the time stamp
//!    is off by less than half a grid step.
//!
//! The per-packet anchors travel from the packet to the decoded frame in
//! `AVPacket.opaque` / `AVFrame.opaque` (`AV_CODEC_FLAG_COPY_OPAQUE`), as the
//! anchor plus one, so that 0 (NULL) means "none".

use std::ffi::c_void;
use std::ptr;

/// A per-packet source of exact positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Anchor {
    None,
    /// FLAC frame headers. `fixed_block` is the block size of a fixed-block-size
    /// stream (from STREAMINFO), whose headers carry frame numbers.
    Flac {
        fixed_block: Option<u64>,
    },
    /// Blocks of `bytes` bytes holding `frames` frames each, back to back from the
    /// first packet. With `byte_seek`, seeks go to a byte position too.
    Blocks {
        bytes: i64,
        frames: i64,
        byte_seek: bool,
    },
}

impl Anchor {
    /// The anchor for FLAC, from the codec's extradata (STREAMINFO, possibly
    /// preceded by the `fLaC` marker and a metadata block header).
    pub(crate) fn flac(extradata: &[u8]) -> Self {
        let info = match extradata {
            [b'f', b'L', b'a', b'C', _, _, _, _, rest @ ..] => rest,
            other => other,
        };
        let fixed_block = match info {
            [a, b, c, d, ..] => {
                let min = u16::from_be_bytes([*a, *b]);
                let max = u16::from_be_bytes([*c, *d]);
                (min == max && min > 0).then_some(u64::from(min))
            }
            _ => None,
        };
        Self::Flac { fixed_block }
    }

    /// Whether seeks go to a byte position: the stream has no usable time stamps.
    pub(crate) fn seeks_by_byte(&self) -> bool {
        matches!(
            self,
            Self::Blocks {
                byte_seek: true,
                ..
            }
        )
    }

    /// The exact sample position of a packet, in frames from the start of the
    /// codec's numbering (before subtracting the stream's first position).
    /// `pos` is the packet's byte position, `data_start` that of the first packet.
    pub(crate) fn of_packet(&self, data: &[u8], pos: i64, data_start: Option<i64>) -> Option<i64> {
        match *self {
            Self::None => None,
            Self::Flac { fixed_block } => {
                flac_frame_start(data, fixed_block).and_then(|n| i64::try_from(n).ok())
            }
            Self::Blocks { bytes, frames, .. } => {
                let offset = pos.checked_sub(data_start?)?;
                (pos >= 0 && offset >= 0 && bytes > 0 && offset % bytes == 0)
                    .then(|| (offset / bytes).checked_mul(frames))
                    .flatten()
            }
        }
    }
}

/// An anchor as an `opaque` pointer value: the anchor plus one, NULL for none.
pub(crate) fn to_opaque(anchor: Option<i64>) -> *mut c_void {
    anchor
        .and_then(|a| a.checked_add(1))
        .and_then(|a| usize::try_from(a).ok())
        .map_or(ptr::null_mut(), ptr::without_provenance_mut)
}

/// The anchor an `opaque` pointer value carries.
pub(crate) fn from_opaque(opaque: *mut c_void) -> Option<i64> {
    let value = i64::try_from(opaque.addr()).ok()?;
    (value > 0).then(|| value - 1)
}

/// Positions that lie on a grid: `offset + k * step`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Grid {
    pub(crate) step: i64,
    pub(crate) offset: i64,
}

impl Grid {
    /// The grid point nearest to `position`.
    pub(crate) fn snap(&self, position: i64) -> i64 {
        let from = position - self.offset;
        let below = from - from.rem_euclid(self.step);
        let nearest = if (from - below) * 2 >= self.step {
            below + self.step
        } else {
            below
        };
        nearest + self.offset
    }
}

/// The frame position of a FLAC frame from its header: the sample number of a
/// variable-block-size stream, or frame number times the block size of a fixed one.
/// `None` when the header is not valid (sync code, reserved values, CRC-8).
pub(crate) fn flac_frame_start(data: &[u8], fixed_block: Option<u64>) -> Option<u64> {
    let [0xFF, sync @ (0xF8 | 0xF9), size_rate, channels_bits, ..] = *data else {
        return None;
    };
    let variable = sync == 0xF9;
    let block_code = size_rate >> 4;
    let rate_code = size_rate & 0x0F;
    if block_code == 0 || rate_code == 0x0F || channels_bits >> 4 > 10 || channels_bits & 1 != 0 {
        return None;
    }
    let (number, used) = coded_number(&data[4..])?;
    let mut end = 4 + used;
    end += match block_code {
        6 => 1,
        7 => 2,
        _ => 0,
    };
    end += match rate_code {
        12 => 1,
        13 | 14 => 2,
        _ => 0,
    };
    let crc = *data.get(end)?;
    if crc8(&data[..end]) != crc {
        return None;
    }
    if variable {
        Some(number)
    } else {
        number.checked_mul(fixed_block?)
    }
}

/// FLAC's UTF-8-like coded number: the value and the bytes it used.
fn coded_number(data: &[u8]) -> Option<(u64, usize)> {
    let first = *data.first()?;
    let len = first.leading_ones() as usize;
    let (mut value, len) = match len {
        0 => return Some((u64::from(first), 1)),
        2..=7 => (u64::from(first) & (0x7F >> len), len),
        _ => return None,
    };
    for &byte in data.get(1..len)? {
        if byte & 0xC0 != 0x80 {
            return None;
        }
        value = (value << 6) | u64::from(byte & 0x3F);
    }
    Some((value, len))
}

/// CRC-8 with polynomial 0x07, as in FLAC frame headers.
fn crc8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |mut crc, &byte| {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
        crc
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame header with the given sync byte and coded number bytes; block size
    /// code 0xC (4096), rate code 9 (44.1 kHz), stereo, 16 bit.
    fn header(sync: u8, number: &[u8]) -> Vec<u8> {
        let mut h = vec![0xFF, sync, 0xC9, 0x18];
        h.extend_from_slice(number);
        h.push(crc8(&h));
        h
    }

    #[test]
    fn reads_frame_and_sample_numbers() {
        // Frame 0 and frame 19 of a fixed-block-size stream.
        assert_eq!(flac_frame_start(&header(0xF8, &[0]), Some(4096)), Some(0));
        assert_eq!(
            flac_frame_start(&header(0xF8, &[19]), Some(4096)),
            Some(19 * 4096)
        );
        // Two-byte coded number: 0xC2 0x80 = 0x80.
        assert_eq!(
            flac_frame_start(&header(0xF8, &[0xC2, 0x80]), Some(4096)),
            Some(128 * 4096)
        );
        // A variable-block-size stream carries the sample number.
        assert_eq!(
            flac_frame_start(&header(0xF9, &[0xC2, 0x80]), None),
            Some(128)
        );
        // Fixed block size without STREAMINFO: unknown.
        assert_eq!(flac_frame_start(&header(0xF8, &[1]), None), None);
    }

    #[test]
    fn refuses_damaged_headers() {
        let mut h = header(0xF8, &[5]);
        *h.last_mut().expect("crc") ^= 1;
        assert_eq!(flac_frame_start(&h, Some(4096)), None);
        assert_eq!(flac_frame_start(&[0xFF, 0xF8, 0xC9], Some(4096)), None);
        assert_eq!(flac_frame_start(b"OggS....", Some(4096)), None);
        // A continuation byte where the number needs one.
        assert_eq!(
            flac_frame_start(&header(0xF8, &[0xC2, 0x00]), Some(4096)),
            None
        );
    }

    #[test]
    fn reads_the_block_size_from_streaminfo() {
        let mut info = vec![0x10, 0x00, 0x10, 0x00];
        info.extend_from_slice(&[0; 30]);
        assert_eq!(
            Anchor::flac(&info),
            Anchor::Flac {
                fixed_block: Some(4096)
            }
        );
        let mut marked = b"fLaC\x80\x00\x00\x22".to_vec();
        marked.extend_from_slice(&info);
        assert_eq!(
            Anchor::flac(&marked),
            Anchor::Flac {
                fixed_block: Some(4096)
            }
        );
        let variable = [0x00, 0x10, 0x10, 0x00];
        assert_eq!(Anchor::flac(&variable), Anchor::Flac { fixed_block: None });
    }

    #[test]
    fn blocks_count_from_the_first_packet() {
        let a = Anchor::Blocks {
            bytes: 1024,
            frames: 1012,
            byte_seek: false,
        };
        assert_eq!(a.of_packet(&[], 90, Some(90)), Some(0));
        assert_eq!(a.of_packet(&[], 90 + 3 * 1024, Some(90)), Some(3 * 1012));
        assert_eq!(a.of_packet(&[], 91, Some(90)), None);
        assert_eq!(a.of_packet(&[], 10, Some(90)), None);
        assert_eq!(a.of_packet(&[], -1, Some(90)), None);
        assert_eq!(a.of_packet(&[], 90, None), None);
    }

    #[test]
    fn opaque_round_trips() {
        assert!(to_opaque(None).is_null());
        assert_eq!(from_opaque(to_opaque(Some(0))), Some(0));
        assert_eq!(from_opaque(to_opaque(Some(123_456))), Some(123_456));
        assert_eq!(from_opaque(ptr::null_mut()), None);
        assert!(to_opaque(Some(-5)).is_null());
    }

    #[test]
    fn snaps_to_the_nearest_grid_point() {
        let g = Grid {
            step: 120,
            offset: 48,
        };
        assert_eq!(g.snap(672), 648);
        assert_eq!(g.snap(624), 648);
        assert_eq!(g.snap(48), 48);
        assert_eq!(g.snap(0), 48);
        assert_eq!(g.snap(-20), -72);
        assert_eq!(g.snap(-80), -72);
    }
}
