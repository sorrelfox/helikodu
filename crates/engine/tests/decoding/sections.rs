// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Spans of a file (CUE-style tracks) are cut to the exact frame.

use std::path::Path;

use helikodu_model::audio::MediaTime;
use helikodu_model::library::{MediaSource, SourceSection};

use crate::media::{self, FRAMES_44K, Source};

/// CUE frames (1/75 s) to frames at 44.1 kHz.
const FRAMES_PER_CUE_FRAME: usize = 588;

fn span(path: &Path, start: MediaTime, end: Option<MediaTime>) -> MediaSource {
    MediaSource::file(path).with_section(SourceSection::Span { start, end })
}

/// Checks that the span `start .. end` (CUE frames) of the lossless file at `path`
/// decodes to exactly those samples of the reference.
fn check_lossless_span(path: &Path, start: u64, end: Option<u64>) {
    let reference = media::reference(Source::S44k);
    let first = start as usize * FRAMES_PER_CUE_FRAME;
    let last = end.map_or(FRAMES_44K, |e| e as usize * FRAMES_PER_CUE_FRAME);
    let d = media::decode(&span(
        path,
        MediaTime::from_cue_frames(start),
        end.map(MediaTime::from_cue_frames),
    ));
    assert_eq!(d.frames(), last - first, "{}: length", path.display());
    assert!(
        d.ints == reference[first * 2..last * 2],
        "{}: samples of {start}..{end:?} differ",
        path.display()
    );
    assert_eq!(
        d.decoder.duration(),
        Some(MediaTime::from_frames((last - first) as u64, d.spec.rate))
    );
}

#[test]
fn wav_spans_are_sample_exact() {
    let path = media::source_wav(Source::S44k);
    // 0:01:13 to 0:03:02 in CUE notation (minutes:seconds:frames).
    check_lossless_span(&path, 75 + 13, Some(3 * 75 + 2));
    check_lossless_span(&path, 0, Some(1));
    check_lossless_span(&path, 4 * 75 + 74, None);
}

#[test]
fn flac_spans_are_sample_exact() {
    let Some(path) = media::ffmpeg("span.flac", Source::S44k, "flac", &["-c:a", "flac"], "flac")
    else {
        return;
    };
    check_lossless_span(&path, 75 + 13, Some(3 * 75 + 2));
    check_lossless_span(&path, 2 * 75 + 40, None);
}

/// Consecutive spans of one file join without a gap or an overlap.
#[test]
fn consecutive_spans_join_exactly() {
    let Some(path) = media::ffmpeg("join.flac", Source::S44k, "flac", &["-c:a", "flac"], "flac")
    else {
        return;
    };
    let cuts = [0, 77, 151, 299];
    let mut joined = Vec::new();
    for (i, &start) in cuts.iter().enumerate() {
        let end = cuts.get(i + 1).map(|&e| MediaTime::from_cue_frames(e));
        let d = media::decode(&span(&path, MediaTime::from_cue_frames(start), end));
        joined.extend(d.ints);
    }
    assert!(
        joined == media::reference(Source::S44k),
        "the joined spans differ"
    );
}

#[test]
fn an_end_past_the_file_is_the_end_of_the_file() {
    let path = media::source_wav(Source::S44k);
    let d = media::decode(&span(
        &path,
        MediaTime::from_millis(4_000),
        Some(MediaTime::from_millis(60_000)),
    ));
    assert_eq!(d.frames(), FRAMES_44K - 176_400);
    assert_eq!(
        d.decoder.duration(),
        Some(MediaTime::from_frames(
            (FRAMES_44K - 176_400) as u64,
            d.spec.rate
        ))
    );
}

/// Lossy spans have the exact length too.
#[test]
fn lossy_span_lengths_are_exact() {
    for (name, encoder, args, format) in [
        ("span.mp3", "libmp3lame", &["-c:a", "libmp3lame"][..], "mp3"),
        ("span.m4a", "aac", &["-c:a", "aac"][..], "ipod"),
    ] {
        let Some(path) = media::ffmpeg(name, Source::S44k, encoder, args, format) else {
            continue;
        };
        let d = media::decode(&span(
            &path,
            MediaTime::from_cue_frames(61),
            Some(MediaTime::from_cue_frames(203)),
        ));
        assert_eq!(d.frames(), (203 - 61) * FRAMES_PER_CUE_FRAME, "{name}");
    }
}
