// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! ID3v1 and ID3v2.3/2.4 tags, written byte by byte.
//!
//! ID3v2 field keys: a text frame ID (`TIT2`, `TPE1`, …, and `GRP1`, `MVNM`,
//! `MVIN`; several values are separated by NUL), `TXXX:<description>`, `COMM` or `COMM:<description>`,
//! `USLT`, `POPM:<email>` (values: the rating byte, and optionally the play
//! counter) and `UFID:<owner>`. Text goes in the block's charset as Latin-1
//! frames when one is set (as old taggers wrote it), otherwise in Latin-1 when
//! it fits, else UTF-8 (2.4) or UTF-16 (2.3).
//!
//! ID3v1 keys: `TITLE`, `ARTIST`, `ALBUM`, `YEAR`, `COMMENT`, `TRACK`, `GENRE`
//! (the genre's number).

use anyhow::{Context, Result, bail};

use super::super::charset;
use super::super::recipe::TagBlock;
use super::picture;
use super::split;

/// The text encodings of ID3v2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Latin1,
    Utf16,
    Utf8,
}

impl Encoding {
    fn byte(self) -> u8 {
        match self {
            Self::Latin1 => 0,
            Self::Utf16 => 1,
            Self::Utf8 => 3,
        }
    }

    fn terminator(self) -> &'static [u8] {
        match self {
            Self::Utf16 => &[0, 0],
            _ => &[0],
        }
    }
}

/// How the text of one frame is written.
struct Writer<'a> {
    version: u8,
    charset: Option<&'a str>,
}

impl Writer<'_> {
    fn encoding(&self, texts: &[&str]) -> Encoding {
        if self.charset.is_some()
            || texts
                .iter()
                .all(|t| t.chars().all(|c| u32::from(c) < 0x100))
        {
            Encoding::Latin1
        } else if self.version == 4 {
            Encoding::Utf8
        } else {
            Encoding::Utf16
        }
    }

    fn text(&self, text: &str, encoding: Encoding) -> Result<Vec<u8>> {
        Ok(match encoding {
            Encoding::Latin1 => charset::encode(text, self.charset.unwrap_or("latin1"))?,
            Encoding::Utf8 => text.as_bytes().to_vec(),
            Encoding::Utf16 => {
                let mut out = vec![0xff, 0xfe];
                for unit in text.encode_utf16() {
                    out.extend_from_slice(&unit.to_le_bytes());
                }
                out
            }
        })
    }

    fn frame(&self, id: &str, body: &[u8]) -> Result<Vec<u8>> {
        if id.len() != 4
            || !id
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            bail!("`{id}` is not an ID3v2 frame ID");
        }
        let size = u32::try_from(body.len()).context("an ID3v2 frame is too long")?;
        let mut out = Vec::with_capacity(10 + body.len());
        out.extend_from_slice(id.as_bytes());
        if self.version == 4 {
            out.extend_from_slice(&syncsafe(size)?);
        } else {
            out.extend_from_slice(&size.to_be_bytes());
        }
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(body);
        Ok(out)
    }
}

fn syncsafe(value: u32) -> Result<[u8; 4]> {
    if value >= 1 << 28 {
        bail!("an ID3v2 size is too large");
    }
    Ok([
        (value >> 21) as u8 & 0x7f,
        (value >> 14) as u8 & 0x7f,
        (value >> 7) as u8 & 0x7f,
        value as u8 & 0x7f,
    ])
}

