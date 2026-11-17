// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Failures map to the model's engine errors.

use helikodu_engine::decode::{DecodeError, DecoderRegistry};
use helikodu_model::audio::MediaTime;
use helikodu_model::library::{MediaSource, SourceSection};
use helikodu_model::player::EngineError;

use crate::media::{self, Source};

fn open_error(source: &MediaSource) -> DecodeError {
    DecoderRegistry::default().open(source).unwrap_err()
}

fn is_unplayable(error: &DecodeError) -> bool {
    matches!(error.to_engine_error(), EngineError::Unplayable { .. })
}

#[test]
fn a_missing_file_is_unplayable() {
    let error = open_error(&MediaSource::file(media::dir().join("missing.flac")));
    assert!(matches!(error, DecodeError::Io(_)), "{error:?}");
    assert!(is_unplayable(&error));
}

#[test]
fn a_file_no_backend_opens_is_unplayable_with_the_reasons() {
    let path = media::write("noise.mp3", &[0x5a; 10_000]);
    let error = open_error(&MediaSource::file(path));
    let DecodeError::NoDecoder { attempts } = &error else {
        panic!("{error:?}");
    };
    assert_eq!(attempts.len(), 1, "{attempts:?}");
    assert_eq!(attempts[0].0, "ffmpeg");
    assert!(is_unplayable(&error));
    let EngineError::Unplayable { reason } = EngineError::from(error) else {
        unreachable!()
    };
    assert!(
        reason.starts_with("no decoder can open the file (ffmpeg: "),
        "{reason}"
    );

    let empty = media::write("empty.flac", b"");
    assert!(is_unplayable(&open_error(&MediaSource::file(empty))));
}

#[test]
fn ffmpeg_has_no_subsongs() {
    let source =
        MediaSource::file(media::source_wav(Source::S44k)).with_section(SourceSection::Subsong(1));
    let error = open_error(&source);
    let DecodeError::NoDecoder { attempts } = &error else {
        panic!("{error:?}");
    };
    assert_eq!(
        attempts,
        &[("ffmpeg", "ffmpeg does not support subsongs".to_owned())]
    );
    assert!(is_unplayable(&error));
}

#[test]
fn sections_outside_the_file_are_unplayable() {
    let path = media::source_wav(Source::S44k);
    for (start, end) in [
        // Starts after the end (5 s file).
        (MediaTime::from_millis(6_000), None),
        // Ends before it starts.
        (
            MediaTime::from_millis(2_000),
            Some(MediaTime::from_millis(1_000)),
        ),
        // Empty.
        (
            MediaTime::from_millis(1_000),
            Some(MediaTime::from_millis(1_000)),
        ),
    ] {
        let source = MediaSource::file(&path).with_section(SourceSection::Span { start, end });
        let error = open_error(&source);
        assert!(
            matches!(error, DecodeError::InvalidSection { .. }),
            "{error:?}"
        );
        assert!(is_unplayable(&error));
    }
}
