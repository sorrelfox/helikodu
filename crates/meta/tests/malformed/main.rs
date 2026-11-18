// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Every file of the malformed-media corpus (see `corpus.rs`) is read the way the
//! player reads it, and must be read or refused cleanly: no panic, no hang, and
//! within a time limit each. Media files go through the scan worker's reader (tags,
//! embedded CUE sheet, chapters, covers) and through the FFmpeg bridge (open,
//! metadata, pictures, decoding, seeking); CUE sheets, lyrics and playlists through
//! their parsers.

mod corpus;

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use corpus::{Kind, Sample};
use helikodu_ffmpeg_bridge::{MediaFile, PictureLimits, StreamChoice};
use helikodu_meta::cue::CueOptions;
use helikodu_meta::lyrics::{SidecarKind, read_sidecar};
use helikodu_meta::playlists::{ParseOptions, read_playlist};
use helikodu_meta::read_cue;
use helikodu_meta::worker::{Request, RequestOptions, read_file};
use helikodu_model::audio::MediaTime;

/// The time one file may take, in a debug build on a busy machine.
const LIMIT: Duration = Duration::from_secs(20);

/// The largest the corpus may grow, in bytes.
const MAX_CORPUS_BYTES: u64 = 5 << 20;

/// Blocks decoded after opening and after each seek.
const BLOCKS: usize = 64;

/// Decodes the file at `path`; whether it opened.
fn decode(path: &Path) -> bool {
    let Ok(media) = MediaFile::open(path) else {
        return false;
    };
    let _ = media.tags();
    let _ = media.chapters();
    let _ = media.duration();
    let _ = media.attached_pictures(&PictureLimits {
        max_bytes: 1 << 20,
        max_count: 4,
    });
    let Ok(mut decoder) = media.into_decoder(StreamChoice::Auto) else {
        return true;
    };
    let read_some = |decoder: &mut helikodu_ffmpeg_bridge::AudioDecoder| {
        for _ in 0..BLOCKS {
            if !matches!(decoder.read(), Ok(Some(_))) {
                break;
            }
        }
    };
    read_some(&mut decoder);
    for to in [
        MediaTime::from_millis(70),
        MediaTime::from_millis(u64::MAX),
        MediaTime::from_millis(0),
    ] {
        if decoder.seek(to).is_ok() {
            read_some(&mut decoder);
        }
    }
    true
}

/// Reads `sample` as the player does; whether it was read rather than refused.
fn read(sample: &Sample) -> bool {
    let path = sample.path.as_path();
    match sample.kind {
        Kind::Media => {
            let request = Request {
                id: 1,
                path: path.to_owned(),
                options: RequestOptions::default(),
            };
            let tags = read_file(&request).is_ok();
            decode(path) || tags
        }
        Kind::Cue => read_cue(path, &CueOptions::default()).is_ok(),
        Kind::Lyrics => match read_sidecar(path, SidecarKind::Lrc, None) {
            Ok(Some(lyrics)) => {
                let _ = lyrics.line_at(Duration::from_secs(1), 0);
                let _ = lyrics.next_change(Duration::ZERO, i64::MIN);
                true
            }
            _ => false,
        },
        Kind::Playlist => read_playlist(path, &ParseOptions::default()).is_ok(),
    }
}

#[test]
fn every_malformed_file_is_read_or_refused_cleanly() {
    helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
    let samples = corpus::generate();
    let total: u64 = samples
        .iter()
        .map(|s| std::fs::metadata(&s.path).map_or(0, |m| m.len()))
        .sum();
    assert!(
        total <= MAX_CORPUS_BYTES,
        "the corpus has grown to {total} bytes"
    );
    for kind in [Kind::Media, Kind::Cue, Kind::Lyrics, Kind::Playlist] {
        assert!(
            samples.iter().filter(|s| s.kind == kind).count() >= 10,
            "too few {kind:?} files"
        );
    }

    let mut failures = Vec::new();
    let (mut read_back, count) = (0, samples.len());
    for sample in samples {
        let name = sample
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (done, finished) = mpsc::channel();
        let start = Instant::now();
        let reader = thread::Builder::new()
            .name(name.clone())
            .spawn(move || {
                let _ = done.send(read(&sample));
            })
            .expect("start a reader thread");
        match finished.recv_timeout(LIMIT) {
            Ok(was_read) => {
                read_back += usize::from(was_read);
                let _ = reader.join();
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // The thread ended without saying so: it panicked.
                let _ = reader.join();
                failures.push(format!("{name}: panicked"));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                failures.push(format!("{name}: still reading after {LIMIT:?}"));
                // The thread is left behind; the test fails anyway.
                continue;
            }
        }
        let took = start.elapsed();
        if took > LIMIT {
            failures.push(format!("{name}: took {took:?}"));
        }
    }
    eprintln!("{count} files of {total} bytes: {read_back} read, the rest refused");
    // Damage that leaves most of a file intact must still let it be read.
    assert!(
        read_back > count / 4,
        "only {read_back} of {count} files were read"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
