// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Statistics recorded through a real library in memory: plays, skips, the start
//! times from the clock, and files played by path that are in the library.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use helikodu_library::{
    AudioProperties, DatabaseConfig, FileStamp, HistoryQuery, Library, ListenOutcome, TrackRecord,
    TrackTags,
};
use helikodu_model::audio::MediaTime;
use helikodu_model::library::{TrackId, TrackRef};
use helikodu_model::player::QueueContext;
use helikodu_service::bus::Subscription;
use helikodu_service::clock::FakeClock;
use helikodu_service::library::MusicLibrary;
use helikodu_service::player::{PlayerCommand, PlayerEvent, SinkPacing};

use crate::media::{folder, next_path, null_output, player_with, until, wav_files, wav_output};

pub(crate) fn add(library: &Library, root: &Path, path: &Path, millis: u64) -> TrackId {
    let root = library.add_root(root).unwrap();
    let mut record = TrackRecord::new(
        root,
        path,
        FileStamp {
            size: 1000,
            mtime_ns: Some(1),
            inode: None,
            quick_hash: path.as_os_str().as_encoded_bytes().to_vec(),
        },
    );
    record.audio = AudioProperties {
        duration: Some(Duration::from_millis(millis)),
        sample_rate: Some(44_100),
        channels: Some(2),
        ..AudioProperties::default()
    };
    record.tags = TrackTags {
        title: Some(path.file_name().unwrap().to_string_lossy().into_owned()),
        ..TrackTags::default()
    };
    library.upsert_track(record).unwrap().track
}

fn recorded(events: &Subscription<PlayerEvent>) -> (TrackId, ListenOutcome) {
    match until(events, |e| matches!(e, PlayerEvent::ListenRecorded { .. })).0 {
        PlayerEvent::ListenRecorded { track, outcome } => (track, outcome),
        _ => unreachable!(),
    }
}

#[test]
fn plays_and_skips_are_recorded() {
    let dir = folder("stats");
    let files = wav_files(&dir, 2, 300);
    let library = Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap());
    let first = add(&library, &dir, &files[0], 300);
    let second = add(&library, &dir, &files[1], 300);
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let clock = FakeClock::new(start);
    let (player, events) = player_with(
        wav_output(&dir.join("out.wav")),
        SinkPacing::Freewheel,
        |config| {
            config.library = Some(MusicLibrary::from_library(Arc::clone(&library), ":memory:"));
            config.clock = clock.shared();
        },
    );

    // Two library tracks played to their end are two plays.
    player
        .send(PlayerCommand::PlayTracks {
            context: QueueContext::files(),
            tracks: vec![TrackRef::Library(first), TrackRef::Library(second)],
            start: 0,
        })
        .unwrap();
    assert_eq!(next_path(&events), files[0]);
    let mut outcomes = vec![recorded(&events), recorded(&events)];
    outcomes.sort_by_key(|(track, _)| *track);
    assert_eq!(
        outcomes,
        vec![(first, ListenOutcome::Play), (second, ListenOutcome::Play)]
    );
    assert_eq!(library.stats(first).unwrap().play_count, 1);
    let history = library
        .history(&HistoryQuery {
            track: Some(first),
            ..HistoryQuery::default()
        })
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].started_at, start,
        "the start comes from the clock"
    );
    assert_eq!(history[0].context.as_deref(), Some("files"));

    // A file played by its path is recorded under its library track.
    player
        .send(PlayerCommand::PlayFiles {
            files: vec![files[1].clone()],
            start: 0,
        })
        .unwrap();
    assert_eq!(recorded(&events), (second, ListenOutcome::Play));
    assert_eq!(library.stats(second).unwrap().play_count, 2);
}

#[test]
fn moving_on_early_is_a_skip() {
    let dir = folder("stats_skip");
    let files = wav_files(&dir, 2, 20_000);
    let library = Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap());
    let first = add(&library, &dir, &files[0], 20_000);
    add(&library, &dir, &files[1], 20_000);
    let (player, events) = player_with(null_output(), SinkPacing::RealTime, |config| {
        config.library = Some(MusicLibrary::from_library(Arc::clone(&library), ":memory:"));
    });
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    until(
        &events,
        |e| matches!(e, PlayerEvent::Position { position, .. } if *position > MediaTime::from_millis(500)),
    );
    player.send(PlayerCommand::Next).unwrap();
    assert_eq!(recorded(&events), (first, ListenOutcome::Skip));
    let stats = library.stats(first).unwrap();
    assert_eq!((stats.play_count, stats.skip_count), (0, 1));
    // Stopping is neither.
    until(&events, |e| matches!(e, PlayerEvent::TrackChanged { .. }));
    player.send(PlayerCommand::Stop).unwrap();
    let (_, outcome) = recorded(&events);
    assert_eq!(outcome, ListenOutcome::Partial);
}
