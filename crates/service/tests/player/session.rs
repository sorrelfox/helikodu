// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The session: saved while playing and at the end, restored paused at the start,
//! and played on when it was playing and the front end asks.

use std::path::Path;
use std::time::Duration;

use helikodu_model::audio::MediaTime;
use helikodu_model::player::{PlaybackState, RepeatMode, ShuffleMode, Volume};
use helikodu_service::bus::Subscription;
use helikodu_service::player::session::{SENTINEL_FILE, SESSION_FILE};
use helikodu_service::player::{Player, PlayerCommand, PlayerEvent, SessionConfig, SinkPacing};

use crate::media::{folder, next_path, null_output, player_with, until, wav_files};

pub(crate) fn with_session(dir: &Path, restore: bool) -> (Player, Subscription<PlayerEvent>) {
    player_with(null_output(), SinkPacing::RealTime, |config| {
        let mut session = SessionConfig::new(dir);
        session.debounce = Duration::ZERO;
        session.restore = restore;
        config.session = Some(session);
        config.shuffle_seed = Some(5);
    })
}

#[test]
fn the_session_is_restored_paused() {
    let dir = folder("session");
    let state = dir.join("state");
    let files = wav_files(&dir, 3, 20_000);
    {
        let (player, events) = with_session(&state, true);
        player
            .send(PlayerCommand::PlayFiles {
                files: files.clone(),
                start: 0,
            })
            .unwrap();
        assert_eq!(next_path(&events), files[0]);
        player.send(PlayerCommand::Next).unwrap();
        assert_eq!(next_path(&events), files[1]);
        player
            .send(PlayerCommand::SetVolume(Volume::new(0.5).unwrap()))
            .unwrap();
        player
            .send(PlayerCommand::SetRepeat(RepeatMode::All))
            .unwrap();
        player
            .send(PlayerCommand::Seek(MediaTime::from_millis(4_000)))
            .unwrap();
        until(
            &events,
            |e| matches!(e, PlayerEvent::Position { position, .. } if *position >= MediaTime::from_millis(4_000)),
        );
        assert!(
            state.join(SENTINEL_FILE).exists(),
            "a track is named while it plays"
        );
        // Dropping the player saves the session and removes the sentinel.
    }
    assert!(state.join(SESSION_FILE).exists());
    assert!(!state.join(SENTINEL_FILE).exists());

    let (player, events) = with_session(&state, true);
    player.sync().unwrap();
    let queue = player.queue();
    assert_eq!(queue.entries.len(), 3);
    assert_eq!(queue.current, Some(1));
    assert_eq!(queue.modes.repeat, RepeatMode::All);
    let status = player.status();
    assert_eq!(status.playback, PlaybackState::Paused);
    assert_eq!(status.volume, Volume::new(0.5).unwrap());
    assert!(
        status.position >= MediaTime::from_millis(4_000),
        "{status:?}"
    );
    let (_, seen) = until(&events, |e| matches!(e, PlayerEvent::QueueChanged { .. }));
    assert!(
        !seen.iter().any(|e| matches!(e, PlayerEvent::Notice(_))),
        "a clean end is no crash: {seen:#?}"
    );

    // Play continues where it was.
    player.send(PlayerCommand::Play).unwrap();
    assert_eq!(next_path(&events), files[1]);
    let (first, _) = until(&events, |e| matches!(e, PlayerEvent::Position { .. }));
    let PlayerEvent::Position { position, .. } = first else {
        unreachable!()
    };
    assert!(position >= MediaTime::from_millis(3_500), "{position}");
    drop(player);

    // Without restoring, the queue starts empty.
    let (player, _) = with_session(&state, false);
    player.sync().unwrap();
    assert!(player.queue().entries.is_empty());
    assert_eq!(player.status().playback, PlaybackState::Stopped);
}

