// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Commands while playing, in real time on the null sink.

use std::path::PathBuf;
use std::time::Duration;

use helikodu_model::audio::MediaTime;
use helikodu_model::player::{TransportState, Volume};
use helikodu_service::player::{EndReason, PlayerCommand, PlayerEvent, SinkPacing};

use crate::media::{
    RATE, folder, next_track, null_output, player, read_wav, samples, until, wav_output, write_wav,
};

/// `count` files of `seconds` each.
fn files(test: &str, count: usize, seconds: usize) -> (PathBuf, Vec<PathBuf>) {
    let dir = folder(test);
    let files = (0..count)
        .map(|i| {
            let path = dir.join(format!("{i}.wav"));
            write_wav(&path, &samples(seconds * RATE as usize, i as u32));
            path
        })
        .collect();
    (dir, files)
}

fn state(wanted: TransportState) -> impl FnMut(&PlayerEvent) -> bool {
    move |e| matches!(e, PlayerEvent::State { transport, .. } if *transport == wanted)
}

#[test]
fn next_previous_and_stop() {
    let (_, files) = files("next_previous", 3, 20);
    let (player, events) = player(null_output(), SinkPacing::RealTime);
    player
        .send(PlayerCommand::PlayFiles { files, start: 0 })
        .unwrap();
    assert_eq!(next_track(&events), 0);
    player.send(PlayerCommand::Next).unwrap();
    assert_eq!(next_track(&events), 1);
    player.send(PlayerCommand::Previous).unwrap();
    assert_eq!(next_track(&events), 0);
    player.send(PlayerCommand::Next).unwrap();
    assert_eq!(next_track(&events), 1);
    player.send(PlayerCommand::Next).unwrap();
    assert_eq!(next_track(&events), 2);
    player.send(PlayerCommand::Next).unwrap();
    let (end, _) = until(&events, |e| matches!(e, PlayerEvent::Ended { .. }));
    assert_eq!(
        end,
        PlayerEvent::Ended {
            reason: EndReason::QueueFinished,
            played: 5,
            failed: 0
        }
    );

    // Play after the end starts the current file again; stop ends it.
    player.send(PlayerCommand::Play).unwrap();
    assert_eq!(next_track(&events), 2);
    until(&events, state(TransportState::Playing));
    player.send(PlayerCommand::Stop).unwrap();
    let (end, _) = until(&events, |e| matches!(e, PlayerEvent::Ended { .. }));
    assert!(matches!(
        end,
        PlayerEvent::Ended {
            reason: EndReason::Stopped,
            ..
        }
    ));
}

#[test]
fn pause_seek_and_previous_restarts() {
    let (_, files) = files("pause_seek", 2, 20);
    let (player, events) = player(null_output(), SinkPacing::RealTime);
    player
        .send(PlayerCommand::PlayFiles { files, start: 1 })
        .unwrap();
    assert_eq!(next_track(&events), 1);
    until(&events, state(TransportState::Playing));

    player.send(PlayerCommand::PlayPause).unwrap();
    until(&events, state(TransportState::Paused));
    player
        .send(PlayerCommand::SeekForward(Duration::from_secs(5)))
        .unwrap();
    // The position where the pause began comes first.
    let (moved, _) = until(
        &events,
        |e| matches!(e, PlayerEvent::Position { position, .. } if *position >= MediaTime::from_millis(5_000)),
    );
    let PlayerEvent::Position {
        index,
        position,
        length,
    } = moved
    else {
        unreachable!()
    };
    assert_eq!(index, 1);
    assert!(position < MediaTime::from_millis(6_000), "{position}");
    assert_eq!(length, Some(MediaTime::from_millis(20_000)));

    player.send(PlayerCommand::PlayPause).unwrap();
    until(&events, state(TransportState::Playing));

    // More than three seconds in: Previous restarts the file.
    player.send(PlayerCommand::Previous).unwrap();
    let (_, seen) = until(
        &events,
        |e| matches!(e, PlayerEvent::Position { position, .. } if *position < MediaTime::from_millis(1_000)),
    );
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, PlayerEvent::TrackChanged { .. })),
        "{seen:#?}"
    );

    // Back at the start: Previous goes to the file before.
    player.send(PlayerCommand::Previous).unwrap();
    assert_eq!(next_track(&events), 0);

    player
        .send(PlayerCommand::SeekBackward(Duration::from_secs(60)))
        .unwrap();
    until(
        &events,
        |e| matches!(e, PlayerEvent::Position { position, .. } if *position < MediaTime::from_millis(1_000)),
    );
}

