// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rewrites the output gain field of an Ogg Opus header in place.
//!
//! The first Ogg page of an Opus stream carries exactly the `OpusHead` packet.
//! Bytes 16..18 of that packet are the output gain, a signed Q7.8 value in dB,
//! little-endian. Changing it means recomputing the page CRC.

pub fn set_opus_output_gain(data: &mut [u8], gain_q78: i16) -> Result<(), String> {
    if data.len() < 27 || &data[0..4] != b"OggS" {
        return Err("not an Ogg file".into());
    }
    let nseg = usize::from(data[26]);
    let header_len = 27 + nseg;
    let body_len: usize = data
        .get(27..header_len)
        .ok_or("truncated page header")?
        .iter()
        .map(|&b| usize::from(b))
        .sum();
    let page_len = header_len + body_len;
    if data.len() < page_len || body_len < 19 {
        return Err("truncated first page".into());
    }
    if &data[header_len..header_len + 8] != b"OpusHead" {
        return Err("first packet is not OpusHead".into());
    }
    data[header_len + 16..header_len + 18].copy_from_slice(&gain_q78.to_le_bytes());
    data[22..26].fill(0);
    let crc = crc32(&data[..page_len]);
    data[22..26].copy_from_slice(&crc.to_le_bytes());
    Ok(())
}

/// Ogg's CRC-32: polynomial 0x04C11DB7, not reflected, initial value 0.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0;
    for &b in bytes {
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}