#[test]
fn shuffle_order_survives_a_restart() {
    let dir = folder("session_shuffle");
    let state = dir.join("state");
    let files = wav_files(&dir, 6, 20_000);
    let order = {
        let (player, events) = with_session(&state, true);
        player
            .send(PlayerCommand::SetShuffle(ShuffleMode::Tracks))
            .unwrap();
        player
            .send(PlayerCommand::PlayFiles { files, start: 0 })
            .unwrap();
        next_path(&events);
        player.sync().unwrap();
        player.queue().entries.clone()
    };
    let (player, _) = with_session(&state, true);
    player.sync().unwrap();
    assert_eq!(player.queue().entries, order);
    assert_eq!(player.queue().modes.shuffle, ShuffleMode::Tracks);
}

/// Plays the second of `files` from four seconds and ends the program while it
/// plays, or after `then` (a pause or a stop).
fn end_while(state: &Path, files: &[std::path::PathBuf], then: Option<PlayerCommand>) {
    let (player, events) = with_session(state, true);
    player
        .send(PlayerCommand::PlayFiles {
            files: files.to_vec(),
            start: 1,
        })
        .unwrap();
    assert_eq!(next_path(&events), files[1]);
    player
        .send(PlayerCommand::Seek(MediaTime::from_millis(4_000)))
        .unwrap();
    until(
        &events,
        |e| matches!(e, PlayerEvent::Position { position, .. } if *position >= MediaTime::from_millis(4_000)),
    );
    if let Some(command) = then {
        let wanted = if command == PlayerCommand::Pause {
            PlaybackState::Paused
        } else {
            PlaybackState::Stopped
        };
        player.send(command).unwrap();
        until(
            &events,
            |e| matches!(e, PlayerEvent::State { playback, .. } if *playback == wanted),
        );
    }
}

/// Starts again, asks to play on, and returns the player once it has handled it.
fn start_and_resume(state: &Path, restore: bool) -> (Player, Subscription<PlayerEvent>) {
    let (player, events) = with_session(state, restore);
    player.send(PlayerCommand::ResumeSession).unwrap();
    player.sync().unwrap();
    (player, events)
}

/// Gives a resume that should not happen time to show, then checks it did not.
fn assert_not_resumed(player: &Player, events: &Subscription<PlayerEvent>) {
    std::thread::sleep(Duration::from_millis(300));
    player.sync().unwrap();
    assert!(
        !events
            .try_iter()
            .any(|e| matches!(e, PlayerEvent::TrackChanged { .. })),
        "nothing was loaded"
    );
    assert_ne!(player.status().playback, PlaybackState::Playing);
}

#[test]
fn a_session_that_was_playing_plays_on() {
    let dir = folder("resume_playing");
    let state = dir.join("state");
    let files = wav_files(&dir, 3, 20_000);
    end_while(&state, &files, None);

    let (player, events) = start_and_resume(&state, true);
    assert_eq!(next_path(&events), files[1]);
    let (first, _) = until(&events, |e| matches!(e, PlayerEvent::Position { .. }));
    let PlayerEvent::Position { position, .. } = first else {
        unreachable!()
    };
    assert!(position >= MediaTime::from_millis(3_500), "{position}");
    until(&events, |e| {
        matches!(
            e,
            PlayerEvent::State {
                playback: PlaybackState::Playing,
                ..
            }
        )
    });
    assert_eq!(player.queue().current, Some(1));
    // Asked again, nothing more happens.
    player.send(PlayerCommand::ResumeSession).unwrap();
    player.sync().unwrap();
    assert_eq!(player.status().playback, PlaybackState::Playing);
    drop(player);

    // Without restoring, there is nothing to play on.
    let (player, events) = start_and_resume(&state, false);
    assert!(player.queue().entries.is_empty());
    assert_not_resumed(&player, &events);
    assert_eq!(player.status().playback, PlaybackState::Stopped);
}

#[test]
fn a_paused_or_stopped_session_stays_put() {
    let dir = folder("resume_paused");
    let state = dir.join("state");
    let files = wav_files(&dir, 3, 20_000);
    for then in [PlayerCommand::Pause, PlayerCommand::Stop] {
        end_while(&state, &files, Some(then));
        let (player, events) = start_and_resume(&state, true);
        assert_not_resumed(&player, &events);
        assert_eq!(player.queue().current, Some(1));
    }
}

