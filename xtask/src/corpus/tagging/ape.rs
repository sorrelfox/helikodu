// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! APEv2 tags, written byte by byte: a header, the items and a footer.
//!
//! An item is its value's length and flags (32-bit little-endian), the key
//! (ASCII, NUL-terminated) and the value: UTF-8 text, several values separated
//! by NUL, or binary data (flag bit 1). Pictures are binary items named
//! `Cover Art (<type>)` holding a file name, a NUL and the image.

use anyhow::{Context, Result, bail};

use super::super::charset;
use super::super::recipe::TagBlock;
use super::picture;
use super::split;

/// The picture type names in APEv2 item keys, by ID3v2 picture type.
const PICTURE_NAMES: [&str; 21] = [
    "Other",
    "Png Icon",
    "Icon",
    "Front",
    "Back",
    "Leaflet",
    "Media",
    "Lead Artist",
    "Artist",
    "Conductor",
    "Band",
    "Composer",
    "Lyricist",
    "Recording Location",
    "During Recording",
    "During Performance",
    "Video Capture",
    "Fish",
    "Illustration",
    "Band Logotype",
    "Publisher Logotype",
];

const BINARY: u32 = 1 << 1;
const HAS_HEADER: u32 = 1 << 31;
const IS_HEADER: u32 = 1 << 29;

pub(crate) fn tag(block: &TagBlock) -> Result<Vec<u8>> {
    let charset = block.charset.as_deref().unwrap_or("utf-8");
    let mut items = Vec::new();
    let mut count = 0u32;
    for field in &block.fields {
        let (key, values) = split(field)?;
        let mut value = Vec::new();
        for (index, text) in values.iter().enumerate() {
            if index > 0 {
                value.push(0);
            }
            value.extend(charset::encode(text, charset)?);
        }
        item(&mut items, key, 0, &value)?;
        count += 1;
    }
    for spec in &block.pictures {
        let name = PICTURE_NAMES
            .get(usize::from(spec.kind))
            .context("an APEv2 picture type is 0 to 20")?;
        let mut value = spec
            .description
            .as_deref()
            .unwrap_or("cover.png")
            .as_bytes()
            .to_vec();
        value.push(0);
        value.extend(picture::png(spec)?);
        item(&mut items, &format!("Cover Art ({name})"), BINARY, &value)?;
        count += 1;
    }
    let size = u32::try_from(items.len() + 32).context("the APEv2 tag is too long")?;
    let mut out = Vec::with_capacity(items.len() + 64);
    out.extend(header(size, count, HAS_HEADER | IS_HEADER));
    out.extend(items);
    out.extend(header(size, count, HAS_HEADER));
    Ok(out)
}

fn item(out: &mut Vec<u8>, key: &str, flags: u32, value: &[u8]) -> Result<()> {
    if key.len() < 2 || !key.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        bail!("`{key}` is not an APEv2 key");
    }
    let length = u32::try_from(value.len()).context("an APEv2 item is too long")?;
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(key.as_bytes());
    out.push(0);
    out.extend_from_slice(value);
    Ok(())
}

fn header(size: u32, count: u32, flags: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(b"APETAGEX");
    out.extend_from_slice(&2000u32.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    out
}

/// The length of the APEv2 tag that ends `bytes`, header included, or 0.
pub(crate) fn length_at_end(bytes: &[u8]) -> usize {
    let Some(footer) = bytes.len().checked_sub(32).map(|start| &bytes[start..]) else {
        return 0;
    };
    if &footer[..8] != b"APETAGEX" {
        return 0;
    }
    let size = u32::from_le_bytes([footer[12], footer[13], footer[14], footer[15]]) as usize;
    let flags = u32::from_le_bytes([footer[20], footer[21], footer[22], footer[23]]);
    let header = if flags & HAS_HEADER != 0 { 32 } else { 0 };
    (size + header).min(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_measures_itself() {
        let block = TagBlock {
            format: String::from("ape"),
            charset: None,
            fields: vec![vec![String::from("Title"), String::from("A")]],
            pictures: Vec::new(),
        };
        let tag = tag(&block).unwrap();
        assert_eq!(tag.len(), 32 + 4 + 4 + 6 + 1 + 32);
        let mut file = b"audio".to_vec();
        file.extend(&tag);
        assert_eq!(length_at_end(&file), tag.len());
    }
}
