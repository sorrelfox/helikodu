// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smarter playback through the player, on WAV and null sinks: endless play and the
//! Auto-DJ adding library tracks, the rating floor of shuffle, Revert, Radio Random
//! and intro scan.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use helikodu_library::{DatabaseConfig, Library, Rating};
use helikodu_model::library::{TrackId, TrackRef};
use helikodu_model::player::{QueueContext, RepeatMode, ShuffleMode};
use helikodu_model::settings::{PlaybackSettings, SmartPlaySettings};
use helikodu_service::bus::Subscription;
use helikodu_service::library::MusicLibrary;
use helikodu_service::player::{Player, PlayerCommand, PlayerEvent, SinkPacing};

use crate::media::{folder, next_path, null_output, player_with, until, wav_files, wav_output};
use crate::stats::add;

struct Setup {
    files: Vec<PathBuf>,
    tracks: Vec<TrackId>,
    library: Arc<Library>,
    player: Player,
    events: Subscription<PlayerEvent>,
}

/// `count` library tracks of `millis` each, and a player with the library, a fixed
/// seed and `smart`, on the WAV sink (freewheeling) or the null sink (in real time).
fn setup(
    test: &str,
    count: usize,
    millis: usize,
    real_time: bool,
    smart: SmartPlaySettings,
) -> Setup {
    let dir = folder(test);
    let files = wav_files(&dir, count, millis);
    let library = Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap());
    let tracks: Vec<TrackId> = files
        .iter()
        .map(|file| add(&library, &dir, file, millis as u64))
        .collect();
    let music = MusicLibrary::from_library(Arc::clone(&library), ":memory:");
    let (output, pacing) = if real_time {
        (null_output(), SinkPacing::RealTime)
    } else {
        (wav_output(&dir.join("out.wav")), SinkPacing::Freewheel)
    };
    let (player, events) = player_with(output, pacing, |config| {
        config.library = Some(music);
        config.shuffle_seed = Some(5);
        config.smart_play = smart;
    });
    Setup {
        files,
        tracks,
        library,
        player,
        events,
    }
}

fn smart() -> SmartPlaySettings {
    PlaybackSettings::default().smart_play()
}

fn play(setup: &Setup, tracks: &[TrackId]) {
    setup
        .player
        .send(PlayerCommand::PlayTracks {
            context: QueueContext::files(),
            tracks: tracks.iter().map(|t| TrackRef::Library(*t)).collect(),
            start: 0,
        })
        .unwrap();
}

#[test]
fn endless_play_goes_on_with_tracks_not_played_yet() {
    let mut settings = smart();
    settings.endless.batch = 2;
    let s = setup("endless", 6, 150, false, settings);
    s.player
        .send(PlayerCommand::SetRepeat(RepeatMode::Endless))
        .unwrap();
    play(&s, &s.tracks[..1]);
    let mut played = Vec::new();
    for _ in 0..6 {
        played.push(next_path(&s.events));
    }
    s.player.send(PlayerCommand::Stop).unwrap();
    let unique: HashSet<&PathBuf> = played.iter().collect();
    assert_eq!(unique.len(), 6, "every track once: {played:?}");
    assert_eq!(played[0], s.files[0]);
}

#[test]
fn the_auto_dj_keeps_tracks_ahead_from_an_empty_queue() {
    let mut settings = smart();
    settings.auto_dj.ahead = 3;
    let s = setup("auto_dj", 8, 150, false, settings);
    s.player
        .send(PlayerCommand::SetRepeat(RepeatMode::AutoDj))
        .unwrap();
    s.player.sync().unwrap();
    assert_eq!(s.player.queue().entries.len(), 3);
    s.player.send(PlayerCommand::Play).unwrap();
    next_path(&s.events);
    next_path(&s.events);
    s.player.send(PlayerCommand::Stop).unwrap();
    s.player.sync().unwrap();
    let queue = s.player.queue();
    let current = queue.current.unwrap();
    assert!(
        queue.entries.len() - current > 3,
        "three ahead: {} entries, at {current}",
        queue.entries.len()
    );
}

#[test]
fn the_rating_floor_passes_tracks_over_while_shuffling() {
    let mut settings = smart();
    settings.shuffle.min_rating = 4;
    let s = setup("shuffle_floor", 4, 120, false, settings);
    s.library
        .set_rating(s.tracks[2], Rating::from_half_stars(2))
        .unwrap();
    s.player
        .send(PlayerCommand::SetShuffle(ShuffleMode::Tracks))
        .unwrap();
    play(&s, &s.tracks);
    let mut played = Vec::new();
    let next = || {
        until(&s.events, |e| {
            matches!(
                e,
                PlayerEvent::TrackChanged { .. } | PlayerEvent::Ended { .. }
            )
        })
        .0
    };
    while let PlayerEvent::TrackChanged { path, .. } = next() {
        played.push(path);
    }
    assert_eq!(played.len(), 3, "{played:?}");
    assert!(!played.contains(&s.files[2]));
}

fn position_past(millis: u64) -> impl Fn(&PlayerEvent) -> bool {
    move |e| matches!(e, PlayerEvent::Position { position, .. } if position.as_duration() >= Duration::from_millis(millis))
}

#[test]
fn revert_goes_back_to_the_skipped_track_where_it_was() {
    let s = setup("revert", 3, 20_000, true, smart());
    play(&s, &s.tracks);
    assert_eq!(next_path(&s.events), s.files[0]);
    until(&s.events, position_past(600));
    s.player.send(PlayerCommand::Next).unwrap();
    assert_eq!(next_path(&s.events), s.files[1]);
    s.player.send(PlayerCommand::Revert).unwrap();
    assert_eq!(next_path(&s.events), s.files[0]);
    // It starts where it was skipped, not at the start.
    let (event, _) = until(&s.events, |e| matches!(e, PlayerEvent::Position { .. }));
    assert!(position_past(500)(&event), "{event:?}");
    s.player.send(PlayerCommand::Stop).unwrap();
}

#[test]
fn radio_random_jumps_into_another_track() {
    let s = setup("radio_random", 3, 20_000, true, smart());
    play(&s, &s.tracks);
    assert_eq!(next_path(&s.events), s.files[0]);
    s.player.send(PlayerCommand::RadioRandom).unwrap();
    let path = next_path(&s.events);
    assert_ne!(path, s.files[0]);
    // At least a tenth of the way in: two seconds of twenty.
    until(&s.events, position_past(1_900));
    s.player.send(PlayerCommand::Stop).unwrap();
}

#[test]
fn intro_scan_plays_the_start_of_each_track() {
    let mut settings = smart();
    settings.intro_scan_s = 3;
    let s = setup("intro_scan", 3, 10_000, false, settings);
    s.player.send(PlayerCommand::SetIntroScan(true)).unwrap();
    play(&s, &s.tracks);
    let (end, seen) = until(&s.events, |e| matches!(e, PlayerEvent::Ended { .. }));
    let played: Vec<&PathBuf> = seen
        .iter()
        .filter_map(|e| match e {
            PlayerEvent::TrackChanged { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    assert_eq!(played, s.files.iter().collect::<Vec<_>>());
    // No track played to its end: each was left after about three seconds.
    let longest = seen
        .iter()
        .filter_map(|e| match e {
            PlayerEvent::Position { position, .. } => Some(position.as_duration()),
            _ => None,
        })
        .max()
        .unwrap();
    assert!(longest < Duration::from_secs(5), "{longest:?}");
    assert!(matches!(end, PlayerEvent::Ended { .. }));
}
