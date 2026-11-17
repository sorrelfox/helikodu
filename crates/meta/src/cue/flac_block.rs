// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The binary `CUESHEET` metadata block of a FLAC file, as `flac --cuesheet` and
//! `metaflac --import-cuesheet-from` write it.
//!
//! The block holds the media catalogue number and, per track, its offset in
//! samples, number, ISRC, audio and pre-emphasis flags and index points relative
//! to the track's offset. It has no titles or performers. The last track is the
//! lead-out (170 on a CD, 255 otherwise).

use std::io::{self, Read, Seek, SeekFrom};
use std::num::NonZeroU32;

use helikodu_model::audio::MediaTime;

/// The most metadata blocks looked at before giving up.
const MAX_BLOCKS: usize = 1024;
/// The largest cue sheet block read; larger ones are not real cue sheets.
const MAX_BLOCK_BYTES: usize = 1 << 20;
/// Block types.
const STREAMINFO: u8 = 0;
const CUESHEET: u8 = 5;

/// A FLAC cue sheet block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FlacCueSheet {
    pub(crate) catalog: Option<String>,
    pub(crate) tracks: Vec<BlockTrack>,
}

/// One track of the block, the lead-out included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockTrack {
    pub(crate) offset: u64,
    pub(crate) number: u8,
    pub(crate) isrc: Option<String>,
    pub(crate) audio: bool,
    pub(crate) pre_emphasis: bool,
    /// Index number and offset relative to the track.
    pub(crate) indexes: Vec<(u8, u64)>,
}

impl BlockTrack {
    pub(crate) fn is_lead_out(&self) -> bool {
        matches!(self.number, 170 | 255)
    }

    /// The position of index `number` in the file, in samples.
    pub(crate) fn index(&self, number: u8) -> Option<u64> {
        self.indexes
            .iter()
            .find(|(n, _)| *n == number)
            .and_then(|(_, offset)| self.offset.checked_add(*offset))
    }
}

/// A time of `samples` samples at `rate`.
pub(crate) fn time(samples: u64, rate: NonZeroU32) -> MediaTime {
    MediaTime::new(samples, rate)
}

/// Reads the cue sheet block and the sample rate of the FLAC stream in `file`, if
/// it is a FLAC file with such a block.
pub(crate) fn read(
    file: &mut (impl Read + Seek),
) -> io::Result<Option<(FlacCueSheet, NonZeroU32)>> {
    file.seek(SeekFrom::Start(0))?;
    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_err() {
        return Ok(None);
    }
    if magic.starts_with(b"ID3") {
        // An ID3v2 tag before the stream: a 10-byte header with a syncsafe size.
        let mut rest = [0u8; 6];
        file.read_exact(&mut rest)?;
        let size = rest[2..]
            .iter()
            .fold(0u64, |acc, &b| (acc << 7) | u64::from(b & 0x7f));
        let footer = if rest[1] & 0x10 != 0 { 10 } else { 0 };
        file.seek(SeekFrom::Start(10 + size + footer))?;
        if file.read_exact(&mut magic).is_err() {
            return Ok(None);
        }
    }
    if &magic != b"fLaC" {
        return Ok(None);
    }
    let mut rate = None;
    let mut sheet = None;
    for _ in 0..MAX_BLOCKS {
        let mut header = [0u8; 4];
        if file.read_exact(&mut header).is_err() {
            break;
        }
        let last = header[0] & 0x80 != 0;
        let kind = header[0] & 0x7f;
        let len =
            usize::from(header[1]) << 16 | usize::from(header[2]) << 8 | usize::from(header[3]);
        match kind {
            STREAMINFO | CUESHEET if len <= MAX_BLOCK_BYTES => {
                let mut data = vec![0u8; len];
                file.read_exact(&mut data)?;
                if kind == STREAMINFO {
                    rate = stream_rate(&data);
                } else {
                    sheet = parse(&data);
                }
            }
            _ => {
                file.seek(SeekFrom::Current(len as i64))?;
            }
        }
        if last {
            break;
        }
    }
    Ok(sheet.zip(rate))
}

