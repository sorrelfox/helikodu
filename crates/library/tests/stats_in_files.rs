// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Play counts and ratings written into the files in batches, with real worker
//! processes over files made with ffmpeg: only files whose values changed are
//! written, and only what is switched on.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[allow(dead_code)]
#[path = "support/scanning.rs"]
mod support;

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use helikodu_library::interchange::StatsToFiles;
use helikodu_library::{
    CancelToken, DatabaseConfig, Library, Listen, ListenEnd, PlayRules, Rating, ScanOptions,
    TagWriteSettings,
};
use helikodu_meta::tags::writing::{FieldKey, field_values};
use helikodu_meta::tags::{ReadOptions, read_tags};
use support::{flac, folder, have_ffmpeg, worker_program};

fn options(root: &Path) -> ScanOptions {
    let mut options = ScanOptions::new([root]);
    options.workers.program = Some(worker_program());
    options.workers.count = 1;
    options
}

fn in_file(path: &Path) -> (Option<u8>, Vec<String>) {
    let tags = read_tags(path, &ReadOptions::default()).unwrap().tags;
    (
        tags.rating.map(|r| r.half_stars()),
        field_values(&tags, &FieldKey::Custom("FMPS_PLAYCOUNT".into())),
    )
}

#[test]
fn changed_statistics_are_written_in_a_batch() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("stats-in-files").join("music");
    let played = root.join("played.flac");
    let rated = root.join("rated.flac");
    let untouched = root.join("untouched.flac");
    flac(&played, 440, 1, &[("title", "Played")]);
    flac(&rated, 550, 1, &[("title", "Rated")]);
    flac(&untouched, 660, 1, &[("title", "Untouched")]);
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let options = options(&root);
    library.scan(&options, None, &CancelToken::new()).unwrap();
    let track = |p: &Path| library.track_at(p, 0).unwrap().unwrap();
    for _ in 0..3 {
        library
            .record_listen(
                track(&played),
                Listen {
                    started_at: UNIX_EPOCH + Duration::from_secs(1000),
                    played: Duration::from_secs(1),
                    end: ListenEnd::Finished,
                    position: None,
                    context: None,
                },
                PlayRules::default(),
            )
            .unwrap();
    }
    library
        .set_rating(track(&rated), Rating::from_half_stars(7))
        .unwrap();
    let both = StatsToFiles {
        ratings: true,
        play_counts: true,
    };
    let counts_only = StatsToFiles {
        ratings: false,
        play_counts: true,
    };
    assert_eq!(library.pending_stats_writes(counts_only).unwrap(), 1);
    assert_eq!(library.pending_stats_writes(both).unwrap(), 2);

    let settings = TagWriteSettings::default();
    let report = library
        .write_stats_to_files(both, &settings, &options, 10, &CancelToken::new())
        .unwrap();
    assert_eq!(report.written, 2, "{report:?}");
    assert!(report.failed.is_empty());
    assert_eq!(report.remaining, 0);
    assert_eq!(in_file(&played), (None, vec!["3".to_owned()]));
    assert_eq!(in_file(&rated).0, Some(7));
    assert_eq!(in_file(&untouched), (None, Vec::new()));
    // The tracks keep their statistics through the re-read.
    assert_eq!(library.stats(track(&played)).unwrap().play_count, 3);

    // Nothing changed: nothing to write.
    assert_eq!(library.pending_stats_writes(both).unwrap(), 0);
    library
        .set_rating(track(&rated), Rating::from_half_stars(10))
        .unwrap();
    assert_eq!(library.pending_stats_writes(counts_only).unwrap(), 0);
    assert_eq!(library.pending_stats_writes(both).unwrap(), 1);
}
