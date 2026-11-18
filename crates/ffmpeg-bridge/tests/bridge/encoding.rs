// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The converter's encoders: every format this FFmpeg can write is encoded from
//! the reference signal and decoded again; lossless formats must be bit-exact and
//! every format must keep the length (gapless).

use std::fs::{self, File};

use helikodu_ffmpeg_bridge::{
    AudioEncoder, EncodeFormat, EncodeQuality, EncoderConfig, encoder_availability, nearest_rate,
    supported_sample_rates,
};

use crate::media::{Source, decode, dir, reference_ints};

fn encode(format: EncodeFormat, src: Source, name: &str) -> Option<std::path::PathBuf> {
    if !encoder_availability(format).is_available() {
        eprintln!("SKIPPED: no encoder for {}", format.name());
        return None;
    }
    let rates = supported_sample_rates(format);
    let rate = nearest_rate(rates.as_deref(), src.rate());
    if rate != src.rate() {
        eprintln!("SKIPPED: {} does not take {} Hz", format.name(), src.rate());
        return None;
    }
    let path = dir().join(name);
    let _ = fs::remove_file(&path);
    let file = File::create(&path).expect("create the output");
    let config = EncoderConfig {
        format,
        sample_rate: rate,
        channels: 2,
        bits: u8::try_from(src.bits()).expect("bits"),
        quality: if format.is_lossless() {
            EncodeQuality::Lossless(None)
        } else {
            EncodeQuality::Bitrate(160)
        },
    };
    let mut encoder = AudioEncoder::create(file, &config).expect("encoder");
    let shift = 32 - src.bits();
    let ints: Vec<i32> = reference_ints(src)
        .into_iter()
        .map(|v| v << shift)
        .collect();
    // Odd-sized pieces, as a decoder hands them over.
    for piece in ints.chunks(2 * 1_777) {
        encoder.write_i32(piece).expect("encode");
    }
    let size = encoder.finish().expect("finish");
    assert_eq!(size, fs::metadata(&path).expect("written").len());
    Some(path)
}

#[test]
fn lossless_formats_are_bit_exact() {
    for (format, src) in [
        (EncodeFormat::Flac, Source::S44k16),
        (EncodeFormat::Flac, Source::S48k24),
        (EncodeFormat::Wav, Source::S44k16),
        (EncodeFormat::Wav, Source::S48k24),
        (EncodeFormat::Alac, Source::S48k24),
    ] {
        let name = format!("encoded-{}-{:?}.{}", format.id(), src, format.extension());
        let Some(path) = encode(format, src, &name) else {
            continue;
        };
        let decoded = decode(&path);
        let shift = if decoded.ints.iter().any(|v| v.abs() > 1 << 24) {
            32 - src.bits()
        } else {
            0
        };
        let expected: Vec<i64> = reference_ints(src)
            .into_iter()
            .map(|v| i64::from(v) << shift)
            .collect();
        assert_eq!(decoded.frames(), src.frames(), "{name}: length");
        assert!(decoded.ints == expected, "{name}: not bit-exact");
    }
}

#[test]
fn lossy_formats_keep_the_length() {
    for format in [
        EncodeFormat::Opus,
        EncodeFormat::Mp3,
        EncodeFormat::Aac,
        EncodeFormat::Vorbis,
    ] {
        let src = if format == EncodeFormat::Opus {
            Source::S48k16
        } else {
            Source::S44k16
        };
        let name = format!("encoded-{}.{}", format.id(), format.extension());
        let Some(path) = encode(format, src, &name) else {
            continue;
        };
        let decoded = decode(&path);
        assert_eq!(decoded.spec.channel_count(), 2, "{name}: channels");
        let frames = decoded.frames() as i64;
        let want = src.frames() as i64;
        // MP3 without a LAME tag cannot carry the exact length; the others can.
        let slack = if format == EncodeFormat::Mp3 {
            2_000
        } else {
            0
        };
        assert!(
            (frames - want).abs() <= slack,
            "{name}: {frames} frames, expected {want}"
        );
        let peak = decoded.samples.iter().fold(0.0_f64, |m, x| m.max(x.abs()));
        assert!(peak > 0.1, "{name}: silent");
    }
}

#[test]
fn a_rate_the_encoder_does_not_take_is_refused() {
    if !encoder_availability(EncodeFormat::Opus).is_available() {
        return;
    }
    let path = dir().join("encoded-refused.opus");
    let config = EncoderConfig {
        format: EncodeFormat::Opus,
        sample_rate: 44_100,
        channels: 2,
        bits: 16,
        quality: EncodeQuality::Bitrate(128),
    };
    assert!(AudioEncoder::create(File::create(&path).expect("create"), &config).is_err());
}

/// Samples that are not numbers, or far beyond full scale, once made LAME abort the
/// whole process (an assertion in its psychoacoustic model); fuzzing found it.
/// Every encoder must take them.
#[test]
fn encoders_take_samples_that_are_not_numbers() {
    for format in EncodeFormat::ALL {
        if !encoder_availability(format).is_available() {
            continue;
        }
        let rate = nearest_rate(supported_sample_rates(format).as_deref(), 44_100);
        let path = dir().join(format!("encoded-wild.{}", format.extension()));
        let config = EncoderConfig {
            format,
            sample_rate: rate,
            channels: 2,
            bits: 16,
            quality: if format.is_lossless() {
                EncodeQuality::Lossless(None)
            } else {
                EncodeQuality::Bitrate(128)
            },
        };
        let mut encoder =
            AudioEncoder::create(File::create(&path).expect("create"), &config).expect("encoder");
        let wild = [
            f32::NAN,
            f32::INFINITY,
            -1.97e27,
            5.3e20,
            f32::NEG_INFINITY,
            -5.4e32,
            0.5,
            -0.5,
        ];
        let samples: Vec<f32> = wild.iter().copied().cycle().take(2 * 4_800).collect();
        encoder.write_f32(&samples).expect("encode");
        encoder.finish().expect("finish");
    }
}

/// FFmpeg's ALAC encoder aborted the process on a last frame of one or two
/// frames with four to six channels; fuzzing found it. Every length must encode.
#[test]
fn alac_takes_any_length_with_many_channels() {
    if !encoder_availability(EncodeFormat::Alac).is_available() {
        return;
    }
    for channels in [3u16, 4, 5, 6, 8] {
        for frames in [1usize, 2, 3, 4097, 4098] {
            let path = dir().join(format!("encoded-alac-{channels}-{frames}.m4a"));
            let config = EncoderConfig {
                format: EncodeFormat::Alac,
                sample_rate: 44_100,
                channels,
                bits: 16,
                quality: EncodeQuality::Lossless(None),
            };
            // Layouts the encoder does not take are refused; that is fine.
            let Ok(mut encoder) =
                AudioEncoder::create(File::create(&path).expect("create"), &config)
            else {
                assert!(
                    channels != 4 && channels != 6,
                    "{channels} channels refused"
                );
                continue;
            };
            let samples: Vec<i32> = (0..frames * usize::from(channels))
                .map(|i| ((i as f64 * 0.01).sin() * 1e9) as i32)
                .collect();
            encoder.write_i32(&samples).expect("encode");
            encoder.finish().expect("finish");
        }
    }
}