/// The sample rate in a STREAMINFO block: 20 bits at byte 10.
fn stream_rate(data: &[u8]) -> Option<NonZeroU32> {
    let b = data.get(10..13)?;
    NonZeroU32::new(u32::from(b[0]) << 12 | u32::from(b[1]) << 4 | u32::from(b[2]) >> 4)
}

/// Reads from a byte slice, failing at its end.
struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u64(&mut self) -> Option<u64> {
        let b: [u8; 8] = self.take(8)?.try_into().ok()?;
        Some(u64::from_be_bytes(b))
    }
}

/// ASCII text in a NUL-padded field, if any.
fn ascii(field: &[u8]) -> Option<String> {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    let text = &field[..end];
    (!text.is_empty() && text.iter().all(|b| b.is_ascii_graphic()))
        .then(|| String::from_utf8_lossy(text).into_owned())
}

/// Parses the body of a CUESHEET block; `None` when it is truncated.
pub(crate) fn parse(data: &[u8]) -> Option<FlacCueSheet> {
    let mut c = Cursor(data);
    let catalog = ascii(c.take(128)?);
    let _lead_in = c.u64()?;
    // One bit "is CD", 7 + 258 × 8 reserved bits.
    c.take(1 + 258)?;
    let count = c.u8()?;
    let mut tracks = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let offset = c.u64()?;
        let number = c.u8()?;
        let isrc = ascii(c.take(12)?);
        let flags = c.u8()?;
        c.take(13)?;
        let index_count = c.u8()?;
        let mut indexes = Vec::with_capacity(usize::from(index_count));
        for _ in 0..index_count {
            let offset = c.u64()?;
            let number = c.u8()?;
            c.take(3)?;
            indexes.push((number, offset));
        }
        tracks.push(BlockTrack {
            offset,
            number,
            isrc,
            audio: flags & 0x80 == 0,
            pre_emphasis: flags & 0x40 != 0,
            indexes,
        });
    }
    Some(FlacCueSheet { catalog, tracks })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block with two audio tracks and the lead-out.
    fn block() -> Vec<u8> {
        let mut b = Vec::new();
        let mut mcn = b"1234567890123".to_vec();
        mcn.resize(128, 0);
        b.extend_from_slice(&mcn);
        b.extend_from_slice(&88_200u64.to_be_bytes());
        b.push(0x80);
        b.extend_from_slice(&[0; 258]);
        b.push(3);
        let mut track = |offset: u64, number: u8, flags: u8, indexes: &[(u8, u64)]| {
            b.extend_from_slice(&offset.to_be_bytes());
            b.push(number);
            b.extend_from_slice(&[0; 12]);
            b.push(flags);
            b.extend_from_slice(&[0; 13]);
            b.push(indexes.len() as u8);
            for (n, o) in indexes {
                b.extend_from_slice(&o.to_be_bytes());
                b.push(*n);
                b.extend_from_slice(&[0; 3]);
            }
        };
        track(0, 1, 0, &[(1, 0)]);
        track(132_300, 2, 0x40, &[(0, 0), (1, 44_100)]);
        track(441_000, 170, 0, &[]);
        b
    }

    #[test]
    fn a_block_parses() {
        let sheet = parse(&block()).expect("valid block");
        assert_eq!(sheet.catalog.as_deref(), Some("1234567890123"));
        assert_eq!(sheet.tracks.len(), 3);
        assert_eq!(sheet.tracks[1].index(1), Some(176_400));
        assert_eq!(sheet.tracks[1].index(0), Some(132_300));
        assert!(sheet.tracks[1].pre_emphasis);
        assert!(sheet.tracks[2].is_lead_out());
    }

    #[test]
    fn truncated_blocks_are_refused() {
        let full = block();
        for len in 0..full.len() {
            assert_eq!(parse(&full[..len]), None, "{len}");
        }
    }
}
