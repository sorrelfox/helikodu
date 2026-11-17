// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Seeking lands on the exact frame.

use std::path::Path;

use helikodu_ffmpeg_bridge::{AudioDecoder, MediaFile, StreamChoice};
use helikodu_model::audio::MediaTime;

use crate::media::{self, Source};

fn open(path: &Path) -> AudioDecoder {
    MediaFile::open(path)
        .and_then(|m| m.into_decoder(StreamChoice::Auto))
        .expect("open")
}

/// Reads `frames` frames from the decoder's current position.
fn read_frames(decoder: &mut AudioDecoder, frames: usize) -> (u64, Vec<f64>) {
    let channels = usize::from(decoder.spec().channel_count());
    let mut out = Vec::new();
    let mut ints = Vec::new();
    let mut first = None;
    while out.len() < frames * channels {
        let Some(block) = decoder.read().expect("decode") else {
            break;
        };
        first.get_or_insert(block.position());
        media::append(block.samples(), &mut out, &mut ints);
    }
    out.truncate(frames * channels);
    (first.unwrap_or(u64::MAX), out)
}

/// How the audio after a seek is compared with a straight decode.
#[derive(Clone, Copy)]
enum Tolerance {
    /// Sample for sample.
    Exact,
    /// Lossy decoders need a while to converge after a flush, and some (AAC with
    /// noise substitution) never give the same samples twice, so only the alignment
    /// is checked: the best cross-correlation lag must be 0.
    Aligned,
}

/// The shift of `got` against `full` (from `start`) with the highest correlation on
/// channel 0, within ±`range` frames.
fn best_lag(got: &[f64], full: &[f64], start: usize, channels: usize, range: i64) -> i64 {
    let mut best = (f64::MIN, i64::MAX);
    for lag in -range..=range {
        let mut sum = 0.0;
        for (i, g) in got.iter().step_by(channels).enumerate() {
            let j = start as i64 + (i as i64 + lag) * channels as i64;
            if let Some(f) = usize::try_from(j).ok().and_then(|j| full.get(j)) {
                sum += g * f;
            }
        }
        if sum > best.0 {
            best = (sum, lag);
        }
    }
    best.1
}

/// Seeks to several targets and compares what follows with a straight decode.
fn check_seeks(path: &Path, tolerance: Tolerance) {
    let full = media::decode(path);
    let channels = usize::from(full.spec.channel_count());
    let total = full.frames() as u64;
    let rate = full.spec.rate;
    let mut decoder = open(path);
    // Out of order on purpose: backwards and forwards.
    for target in [
        100_000,
        0,
        1,
        12_345,
        total - 10,
        50_000,
        4_095,
        200_001.min(total - 1),
    ] {
        decoder
            .seek(MediaTime::from_frames(target, rate))
            .expect("seek");
        assert_eq!(decoder.position(), target);
        let (position, got) = read_frames(&mut decoder, 2048);
        assert_eq!(position, target, "first block after seeking to {target}");
        let start = target as usize * channels;
        let want = &full.samples[start..(start + got.len()).min(full.samples.len())];
        assert_eq!(got.len(), want.len(), "length after seeking to {target}");
        match tolerance {
            Tolerance::Exact => assert!(
                got == want,
                "seek to {target} of {}: samples differ",
                path.display()
            ),
            // A few frames at the very end are too short to correlate.
            Tolerance::Aligned if got.len() < 1024 * channels => {}
            Tolerance::Aligned => {
                let lag = best_lag(&got, &full.samples, start, channels, 64);
                assert_eq!(lag, 0, "seek to {target} of {}: misaligned", path.display());
            }
        }
    }
    // Past the end: the stream ends.
    decoder
        .seek(MediaTime::from_frames(total + 1000, rate))
        .expect("seek past the end");
    let mut rest = 0;
    while let Some(block) = decoder.read().expect("decode") {
        rest += block.frames();
    }
    assert_eq!(rest, 0, "nothing after the end");
    // Milliseconds work as well as frames.
    decoder.seek(MediaTime::from_millis(1000)).expect("seek");
    let (position, _) = read_frames(&mut decoder, 1);
    assert_eq!(position, u64::from(rate.hz()));
}

#[test]
fn flac_seeks_exactly() {
    if let Some(p) = media::ffmpeg(
        "seek.flac",
        Source::S44k16,
        &["flac"],
        &["-c:a", "flac"],
        "flac",
    ) {
        check_seeks(&p, Tolerance::Exact);
    }
}

#[test]
fn wav_seeks_exactly() {
    check_seeks(&media::source_wav(Source::S48k24), Tolerance::Exact);
}

#[test]
fn mp3_seeks_exactly() {
    if let Some(p) = media::encode(
        "seek_lame_v2.mp3",
        Source::S44k16,
        "lame",
        &[],
        &["--quiet", "-V2", "{in}", "{out}"],
    ) {
        check_seeks(&p, Tolerance::Aligned);
    }
}

#[test]
fn opus_seeks_exactly() {
    if let Some(p) = media::ffmpeg(
        "seek.opus",
        Source::S48k16,
        &["libopus"],
        &["-c:a", "libopus"],
        "ogg",
    ) {
        check_seeks(&p, Tolerance::Aligned);
    }
}

#[test]
fn aac_in_mp4_seeks_exactly() {
    if let Some(p) = media::ffmpeg(
        "seek.m4a",
        Source::S44k16,
        &["aac"],
        &["-c:a", "aac"],
        "ipod",
    ) {
        check_seeks(&p, Tolerance::Aligned);
    }
}
