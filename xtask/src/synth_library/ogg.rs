// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Ogg Opus files from a template: the template's `OpusHead` page and audio
//! pages, with a new `OpusTags` packet in between.
//!
//! An Ogg page is a 27-byte header (`OggS`, version, flags, granule position,
//! stream serial number, page sequence number, CRC, segment count), the
//! segment table (lacing values) and the body. The CRC covers the whole page
//! with the CRC field set to zero. `OpusTags` starts on the second page and
//! ends a page; audio starts on a new page. When the new tags need a
//! different number of pages than the template's, every later page gets a
//! new sequence number and so a new CRC.

use anyhow::{Result, bail};

const HEADER_LENGTH: usize = 27;
const CONTINUED: u8 = 0x01;
const BEGINNING: u8 = 0x02;

/// One page, borrowed from a file.
#[derive(Debug, Clone, Copy)]
struct Page<'a> {
    raw: &'a [u8],
    flags: u8,
    serial: u32,
    sequence: u32,
    lacing: &'a [u8],
    body: &'a [u8],
}

fn pages(bytes: &[u8]) -> Result<Vec<Page<'_>>> {
    let mut out = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        if rest.len() < HEADER_LENGTH || &rest[..4] != b"OggS" || rest[4] != 0 {
            bail!("not an Ogg page at byte {}", bytes.len() - rest.len());
        }
        let segments = usize::from(rest[26]);
        if rest.len() < HEADER_LENGTH + segments {
            bail!("truncated Ogg page");
        }
        let lacing = &rest[HEADER_LENGTH..HEADER_LENGTH + segments];
        let body_length: usize = lacing.iter().map(|&value| usize::from(value)).sum();
        let total = HEADER_LENGTH + segments + body_length;
        if rest.len() < total {
            bail!("truncated Ogg page");
        }
        let raw = &rest[..total];
        let stored = u32::from_le_bytes([raw[22], raw[23], raw[24], raw[25]]);
        if crc_of_page(raw) != stored {
            bail!("an Ogg page has a wrong CRC");
        }
        out.push(Page {
            raw,
            flags: raw[5],
            serial: u32::from_le_bytes([raw[14], raw[15], raw[16], raw[17]]),
            sequence: u32::from_le_bytes([raw[18], raw[19], raw[20], raw[21]]),
            lacing,
            body: &raw[HEADER_LENGTH + segments..],
        });
        rest = &rest[total..];
    }
    Ok(out)
}

/// A parsed template.
#[derive(Debug, Clone)]
pub(crate) struct OpusTemplate {
    head_page: Vec<u8>,
    serial: u32,
    /// The audio pages, unchanged.
    audio: Vec<u8>,
    /// The sequence number of the first audio page in `audio`.
    first_audio_sequence: u32,
}

impl OpusTemplate {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let pages = pages(bytes)?;
        let Some(head) = pages.first() else {
            bail!("the template has no pages");
        };
        if head.flags & BEGINNING == 0 || !head.body.starts_with(b"OpusHead") {
            bail!("the template does not start with an OpusHead page");
        }
        if pages.iter().any(|page| page.serial != head.serial) {
            bail!("the template has more than one logical stream");
        }
        // The OpusTags packet: from the second page to the first page whose
        // last lacing value is below 255.
        let mut index = 1;
        loop {
            let Some(page) = pages.get(index) else {
                bail!("the template has no complete OpusTags packet");
            };
            if index == 1 && !page.body.starts_with(b"OpusTags") {
                bail!("the template's second page is not OpusTags");
            }
            index += 1;
            if page.lacing.last().is_some_and(|&value| value < 255) {
                break;
            }
        }
        let first_audio_sequence = pages.get(index).map_or(index as u32, |page| page.sequence);
        let audio = pages[index..]
            .iter()
            .flat_map(|page| page.raw.iter().copied())
            .collect();
        Ok(Self {
            head_page: head.raw.to_vec(),
            serial: head.serial,
            audio,
            first_audio_sequence,
        })
    }

    /// The file with `comment` (see [`super::vorbis::comment`]) as its tags.
    pub(crate) fn file(&self, comment: &[u8]) -> Result<Vec<u8>> {
        let mut packet = Vec::with_capacity(8 + comment.len());
        packet.extend_from_slice(b"OpusTags");
        packet.extend_from_slice(comment);

        let mut out =
            Vec::with_capacity(self.head_page.len() + packet.len() + 64 + self.audio.len());
        out.extend_from_slice(&self.head_page);
        let next_sequence = write_packet(&mut out, &packet, self.serial, 1);
        if next_sequence == self.first_audio_sequence {
            out.extend_from_slice(&self.audio);
        } else {
            for page in pages(&self.audio)? {
                let start = out.len();
                out.extend_from_slice(page.raw);
                let sequence = page.sequence - self.first_audio_sequence + next_sequence;
                out[start + 18..start + 22].copy_from_slice(&sequence.to_le_bytes());
                set_crc(&mut out[start..]);
            }
        }
        Ok(out)
    }
}