#[test]
fn an_older_session_file_does_not_play_on() {
    let dir = folder("resume_older");
    let state = dir.join("state");
    let files = wav_files(&dir, 3, 20_000);
    end_while(&state, &files, None);
    let path = state.join(SESSION_FILE);
    let mut saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["playing"], serde_json::Value::Bool(true));
    saved.as_object_mut().unwrap().remove("playing");
    std::fs::write(&path, saved.to_string()).unwrap();

    let (player, events) = start_and_resume(&state, true);
    assert_not_resumed(&player, &events);
    assert_eq!(player.status().playback, PlaybackState::Paused);
    assert_eq!(player.queue().current, Some(1));
}

#[test]
fn a_track_that_may_have_ended_the_program_does_not_play_on() {
    let dir = folder("resume_crashed");
    let state = dir.join("state");
    let files = wav_files(&dir, 3, 20_000);
    end_while(&state, &files, None);
    // The program ended abnormally while the current track played: the sentinel
    // names it.
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state.join(SESSION_FILE)).unwrap()).unwrap();
    let track = saved["entries"][1]["track"].clone();
    let sentinel = serde_json::json!({ "tracks": [{ "track": track, "strikes": 0 }] });
    std::fs::write(state.join(SENTINEL_FILE), sentinel.to_string()).unwrap();

    let (player, events) = start_and_resume(&state, true);
    assert_not_resumed(&player, &events);
    assert_eq!(player.status().playback, PlaybackState::Paused);
    assert_eq!(player.queue().current, Some(1));
}

#[test]
fn a_missing_file_does_not_play_on() {
    let dir = folder("resume_missing");
    let state = dir.join("state");
    let files = wav_files(&dir, 3, 20_000);
    end_while(&state, &files, None);
    std::fs::remove_file(&files[1]).unwrap();

    let (player, events) = start_and_resume(&state, true);
    assert_not_resumed(&player, &events);
    assert!(
        !events
            .try_iter()
            .any(|e| matches!(e, PlayerEvent::Problem { .. })),
        "nothing was tried"
    );
    assert_eq!(player.queue().entries.len(), 3);
    assert_eq!(player.queue().current, Some(1));
}

#[test]
fn playing_something_else_first_wins() {
    let dir = folder("resume_other");
    let state = dir.join("state");
    let files = wav_files(&dir, 3, 20_000);
    end_while(&state, &files, None);

    let (player, events) = with_session(&state, true);
    player
        .send(PlayerCommand::PlayFiles {
            files: vec![files[2].clone()],
            start: 0,
        })
        .unwrap();
    player.send(PlayerCommand::ResumeSession).unwrap();
    assert_eq!(next_path(&events), files[2]);
    player.sync().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !events
            .try_iter()
            .any(|e| matches!(e, PlayerEvent::TrackChanged { .. })),
        "the session does not play after it"
    );
    assert_eq!(player.queue().entries.len(), 1);
}

#[test]
fn an_ending_desktop_session_is_no_crash() {
    let dir = folder("resume_logout");
    let state = dir.join("state");
    let files = wav_files(&dir, 3, 20_000);
    let (player, events) = with_session(&state, true);
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    assert_eq!(next_path(&events), files[0]);
    player.sync().unwrap();
    assert!(state.join(SENTINEL_FILE).exists());
    player.send(PlayerCommand::SessionEnding).unwrap();
    player.sync().unwrap();
    assert!(!state.join(SENTINEL_FILE).exists(), "nothing is named");
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state.join(SESSION_FILE)).unwrap()).unwrap();
    assert_eq!(saved["playing"], serde_json::Value::Bool(true));
    // When the logout is cancelled, the next track is named again.
    player.send(PlayerCommand::Next).unwrap();
    assert_eq!(next_path(&events), files[1]);
    player.sync().unwrap();
    assert!(state.join(SENTINEL_FILE).exists());
}
