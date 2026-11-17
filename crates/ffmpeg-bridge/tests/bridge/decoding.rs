// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lossless decoding is exact in length and value.

use std::os::unix::ffi::OsStrExt;

use helikodu_ffmpeg_bridge::{AvErrorKind, Error, MediaFile, StreamChoice};
use helikodu_model::audio::{Endianness, SampleFormat};

use crate::media::{self, FRAMES_44K, FRAMES_48K, Source};

#[test]
fn wav_is_exact() {
    let path = media::source_wav(Source::S44k16);
    let d = media::decode(&path);
    assert_eq!(d.spec.format, SampleFormat::S16(Endianness::NATIVE));
    assert_eq!(d.spec.rate.hz(), 44_100);
    assert_eq!(d.spec.channel_count(), 2);
    assert_eq!(d.frames(), FRAMES_44K);
    let reference: Vec<i64> = media::reference_ints(Source::S44k16)
        .into_iter()
        .map(i64::from)
        .collect();
    assert!(d.ints == reference, "WAV samples differ");
}

#[test]
fn wav_24_bit_is_exact() {
    let path = media::source_wav(Source::S48k24);
    let d = media::decode(&path);
    assert_eq!(d.spec.format, SampleFormat::S32(Endianness::NATIVE));
    assert_eq!(d.frames(), FRAMES_48K);
    let reference: Vec<i64> = media::reference_ints(Source::S48k24)
        .into_iter()
        .map(|v| i64::from(v) << 8)
        .collect();
    assert!(d.ints == reference, "24-bit WAV samples differ");
}

#[test]
fn flac_16_bit_is_exact() {
    let Some(path) = media::ffmpeg(
        "flac16.flac",
        Source::S44k16,
        &["flac"],
        &["-c:a", "flac"],
        "flac",
    ) else {
        return;
    };
    let d = media::decode(&path);
    assert_eq!(d.spec.format, SampleFormat::S16(Endianness::NATIVE));
    assert_eq!(d.frames(), FRAMES_44K);
    let reference: Vec<i64> = media::reference_ints(Source::S44k16)
        .into_iter()
        .map(i64::from)
        .collect();
    assert!(d.ints == reference, "FLAC samples differ");
    let duration = d.decoder.duration().expect("FLAC states its length");
    assert_eq!(duration.frames_at(d.spec.rate), FRAMES_44K as u64);
}

#[test]
fn flac_24_bit_is_exact() {
    let Some(path) = media::ffmpeg(
        "flac24.flac",
        Source::S48k24,
        &["flac"],
        &["-c:a", "flac", "-sample_fmt", "s32"],
        "flac",
    ) else {
        return;
    };
    let d = media::decode(&path);
    assert_eq!(d.spec.format, SampleFormat::S32(Endianness::NATIVE));
    assert_eq!(d.decoder.stream().bits_per_sample, Some(24));
    assert_eq!(d.frames(), FRAMES_48K);
    let reference: Vec<i64> = media::reference_ints(Source::S48k24)
        .into_iter()
        .map(|v| i64::from(v) << 8)
        .collect();
    assert!(d.ints == reference, "24-bit FLAC samples differ");
}

#[test]
fn files_and_decoders_can_move_between_threads() {
    fn assert_send<T: Send>() {}
    assert_send::<MediaFile>();
    assert_send::<helikodu_ffmpeg_bridge::AudioDecoder>();
}

#[test]
fn an_open_file_is_read_from_its_start() {
    use std::io::Read;
    let mut file = std::fs::File::open(media::source_wav(Source::S44k16)).expect("open");
    let mut skip = [0u8; 100];
    file.read_exact(&mut skip).expect("read");
    let decoder = MediaFile::open_file(file)
        .and_then(|m| m.into_decoder(StreamChoice::Auto))
        .expect("open");
    assert_eq!(media::decode_rest(decoder).frames(), FRAMES_44K);
}

#[test]
fn file_names_need_not_be_utf8() {
    let source = media::source_wav(Source::S44k16);
    let name = std::ffi::OsStr::from_bytes(b"non-utf8-\xff\xfe-name.wav");
    let path = media::dir().join(name);
    std::fs::copy(&source, &path).expect("copy");
    let d = media::decode(&path);
    assert_eq!(d.frames(), FRAMES_44K);
}

#[test]
fn a_misnamed_file_is_probed_by_content() {
    let Some(path) = media::ffmpeg(
        "flac-named-mp3.mp3",
        Source::S44k16,
        &["flac"],
        &["-c:a", "flac"],
        "flac",
    ) else {
        return;
    };
    let file = MediaFile::open(&path).expect("open");
    assert_eq!(file.format_name(), "flac");
}

#[test]
fn a_missing_file_is_an_io_error() {
    let err = MediaFile::open(media::dir().join("does-not-exist.flac")).expect_err("missing");
    assert!(matches!(err, Error::Io(_)), "{err:?}");
}

#[test]
fn a_text_file_is_refused() {
    let path = media::dir().join("text.mp3");
    std::fs::write(&path, "just some text, not audio\n".repeat(100)).expect("write");
    let err = MediaFile::open(&path).expect_err("not media");
    match err {
        Error::Ffmpeg { error, .. } => assert!(matches!(
            error.kind(),
            AvErrorKind::InvalidData | AvErrorKind::InvalidArgument | AvErrorKind::DemuxerNotFound
        )),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_truncated_file_decodes_what_is_there() {
    let Some(path) = media::ffmpeg(
        "flac-cut-src.flac",
        Source::S44k16,
        &["flac"],
        &["-c:a", "flac"],
        "flac",
    ) else {
        return;
    };
    let bytes = std::fs::read(&path).expect("read");
    let cut = media::dir().join("flac-cut.flac");
    std::fs::write(&cut, &bytes[..bytes.len() / 2]).expect("write");
    let decoder = MediaFile::open(&cut)
        .and_then(|m| m.into_decoder(StreamChoice::Auto))
        .expect("the header is intact");
    let d = media::decode_rest(decoder);
    assert!(d.frames() > FRAMES_44K / 4 && d.frames() < FRAMES_44K);
}
