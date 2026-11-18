// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The session: saved while playing and at the end, restored paused at the start.

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
