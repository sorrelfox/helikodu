// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The built-in step `tag`: writes tags into a copy of an untagged file, byte
//! by byte, so that the corpus holds what FFmpeg cannot write faithfully:
//! several values of one field, legacy charsets in ID3v1 and ID3v2 Latin-1
//! frames, `POPM` frames, APEv2 tags, several picture types and exact MP4
//! atoms.
//!
//! | Format | Files |
//! |---|---|
//! | `vorbis` | FLAC (a `VORBIS_COMMENT` block and `PICTURE` blocks), Ogg Vorbis, Opus and Speex (the comment header; pictures as `METADATA_BLOCK_PICTURE`) |
//! | `id3v23`, `id3v24` | MP3 and ADTS streams (at the start), WAV (`id3 ` chunk), AIFF (`ID3 ` chunk) |
//! | `ape`, `id3v1` | MP3, WavPack, Monkey's Audio, Musepack and TTA files (at the end; ID3v1 last) |
//! | `mp4` | MP4 and M4A files |
//! | `riff` | WAV (a `LIST` `INFO` chunk; keys are the chunk IDs such as `INAM`) |
//!
//! Tags the input already has in the same place are removed first.

mod ape;
mod id3;
mod mp4;
mod picture;

use anyhow::{Context, Result, bail};

use super::charset;
use super::recipe::TagBlock;
use crate::synth_library::{ogg, vorbis};

/// The vendor string of the Vorbis comments written here.
const VENDOR: &str = "Helikodu test corpus";

/// The kinds of file the step knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Flac,
    /// An Ogg stream with this many header packets and this comment header
    /// prefix and suffix.
    Ogg(usize, &'static [u8], &'static [u8]),
    /// MP3 or ADTS: ID3v2 at the start, APEv2 and ID3v1 at the end.
    Mpeg,
    /// WavPack, Monkey's Audio, Musepack, TTA: APEv2 and ID3v1 at the end.
    Trailing,
    Mp4,
    Wav,
    Aiff,
}

fn container(bytes: &[u8]) -> Result<Container> {
    let starts = |magic: &[u8]| bytes.starts_with(magic);
    Ok(if starts(b"fLaC") {
        Container::Flac
    } else if starts(b"OggS") {
        let segments = usize::from(*bytes.get(26).context("a truncated Ogg page")?);
        let body = bytes.get(27 + segments..).unwrap_or_default();
        if body.starts_with(b"OpusHead") {
            Container::Ogg(2, b"OpusTags", b"")
        } else if body.starts_with(b"\x01vorbis") {
            Container::Ogg(3, b"\x03vorbis", b"\x01")
        } else if body.starts_with(b"Speex   ") {
            Container::Ogg(2, b"", b"")
        } else {
            bail!("an Ogg stream of a codec this step does not tag");
        }
    } else if starts(b"wvpk")
        || starts(b"MAC ")
        || starts(b"MPCK")
        || starts(b"MP+")
        || starts(b"TTA1")
    {
        Container::Trailing
    } else if bytes.get(4..8) == Some(b"ftyp") {
        Container::Mp4
    } else if starts(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        Container::Wav
    } else if starts(b"FORM") && matches!(bytes.get(8..12), Some(b"AIFF" | b"AIFC")) {
        Container::Aiff
    } else if starts(b"ID3") || (bytes.len() > 1 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0) {
        Container::Mpeg
    } else {
        bail!("a file this step does not tag");
    })
}

/// A field's key and its values (at least one).
fn split(field: &[String]) -> Result<(&str, &[String])> {
    match field {
        [key, values @ ..] if !values.is_empty() => Ok((key.as_str(), values)),
        _ => bail!("a field is a key followed by at least one value"),
    }
}