/// Writes `packet` as one or more header pages starting at `sequence`, and
/// returns the sequence number of the next page.
fn write_packet(out: &mut Vec<u8>, packet: &[u8], serial: u32, mut sequence: u32) -> u32 {
    // Lacing values: 255 for each full segment, then the remainder (which is 0
    // when the length is a multiple of 255).
    let mut lacing = vec![255u8; packet.len() / 255];
    lacing.push((packet.len() % 255) as u8);
    let mut body_start = 0;
    for (index, chunk) in lacing.chunks(255).enumerate() {
        let ends_packet = chunk.last().is_some_and(|&value| value < 255);
        let body_length: usize = chunk.iter().map(|&value| usize::from(value)).sum();
        let start = out.len();
        out.extend_from_slice(b"OggS");
        out.push(0);
        out.push(if index > 0 { CONTINUED } else { 0 });
        // Header pages have granule position 0; a page on which no packet
        // ends has -1.
        let granule: u64 = if ends_packet { 0 } else { u64::MAX };
        out.extend_from_slice(&granule.to_le_bytes());
        out.extend_from_slice(&serial.to_le_bytes());
        out.extend_from_slice(&sequence.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.push(chunk.len() as u8);
        out.extend_from_slice(chunk);
        out.extend_from_slice(&packet[body_start..body_start + body_length]);
        body_start += body_length;
        set_crc(&mut out[start..]);
        sequence += 1;
    }
    sequence
}

/// Computes and stores the CRC of the page at the start of `page`.
fn set_crc(page: &mut [u8]) {
    let segments = usize::from(page[26]);
    let body: usize = page[HEADER_LENGTH..HEADER_LENGTH + segments]
        .iter()
        .map(|&value| usize::from(value))
        .sum();
    let page = &mut page[..HEADER_LENGTH + segments + body];
    page[22..26].fill(0);
    let crc = crc(page);
    page[22..26].copy_from_slice(&crc.to_le_bytes());
}

fn crc_of_page(raw: &[u8]) -> u32 {
    let mut copy = raw.to_vec();
    copy[22..26].fill(0);
    crc(&copy)
}

/// The Ogg CRC-32: polynomial 0x04c11db7, initial value 0, not reflected, no
/// final XOR.
fn crc(bytes: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (index, entry) in table.iter_mut().enumerate() {
            let mut value = (index as u32) << 24;
            for _ in 0..8 {
                value = if value & 0x8000_0000 != 0 {
                    (value << 1) ^ 0x04c1_1db7
                } else {
                    value << 1
                };
            }
            *entry = value;
        }
        table
    });
    bytes.iter().fold(0u32, |crc, &byte| {
        (crc << 8) ^ table[((crc >> 24) as u8 ^ byte) as usize]
    })
}

#[cfg(test)]
mod tests {
    use super::super::vorbis;
    use super::*;

