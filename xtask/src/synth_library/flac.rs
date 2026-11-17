// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! FLAC files from a template: the template's `STREAMINFO` (and any other
//! block that is not a comment, padding or picture) and its audio frames,
//! with a new `VORBIS_COMMENT` block as the last metadata block.
//!
//! A metadata block starts with one byte (bit 7: last block; bits 0–6: the
//! type) and a 24-bit big-endian length.

use anyhow::{Result, bail};

const STREAMINFO: u8 = 0;
const PADDING: u8 = 1;
const VORBIS_COMMENT: u8 = 4;
const PICTURE: u8 = 6;
const LAST: u8 = 0x80;

/// A parsed template.
#[derive(Debug, Clone)]
pub(crate) struct FlacTemplate {
    /// The kept metadata blocks, headers included, with the last-block flag
    /// cleared.
    blocks: Vec<u8>,
    frames: Vec<u8>,
}

impl FlacTemplate {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let Some(mut rest) = bytes.strip_prefix(b"fLaC") else {
            bail!("the template is not a FLAC file");
        };
        let mut blocks = Vec::new();
        let mut first = true;
        loop {
            if rest.len() < 4 {
                bail!("the template's metadata is truncated");
            }
            let kind = rest[0] & 0x7f;
            let last = rest[0] & LAST != 0;
            let length = u32::from_be_bytes([0, rest[1], rest[2], rest[3]]) as usize;
            if rest.len() < 4 + length {
                bail!("a metadata block of the template is truncated");
            }
            if first && kind != STREAMINFO {
                bail!("the template does not start with STREAMINFO");
            }
            first = false;
            if !matches!(kind, PADDING | VORBIS_COMMENT | PICTURE) {
                blocks.push(kind);
                blocks.extend_from_slice(&rest[1..4 + length]);
            }
            rest = &rest[4 + length..];
            if last {
                break;
            }
        }
        if rest.len() < 2 || rest[0] != 0xff || rest[1] & 0xfc != 0xf8 {
            bail!("the template's audio does not start with a frame");
        }
        Ok(Self {
            blocks,
            frames: rest.to_vec(),
        })
    }

    /// The file with `comment` (see [`super::vorbis::comment`]) as its tags.
    pub(crate) fn file(&self, comment: &[u8]) -> Result<Vec<u8>> {
        let Ok(length) = u32::try_from(comment.len()) else {
            bail!("the tags are too long");
        };
        if length >= 1 << 24 {
            bail!("the tags are longer than a FLAC metadata block can be");
        }
        let mut out =
            Vec::with_capacity(4 + self.blocks.len() + 4 + comment.len() + self.frames.len());
        out.extend_from_slice(b"fLaC");
        out.extend_from_slice(&self.blocks);
        out.push(LAST | VORBIS_COMMENT);
        out.extend_from_slice(&length.to_be_bytes()[1..]);
        out.extend_from_slice(comment);
        out.extend_from_slice(&self.frames);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::super::vorbis;
    use super::*;

    /// A FLAC file in the shape FFmpeg writes: STREAMINFO, a comment, then
    /// padding marked last, and a (fake) frame.
    fn sample() -> Vec<u8> {
        let mut bytes = b"fLaC".to_vec();
        bytes.extend_from_slice(&[STREAMINFO, 0, 0, 34]);
        bytes.extend_from_slice(&[0x11; 34]);
        bytes.extend_from_slice(&[VORBIS_COMMENT, 0, 0, 2, 9, 9]);
        bytes.extend_from_slice(&[LAST | PADDING, 0, 0, 3, 0, 0, 0]);
        bytes.extend_from_slice(&[0xff, 0xf8, 0x69, 0x08, 0x00]);
        bytes
    }

    #[test]
    fn replaces_comment_and_drops_padding() {
        let template = FlacTemplate::parse(&sample()).unwrap();
        let comment = vorbis::comment("v", &[("TITLE", String::from("Öö"))]).unwrap();
        let file = template.file(&comment).unwrap();

        let mut expected = b"fLaC".to_vec();
        expected.extend_from_slice(&[STREAMINFO, 0, 0, 34]);
        expected.extend_from_slice(&[0x11; 34]);
        expected.push(LAST | VORBIS_COMMENT);
        expected.extend_from_slice(&(comment.len() as u32).to_be_bytes()[1..]);
        expected.extend_from_slice(&comment);
        expected.extend_from_slice(&[0xff, 0xf8, 0x69, 0x08, 0x00]);
        assert_eq!(file, expected);

        // The result is itself a valid template, with the same frames.
        let again = FlacTemplate::parse(&file).unwrap();
        assert_eq!(again.frames, template.frames);
        assert_eq!(again.blocks, template.blocks);
    }

    #[test]
    fn rejects_other_files() {
        assert!(FlacTemplate::parse(b"OggS....").is_err());
        let mut truncated = sample();
        truncated.truncate(20);
        assert!(FlacTemplate::parse(&truncated).is_err());
    }
}