/// `input` with the tags of `blocks`.
pub(crate) fn apply(input: &[u8], blocks: &[TagBlock]) -> Result<Vec<u8>> {
    let kind = container(input)?;
    let allowed: &[&str] = match kind {
        Container::Flac | Container::Ogg(..) => &["vorbis"],
        Container::Mpeg => &["id3v23", "id3v24", "ape", "id3v1"],
        Container::Trailing => &["ape", "id3v1"],
        Container::Mp4 => &["mp4"],
        Container::Wav => &["riff", "id3v23", "id3v24"],
        Container::Aiff => &["id3v23", "id3v24"],
    };
    for block in blocks {
        if !allowed.contains(&block.format.as_str()) {
            bail!(
                "a {kind:?} file takes {}, not `{}`",
                allowed.join(", "),
                block.format
            );
        }
    }
    let single = |name: &str| -> Result<Option<&TagBlock>> {
        let mut found = blocks.iter().filter(|b| b.format == name);
        let first = found.next();
        if found.next().is_some() {
            bail!("one `{name}` tag per file");
        }
        Ok(first)
    };
    let id3v2 = match (single("id3v23")?, single("id3v24")?) {
        (Some(_), Some(_)) => bail!("one ID3v2 tag per file"),
        (Some(block), None) => Some(id3::v2(block, 3)?),
        (None, Some(block)) => Some(id3::v2(block, 4)?),
        (None, None) => None,
    };
    match kind {
        Container::Flac => flac(input, single("vorbis")?.context("no tag")?),
        Container::Ogg(headers, prefix, suffix) => {
            let block = single("vorbis")?.context("no tag")?;
            let mut packet = prefix.to_vec();
            packet.extend(vorbis_comment(block, true)?);
            packet.extend_from_slice(suffix);
            ogg::replace_comment_packet(input, headers, &packet)
        }
        Container::Mpeg | Container::Trailing => {
            let mut audio = input;
            if kind == Container::Mpeg {
                while id3::v2_length(audio) > 0 {
                    audio = &audio[id3::v2_length(audio)..];
                }
            }
            loop {
                if audio.len() >= 128 && audio[audio.len() - 128..].starts_with(b"TAG") {
                    audio = &audio[..audio.len() - 128];
                } else if ape::length_at_end(audio) > 0 {
                    audio = &audio[..audio.len() - ape::length_at_end(audio)];
                } else {
                    break;
                }
            }
            let mut out = id3v2.unwrap_or_default();
            out.extend_from_slice(audio);
            if let Some(block) = single("ape")? {
                out.extend(ape::tag(block)?);
            }
            if let Some(block) = single("id3v1")? {
                out.extend(id3::v1(block)?);
            }
            Ok(out)
        }
        Container::Mp4 => mp4::retag(input, single("mp4")?.context("no tag")?),
        Container::Wav => {
            let mut chunks = Vec::new();
            if let Some(block) = single("riff")? {
                chunks.push((*b"LIST", riff_info(block)?));
            }
            if let Some(tag) = id3v2 {
                chunks.push((*b"id3 ", tag));
            }
            append_chunks(input, &chunks, false)
        }
        Container::Aiff => {
            let tag = id3v2.context("no tag")?;
            append_chunks(input, &[(*b"ID3 ", tag)], true)
        }
    }
}

/// A Vorbis comment structure: each value is a field; pictures go in
/// `METADATA_BLOCK_PICTURE` fields when `pictures_inline`.
fn vorbis_comment(block: &TagBlock, pictures_inline: bool) -> Result<Vec<u8>> {
    if block.charset.is_some() {
        bail!("Vorbis comments are UTF-8");
    }
    let mut fields: Vec<(&str, String)> = Vec::new();
    for field in &block.fields {
        let (key, values) = split(field)?;
        fields.extend(values.iter().map(|value| (key, value.clone())));
    }
    if pictures_inline {
        for spec in &block.pictures {
            fields.push((
                "METADATA_BLOCK_PICTURE",
                picture::base64(&picture::flac_picture(spec)?),
            ));
        }
    }
    vorbis::comment(VENDOR, &fields)
}

