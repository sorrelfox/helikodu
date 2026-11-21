// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smarter playback through the player, on WAV and null sinks: endless play and the
//! Auto-DJ adding library tracks, the rating floor of shuffle, Revert (and
//! whether it has somewhere to go), Radio Random, and intro scan, also at another
//! speed.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use helikodu_library::{DatabaseConfig, Library, Rating};
use helikodu_model::audio::MediaTime;
use helikodu_model::library::{TrackId, TrackRef};
use helikodu_model::player::{EnqueueAt, PlaybackState, QueueContext, RepeatMode, ShuffleMode};
use helikodu_model::settings::{PlaybackSettings, SmartPlaySettings, SpeedSettings};
use helikodu_service::bus::Subscription;
use helikodu_service::library::{MusicLibrary, PlaySeed};
use helikodu_service::player::{
    Player, PlayerCommand, PlayerConfig, PlayerEvent, SessionConfig, SinkPacing,
};

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
    setup_with(test, count, millis, real_time, smart, |_| {})
}

/// [`setup`] with more of the player's configuration changed by `change`.
fn setup_with(
    test: &str,
    count: usize,
    millis: usize,
    real_time: bool,
    smart: SmartPlaySettings,
    change: impl FnOnce(&mut PlayerConfig),
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
        change(config);
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
fn the_queue_snapshot_marks_what_helikodu_added() {
    let mut settings = smart();
    settings.auto_dj.ahead = 3;
    let s = setup("added_marks", 6, 150, false, settings);
    play(&s, &s.tracks[..1]);
    s.player.send(PlayerCommand::Stop).unwrap();
    s.player
        .send(PlayerCommand::SetRepeat(RepeatMode::AutoDj))
        .unwrap();
    s.player.sync().unwrap();
    let queue = s.player.queue();
    assert_eq!(queue.entries.len(), 4, "{:?}", queue.entries);
    let added: Vec<_> = queue.entries[1..].iter().map(|e| e.id).collect();
    assert_eq!(queue.smart.added, added);
    assert!(!queue.smart.nothing_to_add);
    assert_eq!(queue.smart.radio_seed, None);
    // A moved entry stays marked; a removed one leaves the list.
    s.player
        .send(PlayerCommand::Move {
            entry: added[2],
            to: 0,
        })
        .unwrap();
    s.player
        .send(PlayerCommand::Remove(vec![added[0]]))
        .unwrap();
    s.player.sync().unwrap();
    let queue = s.player.queue();
    assert!(queue.smart.added.contains(&added[2]));
    assert!(!queue.smart.added.contains(&added[0]));
}

#[test]
fn nothing_to_add_is_reported_when_no_track_matches() {
    let mut settings = smart();
    settings.auto_dj.filter = "title:\"no such title\"".into();
    let s = setup("nothing_to_add", 3, 150, false, settings);
    s.player
        .send(PlayerCommand::SetRepeat(RepeatMode::AutoDj))
        .unwrap();
    s.player.sync().unwrap();
    let queue = s.player.queue();
    assert!(queue.entries.is_empty());
    assert!(queue.smart.nothing_to_add);
    // Another mode forgets it.
    s.player
        .send(PlayerCommand::SetRepeat(RepeatMode::Off))
        .unwrap();
    s.player.sync().unwrap();
    assert!(!s.player.queue().smart.nothing_to_add);
}

#[test]
fn a_radio_keeps_its_seed_and_marks_the_related_tracks() {
    let s = setup("radio_marks", 5, 150, false, smart());
    s.player
        .send(PlayerCommand::StartRadio(PlaySeed::Track(s.tracks[1])))
        .unwrap();
    s.player.send(PlayerCommand::Stop).unwrap();
    s.player.sync().unwrap();
    let queue = s.player.queue();
    assert_eq!(queue.modes.repeat, RepeatMode::Endless);
    assert_eq!(queue.smart.radio_seed, Some(PlaySeed::Track(s.tracks[1])));
    assert_eq!(queue.entries[0].track, TrackRef::Library(s.tracks[1]));
    assert!(!queue.smart.added.contains(&queue.entries[0].id));
    assert_eq!(queue.smart.added.len(), queue.entries.len() - 1);
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

/// Waits until `done` holds, for at most five seconds.
fn eventually(what: &str, done: impl Fn() -> bool) {
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(
            std::time::Instant::now() < end,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
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
    assert!(!s.player.queue().can_revert, "nothing to revert to yet");
    s.player.send(PlayerCommand::Next).unwrap();
    assert_eq!(next_path(&s.events), s.files[1]);
    eventually("Revert to have a track", || s.player.queue().can_revert);
    s.player.send(PlayerCommand::Revert).unwrap();
    assert_eq!(next_path(&s.events), s.files[0]);
    eventually("Revert to have none", || !s.player.queue().can_revert);
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

#[test]
fn intro_scan_counts_listening_time_at_another_speed() {
    let mut settings = smart();
    settings.intro_scan_s = 1;
    // In real time: freewheeling, the sink runs far ahead of the positions. Long
    // tracks: intro scan waits while the next track is being prepared.
    let s = setup("intro_scan_speed", 2, 30_000, true, settings);
    let speed = SpeedSettings::at(2.0, true);
    s.player.send(PlayerCommand::SetSpeed(speed)).unwrap();
    until(&s.events, |e| *e == PlayerEvent::SpeedChanged(speed));
    assert_eq!(s.player.status().speed, speed);
    s.player.send(PlayerCommand::SetIntroScan(true)).unwrap();
    play(&s, &s.tracks);
    let (_, seen) = until(&s.events, |e| matches!(e, PlayerEvent::Ended { .. }));
    // A second of listening at twice the speed is two of each track.
    let longest = seen
        .iter()
        .filter_map(|e| match e {
            PlayerEvent::Position { position, .. } => Some(position.as_duration()),
            _ => None,
        })
        .max()
        .unwrap();
    assert!(
        longest >= Duration::from_millis(1_800) && longest < Duration::from_secs(3),
        "{longest:?}"
    );
}

/// Waits until the player's status is `playback`.
fn until_playback(s: &Setup, playback: PlaybackState) {
    eventually(&format!("{playback:?}"), || {
        s.player.status().playback == playback
    });
}

/// A queue of tracks 0 to 2 shuffled, with track 4 queued with "Play Next" and
/// track 3 with "Add to Queue", the first playing.
fn queue_with_queued_entries(s: &Setup) {
    s.player
        .send(PlayerCommand::SetShuffle(ShuffleMode::Tracks))
        .unwrap();
    play(s, &s.tracks[..3]);
    s.player
        .send(PlayerCommand::Enqueue {
            tracks: vec![TrackRef::Library(s.tracks[3])],
            at: EnqueueAt::End,
        })
        .unwrap();
    s.player
        .send(PlayerCommand::Enqueue {
            tracks: vec![TrackRef::Library(s.tracks[4])],
            at: EnqueueAt::Next,
        })
        .unwrap();
    assert_eq!(next_path(&s.events), s.files[0]);
}

/// Starts a radio from track 1 and waits until it plays.
fn start_radio(s: &Setup) {
    s.player
        .send(PlayerCommand::StartRadio(PlaySeed::Track(s.tracks[1])))
        .unwrap();
    assert_eq!(next_path(&s.events), s.files[1]);
    assert_eq!(s.player.queue().modes.repeat, RepeatMode::Endless);
}

/// The parts of the queue an Undo of a radio puts back.
fn queue_state(s: &Setup) -> impl PartialEq + std::fmt::Debug {
    let queue = s.player.queue();
    (
        queue.context.clone(),
        queue.entries.clone(),
        queue.current,
        queue.modes,
        queue.smart.clone(),
    )
}

#[test]
fn undo_of_a_radio_puts_a_paused_queue_back_exactly() {
    let s = setup("undo_radio_paused", 6, 20_000, true, smart());
    queue_with_queued_entries(&s);
    until(&s.events, position_past(400));
    s.player.send(PlayerCommand::Pause).unwrap();
    until_playback(&s, PlaybackState::Paused);
    s.player.sync().unwrap();
    let before = queue_state(&s);
    let position = s.player.status().position;
    assert!(position >= MediaTime::from_millis(400), "{position:?}");

    start_radio(&s);
    s.player.send(PlayerCommand::UndoRadio).unwrap();
    s.player.sync().unwrap();
    // The entries with their kinds, the shuffled order, the current entry and
    // the modes are those from before; the queued entries stay queued.
    assert_eq!(queue_state(&s), before);
    let queue = s.player.queue();
    assert_eq!(
        queue.entries.iter().filter(|e| !e.from_context).count(),
        2,
        "{queue:?}"
    );
    // Paused where it was, and it stays paused.
    until_playback(&s, PlaybackState::Paused);
    std::thread::sleep(Duration::from_millis(400));
    let status = s.player.status();
    assert_eq!(status.playback, PlaybackState::Paused);
    let off = status
        .position
        .as_duration()
        .abs_diff(position.as_duration());
    assert!(off < Duration::from_millis(50), "{status:?} {position:?}");
    // A second Undo has nothing left to do.
    s.player.send(PlayerCommand::UndoRadio).unwrap();
    s.player.sync().unwrap();
    assert_eq!(queue_state(&s), before);
    s.player.send(PlayerCommand::Stop).unwrap();
}

#[test]
fn undo_of_a_radio_puts_a_playing_queue_back_playing_where_it_was() {
    let s = setup("undo_radio_playing", 6, 20_000, true, smart());
    queue_with_queued_entries(&s);
    until(&s.events, position_past(500));
    s.player.sync().unwrap();
    let before = queue_state(&s);
    start_radio(&s);
    s.player.send(PlayerCommand::UndoRadio).unwrap();
    assert_eq!(next_path(&s.events), s.files[0]);
    let (event, _) = until(&s.events, |e| matches!(e, PlayerEvent::Position { .. }));
    assert!(position_past(450)(&event), "{event:?}");
    assert_eq!(queue_state(&s), before);
    assert_eq!(s.player.status().playback, PlaybackState::Playing);
    s.player.send(PlayerCommand::Stop).unwrap();
}

#[test]
fn undo_of_a_radio_leaves_a_stopped_queue_stopped() {
    let s = setup("undo_radio_stopped", 6, 20_000, true, smart());
    queue_with_queued_entries(&s);
    s.player.send(PlayerCommand::Stop).unwrap();
    until_playback(&s, PlaybackState::Stopped);
    s.player.sync().unwrap();
    let before = queue_state(&s);
    start_radio(&s);
    s.player.send(PlayerCommand::UndoRadio).unwrap();
    s.player.sync().unwrap();
    assert_eq!(queue_state(&s), before);
    until_playback(&s, PlaybackState::Stopped);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(s.player.status().playback, PlaybackState::Stopped);
}

#[test]
fn undo_of_a_radio_does_nothing_once_the_queue_changed_otherwise() {
    let s = setup("undo_radio_changed", 6, 150, false, smart());
    play(&s, &s.tracks[..3]);
    s.player.send(PlayerCommand::Stop).unwrap();
    s.player
        .send(PlayerCommand::StartRadio(PlaySeed::Track(s.tracks[1])))
        .unwrap();
    s.player.send(PlayerCommand::Stop).unwrap();
    s.player
        .send(PlayerCommand::Enqueue {
            tracks: vec![TrackRef::Library(s.tracks[5])],
            at: EnqueueAt::End,
        })
        .unwrap();
    s.player.sync().unwrap();
    let radio = queue_state(&s);
    s.player.send(PlayerCommand::UndoRadio).unwrap();
    s.player.sync().unwrap();
    assert_eq!(queue_state(&s), radio);
    assert_eq!(s.player.queue().modes.repeat, RepeatMode::Endless);
}

#[test]
fn what_helikodu_added_is_still_marked_after_a_restart() {
    let state = folder("radio_session_state");
    let session = |restore: bool| {
        let state = state.clone();
        move |config: &mut PlayerConfig| {
            let mut session = SessionConfig::new(&state);
            session.debounce = Duration::ZERO;
            session.restore = restore;
            config.session = Some(session);
        }
    };
    let s = setup_with("radio_session", 5, 150, false, smart(), session(true));
    s.player
        .send(PlayerCommand::StartRadio(PlaySeed::Track(s.tracks[1])))
        .unwrap();
    s.player.send(PlayerCommand::Stop).unwrap();
    s.player.sync().unwrap();
    let before = s.player.queue();
    assert!(!before.smart.added.is_empty());
    let Setup {
        library, player, ..
    } = s;
    drop(player);

    let music = MusicLibrary::from_library(Arc::clone(&library), ":memory:");
    let (player, _events) = player_with(null_output(), SinkPacing::RealTime, |config| {
        config.library = Some(music);
        config.shuffle_seed = Some(5);
        session(true)(config);
    });
    player.sync().unwrap();
    let after = player.queue();
    assert_eq!(after.entries, before.entries);
    assert_eq!(after.smart.added, before.smart.added);
    assert_eq!(
        after.smart.radio_seed,
        Some(PlaySeed::Track(library_track(&before)))
    );
}

/// The seed track of a radio started from a track: the queue's first entry.
fn library_track(queue: &helikodu_service::player::QueueSnapshot) -> TrackId {
    match queue.entries[0].track {
        TrackRef::Library(id) => id,
        TrackRef::File(_) => panic!("not a library track"),
    }
}