    /// The first page of a one-second silent Opus file written by FFmpeg.
    const HEAD_PAGE: [u8; 47] = [
        0x4f, 0x67, 0x67, 0x53, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1c, 0xd5, 0xc5, 0xf7, 0x01, 0x13, 0x4f, 0x70,
        0x75, 0x73, 0x48, 0x65, 0x61, 0x64, 0x01, 0x02, 0x38, 0x01, 0x80, 0xbb, 0x00, 0x00, 0x00,
        0x00, 0x00,
    ];

    #[test]
    fn crc_matches_a_real_page() {
        assert_eq!(crc_of_page(&HEAD_PAGE), 0xf7c5_d51c);
        let mut copy = HEAD_PAGE;
        copy[22..26].fill(0xaa);
        set_crc(&mut copy);
        assert_eq!(copy, HEAD_PAGE);
    }

    /// A template: the real head page, a short OpusTags page and two fake
    /// audio pages.
    fn sample() -> Vec<u8> {
        let mut bytes = HEAD_PAGE.to_vec();
        let tags = [b"OpusTags".as_slice(), &vorbis::comment("x", &[]).unwrap()].concat();
        write_packet(&mut bytes, &tags, 0, 1);
        for (sequence, granule) in [(2u32, 960u64), (3, 48_312)] {
            let start = bytes.len();
            bytes.extend_from_slice(b"OggS\0");
            bytes.push(if sequence == 3 { 0x04 } else { 0 });
            bytes.extend_from_slice(&granule.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
            bytes.extend_from_slice(&sequence.to_le_bytes());
            bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&[2, 3, 3, 0xf8, 0xff, 0xfe, 0xf8, 0xff, 0xfe]);
            set_crc(&mut bytes[start..]);
        }
        bytes
    }

    fn comment_of(file: &[u8]) -> Vec<u8> {
        let pages = pages(file).unwrap();
        let mut packet = Vec::new();
        for page in &pages[1..] {
            packet.extend_from_slice(page.body);
            if page.lacing.last().is_some_and(|&value| value < 255) {
                break;
            }
        }
        packet.strip_prefix(b"OpusTags").unwrap().to_vec()
    }

    #[test]
    fn short_tags_keep_the_audio_pages() {
        let template = OpusTemplate::parse(&sample()).unwrap();
        let comment = vorbis::comment(vorbis::VENDOR, &[("TITLE", String::from("夜"))]).unwrap();
        let file = template.file(&comment).unwrap();
        assert!(file.ends_with(&template.audio));
        assert_eq!(comment_of(&file), comment);
        let sequences: Vec<u32> = pages(&file).unwrap().iter().map(|p| p.sequence).collect();
        assert_eq!(sequences, [0, 1, 2, 3]);
    }

    #[test]
    fn long_tags_span_pages_and_renumber_the_audio() {
        let template = OpusTemplate::parse(&sample()).unwrap();
        // 255 × 255 bytes do not fit on one page.
        let long = "ä".repeat(40_000);
        let comment = vorbis::comment(vorbis::VENDOR, &[("LYRICS", long)]).unwrap();
        let file = template.file(&comment).unwrap();
        let parsed = pages(&file).unwrap();
        let sequences: Vec<u32> = parsed.iter().map(|p| p.sequence).collect();
        assert_eq!(sequences, [0, 1, 2, 3, 4]);
        assert_eq!(parsed[2].flags & CONTINUED, CONTINUED);
        assert_eq!(comment_of(&file), comment);
        // The renumbered audio pages keep their bodies.
        assert_eq!(parsed[4].body, [0xf8, 0xff, 0xfe, 0xf8, 0xff, 0xfe]);
    }

    #[test]
    fn packet_of_exactly_255_bytes_gets_a_zero_lacing_value() {
        let mut out = Vec::new();
        let next = write_packet(&mut out, &[7; 255], 9, 1);
        assert_eq!(next, 2);
        let parsed = pages(&out).unwrap();
        assert_eq!(parsed[0].lacing, [255, 0]);
    }

    #[test]
    fn rejects_a_wrong_crc() {
        let mut bytes = sample();
        bytes[30] ^= 1;
        assert!(OpusTemplate::parse(&bytes).is_err());
    }
}