/// A FLAC file with `block` as its `VORBIS_COMMENT` and `PICTURE` blocks; the
/// other metadata blocks are kept, padding is dropped.
fn flac(input: &[u8], block: &TagBlock) -> Result<Vec<u8>> {
    const PADDING: u8 = 1;
    const VORBIS_COMMENT: u8 = 4;
    const PICTURE: u8 = 6;
    let mut rest = &input[4..];
    let mut blocks: Vec<(u8, Vec<u8>)> = Vec::new();
    loop {
        if rest.len() < 4 {
            bail!("the FLAC metadata is truncated");
        }
        let kind = rest[0] & 0x7f;
        let last = rest[0] & 0x80 != 0;
        let length = u32::from_be_bytes([0, rest[1], rest[2], rest[3]]) as usize;
        let body = rest.get(4..4 + length).context("a truncated FLAC block")?;
        if !matches!(kind, PADDING | VORBIS_COMMENT | PICTURE) {
            blocks.push((kind, body.to_vec()));
        }
        rest = &rest[4 + length..];
        if last {
            break;
        }
    }
    blocks.push((VORBIS_COMMENT, vorbis_comment(block, false)?));
    for spec in &block.pictures {
        blocks.push((PICTURE, picture::flac_picture(spec)?));
    }
    let mut out = b"fLaC".to_vec();
    let count = blocks.len();
    for (index, (kind, body)) in blocks.into_iter().enumerate() {
        let length = u32::try_from(body.len())
            .ok()
            .filter(|l| *l < 1 << 24)
            .context("a FLAC metadata block is too long")?;
        out.push(if index + 1 == count {
            kind | 0x80
        } else {
            kind
        });
        out.extend_from_slice(&length.to_be_bytes()[1..]);
        out.extend(body);
    }
    out.extend_from_slice(rest);
    Ok(out)
}

/// A RIFF `INFO` list: each field is a chunk with NUL-terminated text.
fn riff_info(block: &TagBlock) -> Result<Vec<u8>> {
    if !block.pictures.is_empty() {
        bail!("RIFF INFO has no pictures");
    }
    let charset = block.charset.as_deref().unwrap_or("utf-8");
    let mut out = b"INFO".to_vec();
    for field in &block.fields {
        let (key, values) = split(field)?;
        if key.len() != 4 || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
            bail!("`{key}` is not a RIFF INFO chunk ID");
        }
        let mut text = charset::encode(&values.join("; "), charset)?;
        text.push(0);
        out.extend_from_slice(key.as_bytes());
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend(&text);
        if text.len() % 2 == 1 {
            out.push(0);
        }
    }
    Ok(out)
}

/// A RIFF (little-endian) or IFF (big-endian) file with `chunks` appended and
/// its size updated.
fn append_chunks(input: &[u8], chunks: &[([u8; 4], Vec<u8>)], big_endian: bool) -> Result<Vec<u8>> {
    let mut out = input.to_vec();
    if out.len() % 2 == 1 {
        out.push(0);
    }
    let number = |value: u32| {
        if big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        }
    };
    for (id, body) in chunks {
        let length = u32::try_from(body.len()).context("a chunk is too long")?;
        out.extend_from_slice(id);
        out.extend_from_slice(&number(length));
        out.extend_from_slice(body);
        if body.len() % 2 == 1 {
            out.push(0);
        }
    }
    let size = u32::try_from(out.len() - 8).context("the file is too long")?;
    out[4..8].copy_from_slice(&number(size));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpeg_tags_go_around_the_audio() {
        let audio = [0xff, 0xfb, 0x90, 0x00, 1, 2, 3];
        let block = |format: &str| TagBlock {
            format: format.to_owned(),
            charset: None,
            fields: vec![vec![String::from("TITLE"), String::from("A")]],
            pictures: Vec::new(),
        };
        let mut v2 = block("id3v24");
        v2.fields = vec![vec![String::from("TIT2"), String::from("A")]];
        let blocks = [block("id3v1"), block("ape"), v2];
        let once = apply(&audio, &blocks).unwrap();
        assert!(once.starts_with(b"ID3\x04"));
        assert_eq!(&once[once.len() - 128..once.len() - 125], b"TAG");
        // Tagging again replaces the tags instead of adding more.
        assert_eq!(apply(&once, &blocks).unwrap(), once);
    }
}
