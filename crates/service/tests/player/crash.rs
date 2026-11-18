// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The crash sentinel. An abnormal end is simulated by leaking a player while a
//! track is named (paused, so that it changes nothing more): its files are left as a
//! crash leaves them.

use std::path::PathBuf;

use helikodu_model::library::{MediaSource, TrackRef};
use helikodu_model::player::{PlaybackState, TransportState};
use helikodu_service::bus::Subscription;
use helikodu_service::player::session::SENTINEL_FILE;
use helikodu_service::player::{Notice, Player, PlayerCommand, PlayerEvent};

use crate::media::{folder, next_path, until, wav_files};
use crate::session::with_session;

fn crash(player: Player, events: &Subscription<PlayerEvent>) {
    player.send(PlayerCommand::Pause).unwrap();
    until(events, |e| {
        matches!(
            e,
            PlayerEvent::State {
                transport: TransportState::Paused,
                ..
            }
        )
    });
    player.sync().unwrap();
    std::mem::forget(player);
}

fn file(path: &PathBuf) -> TrackRef {
    TrackRef::File(MediaSource::file(path))
}

#[test]
fn a_track_that_crashed_twice_is_passed_over() {
    let dir = folder("crash");
    let state = dir.join("state");
    let files = wav_files(&dir, 3, 30_000);
    let culprit = &files[1];

    // First run: the culprit plays when the program dies.
    let (player, events) = with_session(&state, true);
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 1,
        })
        .unwrap();
    assert_eq!(next_path(&events), *culprit);
    assert!(state.join(SENTINEL_FILE).exists());
    crash(player, &events);

    // Second run: restored paused, with a notice; one strike is not enough.
    let (player, events) = with_session(&state, true);
    let (notice, _) = until(&events, |e| matches!(e, PlayerEvent::Notice(_)));
    assert_eq!(
        notice,
        PlayerEvent::Notice(Notice::RecoveredAfterCrash {
            tracks: vec![file(culprit)],
            suspects: vec![],
        })
    );
    player.sync().unwrap();
    assert_eq!(player.status().playback, PlaybackState::Paused);
    assert!(
        !state.join(SENTINEL_FILE).exists(),
        "nothing plays, so nothing is named"
    );
    // The user starts it again, and the program dies again.
    player.send(PlayerCommand::Play).unwrap();
    assert_eq!(next_path(&events), *culprit);
    crash(player, &events);

    // Third run: the culprit is suspect.
    let (player, events) = with_session(&state, true);
    let (notice, _) = until(&events, |e| matches!(e, PlayerEvent::Notice(_)));
    assert_eq!(
        notice,
        PlayerEvent::Notice(Notice::RecoveredAfterCrash {
            tracks: vec![file(culprit)],
            suspects: vec![file(culprit)],
        })
    );
    // Automatic playback passes over it: "Next" from the first file goes to the third.
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    assert_eq!(next_path(&events), files[0]);
    player.send(PlayerCommand::Next).unwrap();
    let (skipped, _) = until(&events, |e| {
        matches!(e, PlayerEvent::Notice(Notice::SuspectSkipped { .. }))
    });
    assert!(matches!(
        skipped,
        PlayerEvent::Notice(Notice::SuspectSkipped { track, .. }) if track == file(culprit)
    ));
    assert_eq!(next_path(&events), files[2]);
    // Chosen directly, it plays.
    let entry = player.queue().entries[1].id;
    player.send(PlayerCommand::PlayEntry(entry)).unwrap();
    assert_eq!(next_path(&events), *culprit);
    drop(player);

    // A clean end leaves no sentinel; the mark stays in the session.
    assert!(!state.join(SENTINEL_FILE).exists());
    let (player, events) = with_session(&state, true);
    player.sync().unwrap();
    assert!(
        !events
            .try_iter()
            .any(|e| matches!(e, PlayerEvent::Notice(Notice::RecoveredAfterCrash { .. })))
    );
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    assert_eq!(next_path(&events), files[0]);
    player.send(PlayerCommand::Next).unwrap();
    assert_eq!(next_path(&events), files[2]);
}
