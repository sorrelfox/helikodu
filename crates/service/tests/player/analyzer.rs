// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The analyzer feed: frames while sound is played, one silent frame when it
//! stops, then nothing.

use std::sync::mpsc;
use std::time::Duration;

use helikodu_model::player::{AnalyzerFrame, PlaybackState};
use helikodu_service::player::{PlayerCommand, PlayerEvent, SinkPacing};

use crate::media::{folder, null_output, player, until, wav_files};

#[test]
fn frames_come_only_while_sound_is_played() {
    let dir = folder("analyzer");
    let files = wav_files(&dir, 1, 20_000);
    let (player, events) = player(null_output(), SinkPacing::RealTime);
    let (frames, received) = mpsc::channel::<AnalyzerFrame>();
    let watch = player
        .watch_analyzer(Duration::from_millis(33), move |frame| {
            frames.send(frame).is_ok()
        })
        .unwrap();
    // Nothing plays: nothing comes.
    assert!(received.recv_timeout(Duration::from_millis(300)).is_err());

    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    let frame = received.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(frame.live);
    assert_eq!(frame.channels.len(), 2);
    // The generated noise is heard in the levels.
    let frame = received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(frame.channels[0].rms_db > -60.0, "{:?}", frame.channels[0]);

    player.send(PlayerCommand::Pause).unwrap();
    until(&events, |e| {
        matches!(
            e,
            PlayerEvent::State {
                playback: PlaybackState::Paused,
                ..
            }
        )
    });
    // The last frame is silent and not live; after it, nothing.
    let last = loop {
        let frame = received.recv_timeout(Duration::from_secs(5)).unwrap();
        if !frame.live {
            break frame;
        }
    };
    assert!(last.channels.iter().all(|c| c.rms_db <= -90.0));
    assert!(received.recv_timeout(Duration::from_millis(500)).is_err());

    // Closing the watch ends the thread.
    drop(watch);
    player.send(PlayerCommand::Play).unwrap();
    assert!(received.recv_timeout(Duration::from_millis(500)).is_err());
}