/// An ID3v2 tag of `version` 3 or 4.
pub(crate) fn v2(block: &TagBlock, version: u8) -> Result<Vec<u8>> {
    let writer = Writer {
        version,
        charset: block.charset.as_deref(),
    };
    let mut frames = Vec::new();
    for field in &block.fields {
        let (key, values) = split(field)?;
        let joined = values.join("\0");
        let (id, argument) = match key.split_once(':') {
            Some((id, argument)) => (id, Some(argument)),
            None => (key, None),
        };
        let body = match (id, argument) {
            ("POPM", Some(email)) => {
                let rating: u8 = values[0].parse().context("a POPM rating is 0 to 255")?;
                let mut body = email.as_bytes().to_vec();
                body.push(0);
                body.push(rating);
                if let Some(count) = values.get(1) {
                    let count: u32 = count.parse().context("a POPM play count is a number")?;
                    body.extend_from_slice(&count.to_be_bytes());
                }
                body
            }
            ("UFID", Some(owner)) => {
                let mut body = owner.as_bytes().to_vec();
                body.push(0);
                body.extend_from_slice(joined.as_bytes());
                body
            }
            ("TXXX", Some(description)) => {
                let encoding = writer.encoding(&[description, &joined]);
                let mut body = vec![encoding.byte()];
                body.extend(writer.text(description, encoding)?);
                body.extend_from_slice(encoding.terminator());
                body.extend(writer.text(&joined, encoding)?);
                body
            }
            ("COMM" | "USLT", description) => {
                let description = description.unwrap_or("");
                let encoding = writer.encoding(&[description, &joined]);
                let mut body = vec![encoding.byte()];
                body.extend_from_slice(b"eng");
                body.extend(writer.text(description, encoding)?);
                body.extend_from_slice(encoding.terminator());
                body.extend(writer.text(&joined, encoding)?);
                body
            }
            // Text frames, and the iTunes frames for grouping and movements.
            (id, None)
                if (id.starts_with('T') && id != "TXXX")
                    || matches!(id, "GRP1" | "MVNM" | "MVIN") =>
            {
                let encoding = writer.encoding(&[&joined]);
                let mut body = vec![encoding.byte()];
                body.extend(writer.text(&joined, encoding)?);
                body
            }
            _ => bail!("`{key}` is not an ID3v2 field this step writes"),
        };
        frames.extend(writer.frame(id, &body)?);
    }
    for spec in &block.pictures {
        let description = spec.description.as_deref().unwrap_or("");
        let encoding = writer.encoding(&[description]);
        let mut body = vec![encoding.byte()];
        body.extend_from_slice(b"image/png\0");
        body.push(spec.kind);
        body.extend(writer.text(description, encoding)?);
        body.extend_from_slice(encoding.terminator());
        body.extend(picture::png(spec)?);
        frames.extend(writer.frame("APIC", &body)?);
    }
    let size = u32::try_from(frames.len()).context("the ID3v2 tag is too long")?;
    let mut out = Vec::with_capacity(10 + frames.len());
    out.extend_from_slice(b"ID3");
    out.extend_from_slice(&[version, 0, 0]);
    out.extend_from_slice(&syncsafe(size)?);
    out.extend(frames);
    Ok(out)
}

/// An ID3v1.1 tag: 128 bytes.
pub(crate) fn v1(block: &TagBlock) -> Result<Vec<u8>> {
    if !block.pictures.is_empty() {
        bail!("ID3v1 has no pictures");
    }
    let charset = block.charset.as_deref().unwrap_or("latin1");
    let mut out = vec![0u8; 128];
    out[..3].copy_from_slice(b"TAG");
    out[127] = 255;
    for field in &block.fields {
        let (key, values) = split(field)?;
        let value = values.join(" ");
        let (offset, length) = match key.to_ascii_uppercase().as_str() {
            "TITLE" => (3, 30),
            "ARTIST" => (33, 30),
            "ALBUM" => (63, 30),
            "YEAR" => (93, 4),
            "COMMENT" => (97, 28),
            "TRACK" => {
                out[126] = value.parse().context("an ID3v1 track is 1 to 255")?;
                continue;
            }
            "GENRE" => {
                out[127] = value.parse().context("an ID3v1 genre is a number")?;
                continue;
            }
            _ => bail!("`{key}` is not an ID3v1 field"),
        };
        let bytes = charset::encode(&value, charset)?;
        if bytes.len() > length {
            bail!("`{value}` does not fit the {length} bytes of ID3v1 {key}");
        }
        out[offset..offset + bytes.len()].copy_from_slice(&bytes);
    }
    Ok(out)
}

/// The length of the ID3v2 tag at the start of `bytes`, footer included, or 0.
pub(crate) fn v2_length(bytes: &[u8]) -> usize {
    if bytes.len() < 10 || &bytes[..3] != b"ID3" {
        return 0;
    }
    let size = bytes[6..10]
        .iter()
        .fold(0usize, |size, &byte| (size << 7) | usize::from(byte & 0x7f));
    let footer = if bytes[5] & 0x10 != 0 { 10 } else { 0 };
    (10 + size + footer).min(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(fields: &[&[&str]], charset: Option<&str>) -> TagBlock {
        TagBlock {
            format: String::from("id3v23"),
            charset: charset.map(str::to_owned),
            fields: fields
                .iter()
                .map(|f| f.iter().map(|s| (*s).to_owned()).collect())
                .collect(),
            pictures: Vec::new(),
        }
    }

    #[test]
    fn legacy_text_frames() {
        let tag = v2(&block(&[&["TPE1", "Кино"]], Some("windows-1251")), 3).unwrap();
        assert_eq!(
            tag,
            b"ID3\x03\x00\x00\x00\x00\x00\x0fTPE1\x00\x00\x00\x05\x00\x00\x00\xca\xe8\xed\xee"
        );
        assert_eq!(v2_length(&tag), tag.len());
    }

    #[test]
    fn id3v1_layout() {
        let tag = v1(&block(
            &[&["TITLE", "A"], &["TRACK", "7"], &["GENRE", "13"]],
            None,
        ))
        .unwrap();
        assert_eq!(tag.len(), 128);
        assert_eq!(&tag[..4], b"TAGA");
        assert_eq!((tag[125], tag[126], tag[127]), (0, 7, 13));
    }
}
