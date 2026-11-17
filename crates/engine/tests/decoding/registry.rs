// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The registry picks FFmpeg for common files, and decoded lengths are exact.

use std::fs;

use helikodu_engine::decode::{DecoderRegistry, ProbeScore};
use helikodu_model::audio::{Endianness, MediaTime, SampleFormat};

use crate::media::{self, FRAMES_44K, FRAMES_48K, Source};

#[test]
fn wav_goes_to_ffmpeg_and_is_exact() {
    let path = media::source_wav(Source::S44k);
    let candidates: Vec<_> = DecoderRegistry::default()
        .candidates(&path)
        .unwrap()
        .into_iter()
        .map(|(f, score)| (f.name(), score))
        .collect();
    assert_eq!(candidates, [("ffmpeg", ProbeScore::FALLBACK)]);

    let d = media::decode_file(&path);
    let info = d.decoder.info();
    assert_eq!(info.backend, "ffmpeg");
    assert_eq!(info.container, "wav");
    assert_eq!(info.codec, "pcm_s16le");
    assert_eq!(d.spec.format, SampleFormat::S16(Endianness::NATIVE));
    assert_eq!(d.spec.rate.hz(), 44_100);
    assert_eq!(d.frames(), FRAMES_44K);
    assert!(
        d.ints == media::reference(Source::S44k),
        "WAV samples differ"
    );
    assert_eq!(
        d.decoder.duration(),
        Some(MediaTime::from_frames(FRAMES_44K as u64, d.spec.rate))
    );
}

#[test]
fn flac_is_bit_exact_and_carries_its_tags() {
    let Some(path) = media::ffmpeg(
        "tagged.flac",
        Source::S44k,
        "flac",
        &["-c:a", "flac", "-metadata", "title=Reference"],
        "flac",
    ) else {
        return;
    };
    let d = media::decode_file(&path);
    let info = d.decoder.info();
    assert_eq!((info.backend, info.codec.as_str()), ("ffmpeg", "flac"));
    assert!(
        info.tags
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("title") && v == "Reference"),
        "{:?}",
        info.tags
    );
    assert_eq!(d.frames(), FRAMES_44K);
    assert!(
        d.ints == media::reference(Source::S44k),
        "FLAC samples differ"
    );
}

/// Lossy formats with gapless information decode to exactly the source length.
#[test]
fn lossy_lengths_are_exact() {
    let cases: [(&str, Source, &str, &[&str], &str); 4] = [
        (
            "lame.mp3",
            Source::S44k,
            "libmp3lame",
            &["-c:a", "libmp3lame", "-b:a", "192k"],
            "mp3",
        ),
        (
            "opus.ogg",
            Source::S48k,
            "libopus",
            &["-c:a", "libopus", "-b:a", "128k"],
            "ogg",
        ),
        (
            "vorbis.ogg",
            Source::S44k,
            "libvorbis",
            &["-c:a", "libvorbis", "-q:a", "5"],
            "ogg",
        ),
        (
            "aac.m4a",
            Source::S44k,
            "aac",
            &["-c:a", "aac", "-b:a", "192k"],
            "ipod",
        ),
    ];
    for (name, src, encoder, args, format) in cases {
        let Some(path) = media::ffmpeg(name, src, encoder, args, format) else {
            continue;
        };
        let d = media::decode_file(&path);
        assert_eq!(d.decoder.info().backend, "ffmpeg", "{name}");
        let expected = match src {
            Source::S44k => FRAMES_44K,
            Source::S48k => FRAMES_48K,
        };
        assert_eq!(d.frames(), expected, "{name}: length");
        assert_eq!(d.spec.rate.hz(), src.rate(), "{name}: rate");
    }
}

/// The name is no reason to pick a backend: FLAC content named `.dsf` still goes
/// to FFmpeg.
#[test]
fn misnamed_files_are_recognised_by_content() {
    let Some(flac) = media::ffmpeg(
        "plain.flac",
        Source::S44k,
        "flac",
        &["-c:a", "flac"],
        "flac",
    ) else {
        return;
    };
    let misnamed = media::dir().join("really-flac.dsf");
    fs::copy(&flac, &misnamed).unwrap();
    let d = media::decode_file(&misnamed);
    assert_eq!(d.decoder.info().backend, "ffmpeg");
    assert_eq!(d.frames(), FRAMES_44K);
}
