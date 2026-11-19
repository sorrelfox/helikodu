// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! iTunes-style MP4 metadata: `moov.udta.meta.ilst`, written byte by byte.
//!
//! Field keys are atom names (`©nam`, `aART`, `soar`, `trkn`, …; `©` is the
//! byte 0xA9) or `----:<mean>:<name>` for freeform atoms. Several values become
//! several `data` atoms. `trkn` and `disk` take `N` or `N/M`; `cpil`, `pgap`,
//! `shwm`, `tmpo` and `©mvi` are integers. Pictures go to `covr`, which has no
//! picture types.
//!
//! The file's existing `udta` is replaced. Only `moov` changes size, so it must
//! come after the media data (as FFmpeg writes it without `+faststart`): the
//! chunk offsets then stay valid.

use anyhow::{Context, Result, bail};

use super::super::recipe::TagBlock;
use super::picture;
use super::split;

/// UTF-8 text.
const TYPE_UTF8: u32 = 1;
/// A big-endian signed integer.
const TYPE_INTEGER: u32 = 21;
const TYPE_PNG: u32 = 14;

pub(crate) fn retag(file: &[u8], block: &TagBlock) -> Result<Vec<u8>> {
    let boxes = children(file)?;
    let moov = boxes
        .iter()
        .position(|(kind, _, _)| kind == b"moov")
        .context("the file has no moov box")?;
    if boxes[moov + 1..].iter().any(|(kind, _, _)| kind == b"mdat") {
        bail!("moov comes before the media data; the chunk offsets would move");
    }
    let (_, moov_start, moov_end) = boxes[moov];
    let mut moov_body = Vec::new();
    for (kind, start, end) in children(&file[moov_start + 8..moov_end])? {
        if &kind != b"udta" {
            moov_body.extend_from_slice(&file[moov_start + 8 + start..moov_start + 8 + end]);
        }
    }
    moov_body.extend(make_box(b"udta", &meta(block)?)?);

    let mut out = file[..moov_start].to_vec();
    out.extend(make_box(b"moov", &moov_body)?);
    out.extend_from_slice(&file[moov_end..]);
    Ok(out)
}

/// The boxes in `bytes`: type, start and end.
fn children(bytes: &[u8]) -> Result<Vec<([u8; 4], usize, usize)>> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes.len() - offset < 8 {
            bail!("a truncated MP4 box");
        }
        let size = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        let kind = [
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ];
        let size = match size {
            0 => bytes.len() - offset,
            1 => bail!("64-bit MP4 box sizes are not supported here"),
            size => size,
        };
        if size < 8 || offset + size > bytes.len() {
            bail!("an MP4 box has a wrong size");
        }
        out.push((kind, offset, offset + size));
        offset += size;
    }
    Ok(out)
}

fn make_box(kind: &[u8], body: &[u8]) -> Result<Vec<u8>> {
    let size = u32::try_from(body.len() + 8).context("an MP4 box is too long")?;
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    Ok(out)
}

/// A full box: version and flags (all 0), then the body.
fn full_box(kind: &[u8], body: &[u8]) -> Result<Vec<u8>> {
    let mut content = vec![0; 4];
    content.extend_from_slice(body);
    make_box(kind, &content)
}

fn data(kind: u32, payload: &[u8]) -> Result<Vec<u8>> {
    let mut body = kind.to_be_bytes().to_vec();
    body.extend_from_slice(&[0; 4]); // locale
    body.extend_from_slice(payload);
    make_box(b"data", &body)
}

/// The atom name of a key: one byte per character, `©` as 0xA9.
fn atom_name(key: &str) -> Result<[u8; 4]> {
    let bytes: Vec<u8> = key
        .chars()
        .map(|c| u8::try_from(u32::from(c)).context("an atom name is Latin-1"))
        .collect::<Result<_>>()?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("`{key}` is not a four-character atom name"))
}

fn meta(block: &TagBlock) -> Result<Vec<u8>> {
    let mut ilst = Vec::new();
    for field in &block.fields {
        let (key, values) = split(field)?;
        if let Some(rest) = key.strip_prefix("----:") {
            let (mean, name) = rest
                .split_once(':')
                .context("a freeform key is `----:<mean>:<name>`")?;
            let mut body = full_box(b"mean", mean.as_bytes())?;
            body.extend(full_box(b"name", name.as_bytes())?);
            for value in values {
                body.extend(data(TYPE_UTF8, value.as_bytes())?);
            }
            ilst.extend(make_box(b"----", &body)?);
            continue;
        }
        let name = atom_name(key)?;
        let mut body = Vec::new();
        for value in values {
            body.extend(match &name {
                b"trkn" | b"disk" => {
                    let (number, total) = value.split_once('/').unwrap_or((value, "0"));
                    let number: u16 = number.trim().parse().context("a number")?;
                    let total: u16 = total.trim().parse().context("a total")?;
                    let mut payload = vec![0, 0];
                    payload.extend_from_slice(&number.to_be_bytes());
                    payload.extend_from_slice(&total.to_be_bytes());
                    if &name == b"trkn" {
                        payload.extend_from_slice(&[0, 0]);
                    }
                    data(0, &payload)?
                }
                b"cpil" | b"pgap" | b"shwm" => {
                    let flag: u8 = value.parse().context("a flag is 0 or 1")?;
                    data(TYPE_INTEGER, &[flag])?
                }
                b"tmpo" | b"\xa9mvi" | b"\xa9mvc" => {
                    let number: u16 = value.parse().context("a number")?;
                    data(TYPE_INTEGER, &number.to_be_bytes())?
                }
                _ => data(TYPE_UTF8, value.as_bytes())?,
            });
        }
        ilst.extend(make_box(&name, &body)?);
    }
    if !block.pictures.is_empty() {
        let mut body = Vec::new();
        for spec in &block.pictures {
            body.extend(data(TYPE_PNG, &picture::png(spec)?)?);
        }
        ilst.extend(make_box(b"covr", &body)?);
    }
    // The handler: pre_defined, `mdir`, the reserved `appl` 0 0, an empty name.
    let mut handler = vec![0; 4];
    handler.extend_from_slice(b"mdirappl");
    handler.extend_from_slice(&[0; 9]);
    let mut meta = full_box(b"hdlr", &handler)?;
    meta.extend(make_box(b"ilst", &ilst)?);
    full_box(b"meta", &meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_udta_is_replaced() {
        let mut file = make_box(b"ftyp", b"M4A \0\0\0\0").unwrap();
        file.extend(make_box(b"mdat", b"audio").unwrap());
        let mut moov = make_box(b"mvhd", b"x").unwrap();
        moov.extend(make_box(b"udta", b"old").unwrap());
        file.extend(make_box(b"moov", &moov).unwrap());
        let block = TagBlock {
            format: String::from("mp4"),
            charset: None,
            fields: vec![vec![String::from("\u{a9}nam"), String::from("T")]],
            pictures: Vec::new(),
        };
        let out = retag(&file, &block).unwrap();
        let boxes = children(&out).unwrap();
        assert_eq!(boxes.len(), 3);
        let (_, start, end) = boxes[2];
        let inner = children(&out[start + 8..end]).unwrap();
        assert_eq!(&inner[0].0, b"mvhd");
        assert_eq!(&inner[1].0, b"udta");
        assert!(!out.windows(3).any(|w| w == b"old"));
    }
}
