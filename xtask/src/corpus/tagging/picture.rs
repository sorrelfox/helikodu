// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The embedded pictures: small PNG images in one colour, written here byte by
//! byte (uncompressed deflate blocks), and the FLAC `PICTURE` structure that
//! FLAC files and Vorbis comments (`METADATA_BLOCK_PICTURE`, in Base64) share.

use anyhow::{Result, bail};

use super::super::recipe::PictureSpec;

/// A PNG image of `size` × `size` pixels, its colour chosen by the picture
/// type, so that pictures of different types differ.
pub(crate) fn png(spec: &PictureSpec) -> Result<Vec<u8>> {
    if spec.size == 0 || spec.size > 64 {
        bail!("pictures are 1 to 64 pixels wide");
    }
    let colour = [
        spec.kind.wrapping_mul(53),
        spec.kind.wrapping_mul(97).wrapping_add(40),
        200u8.wrapping_sub(spec.kind.wrapping_mul(31)),
    ];
    let side = spec.size as usize;
    // Each row: filter type 0 (none), then the pixels.
    let mut row = vec![0u8];
    row.extend(colour.repeat(side));
    let raw = row.repeat(side);
    // zlib: header, one stored deflate block (at most 64·193 bytes), Adler-32.
    let length = u16::try_from(raw.len()).unwrap_or_else(|_| unreachable!("at most 64 pixels"));
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&length.to_le_bytes());
    zlib.extend_from_slice(&(!length).to_le_bytes());
    zlib.extend_from_slice(&raw);
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&spec.size.to_be_bytes());
    header.extend_from_slice(&spec.size.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]); // 8 bits, RGB
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// The CRC-32 of PNG (and zip): reflected polynomial 0xedb88320.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The body of a FLAC `PICTURE` block.
pub(crate) fn flac_picture(spec: &PictureSpec) -> Result<Vec<u8>> {
    let data = png(spec)?;
    let mime = b"image/png";
    let description = spec.description.as_deref().unwrap_or("").as_bytes();
    let mut out = Vec::with_capacity(32 + mime.len() + description.len() + data.len());
    out.extend_from_slice(&u32::from(spec.kind).to_be_bytes());
    out.extend_from_slice(&(mime.len() as u32).to_be_bytes());
    out.extend_from_slice(mime);
    out.extend_from_slice(&(description.len() as u32).to_be_bytes());
    out.extend_from_slice(description);
    out.extend_from_slice(&spec.size.to_be_bytes());
    out.extend_from_slice(&spec.size.to_be_bytes());
    out.extend_from_slice(&24u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&data);
    Ok(out)
}

/// Standard Base64 with padding.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let value = (u32::from(group[0]) << 16)
            | (u32::from(*group.get(1).unwrap_or(&0)) << 8)
            | u32::from(*group.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= group.len() {
                out.push(char::from(
                    ALPHABET[(value >> (18 - 6 * index)) as usize & 63],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_of_known_text() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn crc32_of_known_text() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn png_has_the_size() {
        let spec = PictureSpec {
            kind: 3,
            size: 16,
            description: None,
        };
        let png = png(&spec).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert_eq!(&png[16..24], &[0, 0, 0, 16, 0, 0, 0, 16]);
    }
}
