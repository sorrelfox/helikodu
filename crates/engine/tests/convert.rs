// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Converting generated audio through the decoder registry and the bridge's
//! encoders: lossless output is exact, rates and channels are converted, a
//! section of a file converts to its own length, and cancelling stops.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::f64::consts::TAU;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use helikodu_engine::convert::{
    EncodeFormat, EncodeQuality, TranscodeError, TranscodeSettings, encoder_availability, transcode,
};
use helikodu_engine::decode::{BlockSamples, DecoderRegistry, PcmBlock};
use helikodu_model::audio::MediaTime;
use helikodu_model::library::{MediaSource, SourceSection};

const RATE: u32 = 44_100;
const FRAMES: usize = 100_003;

fn dir() -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine-convert");
    fs::create_dir_all(&d).unwrap();
    d
}

/// A 16-bit stereo WAV file of two tones.
fn source_wav(name: &str) -> PathBuf {
    let path = dir().join(format!("{name}.wav"));
    let mut data = Vec::with_capacity(FRAMES * 4);
    for i in 0..FRAMES {
        let t = i as f64 / f64::from(RATE);
        for hz in [440.0, 660.0] {
            let v = (0.4 * (TAU * hz * t).sin() * 32_767.0).round() as i16;
            data.extend_from_slice(&v.to_le_bytes());
        }
    }
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    fs::write(&path, b).unwrap();
    path
}

fn settings(format: EncodeFormat, quality: EncodeQuality) -> TranscodeSettings {
    TranscodeSettings {
        format,
        quality,
        sample_rate: None,
        channels: None,
        bits: None,
    }
}

/// Decodes `path` whole: the rate, the channels and the samples as i64.
fn decode_all(path: &Path) -> (u32, u16, Vec<i64>) {
    let registry = DecoderRegistry::with_default_backends();
    let mut decoder = registry.open(&MediaSource::file(path)).unwrap();
    let spec = decoder.spec();
    let mut block = PcmBlock::new();
    let mut out = Vec::new();
    while decoder.read(&mut block).unwrap() {
        match block.samples() {
            BlockSamples::S16(v) => out.extend(v.iter().map(|&x| i64::from(x))),
            BlockSamples::S32(v) => out.extend(v.iter().map(|&x| i64::from(x) >> 16)),
            BlockSamples::F32(v) => out.extend(v.iter().map(|&x| (f64::from(x) * 32_768.0) as i64)),
            BlockSamples::U8(v) => out.extend(v.iter().map(|&x| i64::from(x))),
        }
    }
    (spec.rate.hz(), spec.layout.channel_count(), out)
}

fn run(
    source: &MediaSource,
    out: &Path,
    settings: &TranscodeSettings,
) -> Result<u64, TranscodeError> {
    let registry = DecoderRegistry::with_default_backends();
    let mut last = 0;
    let report = transcode(
        &registry,
        source,
        File::create(out).unwrap(),
        settings,
        &mut |done, _| last = done,
        &|| false,
    )?;
    assert!(last > 0, "progress was reported");
    Ok(report.frames)
}

#[test]
fn flac_from_wav_is_exact() {
    let src = source_wav("exact");
    let out = dir().join("exact.flac");
    run(
        &MediaSource::file(&src),
        &out,
        &settings(EncodeFormat::Flac, EncodeQuality::Lossless(Some(5))),
    )
    .unwrap();
    let (_, _, original) = decode_all(&src);
    let (rate, channels, converted) = decode_all(&out);
    assert_eq!((rate, channels), (RATE, 2));
    assert!(original == converted, "FLAC is not exact");
}

#[test]
fn opus_is_resampled_to_48_khz() {
    if !encoder_availability(EncodeFormat::Opus).is_available() {
        eprintln!("SKIPPED: no Opus encoder");
        return;
    }
    let out = dir().join("resampled.opus");
    run(
        &MediaSource::file(source_wav("opus")),
        &out,
        &settings(EncodeFormat::Opus, EncodeQuality::Bitrate(128)),
    )
    .unwrap();
    let (rate, channels, samples) = decode_all(&out);
    assert_eq!((rate, channels), (48_000, 2));
    let frames = samples.len() / 2;
    let expected = (FRAMES as u64 * 48_000).div_ceil(u64::from(RATE)) as usize;
    assert!(
        frames.abs_diff(expected) <= 1,
        "{frames} frames, expected {expected}"
    );
}

#[test]
fn a_mono_downmix_and_a_section() {
    let out = dir().join("section-mono.wav");
    let source = MediaSource {
        path: source_wav("section"),
        section: Some(SourceSection::Span {
            start: MediaTime::from_millis(500),
            end: Some(MediaTime::from_millis(1500)),
        }),
    };
    let mut wanted = settings(EncodeFormat::Wav, EncodeQuality::Lossless(None));
    wanted.channels = Some(1);
    run(&source, &out, &wanted).unwrap();
    let (rate, channels, samples) = decode_all(&out);
    assert_eq!((rate, channels), (RATE, 1));
    assert_eq!(samples.len(), RATE as usize);
}

#[test]
fn cancelling_stops() {
    let registry = DecoderRegistry::with_default_backends();
    let out = dir().join("cancelled.flac");
    let result = transcode(
        &registry,
        &MediaSource::file(source_wav("cancel")),
        File::create(&out).unwrap(),
        &settings(EncodeFormat::Flac, EncodeQuality::Lossless(None)),
        &mut |_, _| {},
        &|| true,
    );
    assert!(matches!(result, Err(TranscodeError::Cancelled)));
}