#[test]
fn volume_changes_are_reported_and_clamped() {
    let (_, files) = files("volume", 1, 5);
    let (player, events) = player(null_output(), SinkPacing::RealTime);
    player
        .send(PlayerCommand::PlayFiles { files, start: 0 })
        .unwrap();
    // The player announces the volume it starts with first.
    let (event, _) = until(&events, |e| matches!(e, PlayerEvent::VolumeChanged(_)));
    assert_eq!(event, PlayerEvent::VolumeChanged(Volume::FULL));
    player.send(PlayerCommand::AdjustVolume(-0.25)).unwrap();
    let (event, _) = until(&events, |e| matches!(e, PlayerEvent::VolumeChanged(_)));
    assert_eq!(
        event,
        PlayerEvent::VolumeChanged(Volume::new(0.75).unwrap())
    );
    player.send(PlayerCommand::AdjustVolume(2.0)).unwrap();
    let (event, _) = until(&events, |e| matches!(e, PlayerEvent::VolumeChanged(_)));
    assert_eq!(event, PlayerEvent::VolumeChanged(Volume::FULL));
}

#[test]
fn output_switches_while_playing() {
    let (dir, files) = files("switch", 1, 2);
    let out = dir.join("out.wav");
    let (player, events) = player(null_output(), SinkPacing::RealTime);
    player
        .send(PlayerCommand::PlayFiles { files, start: 0 })
        .unwrap();
    until(&events, state(TransportState::Playing));
    until(
        &events,
        |e| matches!(e, PlayerEvent::Position { position, .. } if *position > MediaTime::from_millis(500)),
    );

    let target = wav_output(&out);
    player
        .send(PlayerCommand::SetOutput(target.clone()))
        .unwrap();
    let (event, _) = until(&events, |e| matches!(e, PlayerEvent::OutputChanged(_)));
    assert_eq!(event, PlayerEvent::OutputChanged(target));
    let (end, seen) = until(&events, |e| matches!(e, PlayerEvent::Ended { .. }));
    assert!(seen.iter().any(|e| matches!(
        e,
        PlayerEvent::State {
            transport: TransportState::Reconfiguring,
            ..
        }
    )));
    assert_eq!(
        end,
        PlayerEvent::Ended {
            reason: EndReason::QueueFinished,
            played: 1,
            failed: 0
        }
    );
    drop(player);
    // The rest of the file, after the switch, reached the WAV file.
    let written = read_wav(&out).len() / 2;
    assert!(
        written > RATE as usize / 4 && written < 2 * RATE as usize,
        "{written} frames"
    );
}

#[test]
fn next_output_stays_on_the_null_backend() {
    let (_, files) = files("next_output", 1, 5);
    let (player, events) = player(null_output(), SinkPacing::RealTime);
    player
        .send(PlayerCommand::PlayFiles { files, start: 0 })
        .unwrap();
    until(&events, state(TransportState::Playing));
    player.send(PlayerCommand::NextOutput).unwrap();
    let (event, _) = until(&events, |e| {
        matches!(
            e,
            PlayerEvent::Problem { .. } | PlayerEvent::OutputChanged(_)
        )
    });
    assert!(
        matches!(event, PlayerEvent::Problem { file: None, .. }),
        "{event:?}"
    );
}
