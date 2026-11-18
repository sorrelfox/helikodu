// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Starts the FFmpeg bridge's encoder with arbitrary settings (format, rate,
//! channels, bits and quality from the first bytes) and feeds it arbitrary samples
//! (NaN, infinities and values far out of range included) in pieces of arbitrary
//! length, not always whole frames, then finishes the file. Any crash, sanitizer
//! report or panic is a bug: settings the encoder cannot take must be refused with
//! an error.

#![no_main]

use helikodu_ffmpeg_bridge::{AudioEncoder, EncodeFormat, EncodeQuality, EncoderConfig, LogLevel};
use libfuzzer_sys::fuzz_target;

const RATES: [u32; 8] = [0, 8_000, 22_050, 44_100, 48_000, 96_000, 384_000, u32::MAX];

fn output() -> Option<std::fs::File> {
    let path = std::env::temp_dir().join(format!("helikodu-fuzz-enc-{}", std::process::id()));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .ok()?;
    let _ = std::fs::remove_file(&path);
    Some(file)
}

fuzz_target!(|data: &[u8]| {
    helikodu_ffmpeg_bridge::set_log_level(LogLevel::Quiet);
    if data.len() < 6 {
        return;
    }
    let (head, body) = data.split_at(6);
    let format = EncodeFormat::ALL[usize::from(head[0]) % EncodeFormat::ALL.len()];
    let quality = match head[4] % 3 {
        0 => EncodeQuality::Bitrate(
            u32::from(head[5]) * 4 + if head[4] & 0x80 != 0 { u32::MAX / 2 } else { 0 },
        ),
        1 => EncodeQuality::Vbr(head[5]),
        _ => EncodeQuality::Lossless((head[5] != 0xff).then_some(head[5])),
    };
    let config = EncoderConfig {
        format,
        sample_rate: RATES[usize::from(head[1]) % RATES.len()],
        channels: u16::from(head[2] % 10),
        bits: head[3],
        quality,
    };
    let Some(file) = output() else { return };
    let Ok(mut encoder) = AudioEncoder::create(file, &config) else {
        return;
    };
    let _ = encoder.encoder_name();
    // The rest is samples: the first byte of each piece says its length and kind.
    let mut rest = body;
    while let Some((&piece, tail)) = rest.split_first() {
        let words = usize::from(piece & 0x3f) + 1;
        let take = (words * 4).min(tail.len());
        let (bytes, next) = tail.split_at(take);
        rest = next;
        let result = if piece & 0x40 == 0 {
            let samples: Vec<f32> = bytes
                .chunks(4)
                .map(|c| {
                    let mut b = [0u8; 4];
                    b[..c.len()].copy_from_slice(c);
                    f32::from_le_bytes(b)
                })
                .collect();
            encoder.write_f32(&samples)
        } else {
            let samples: Vec<i32> = bytes
                .chunks(4)
                .map(|c| {
                    let mut b = [0u8; 4];
                    b[..c.len()].copy_from_slice(c);
                    i32::from_le_bytes(b)
                })
                .collect();
            encoder.write_i32(&samples)
        };
        if result.is_err() {
            return;
        }
    }
    let _ = encoder.finish();
});
