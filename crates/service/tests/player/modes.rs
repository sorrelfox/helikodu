// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Play modes and queue editing through the engine: stop-after, repeat, shuffle,
//! "Play next", removing and choosing entries, and the order of events.

use std::collections::HashSet;
use std::time::Duration;

use helikodu_model::library::{MediaSource, TrackRef};
use helikodu_model::player::{
    EnqueueAt, PlaybackState, RepeatMode, ShuffleMode, StopAfter, TransportState,
};
use helikodu_service::SubscribeOptions;
use helikodu_service::player::{EndReason, PlayerCommand, PlayerEvent, SinkPacing};

use crate::media::{
    folder, next_path, null_output, player, player_with, samples, until, wav_files, wav_output,
    write_wav,
};

fn ended(event: &PlayerEvent) -> bool {
    matches!(event, PlayerEvent::Ended { .. })
}

#[test]
fn stop_after_ends_with_the_current_track() {
    let dir = folder("stop_after");
    let first = samples(8_000, 1);
    let files = wav_files(&dir, 3, 200);
    write_wav(&files[0], &first);
    let out = dir.join("out.wav");
    let (player, events) = player(wav_output(&out), SinkPacing::Freewheel);
    player
        .send(PlayerCommand::SetStopAfter(StopAfter::Track))
        .unwrap();
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    let (end, seen) = until(&events, ended);
    assert_eq!(
        end,
        PlayerEvent::Ended {
            reason: EndReason::StopAfter,
            played: 1,
            failed: 0
        }
    );
    // The mark goes away when the track ends, and the next entry is current.
    assert!(seen.contains(&PlayerEvent::StopAfterChanged(StopAfter::Off)));
    player.sync().unwrap();
    assert_eq!(player.queue().current, Some(1));
    assert_eq!(player.status().stop_after, StopAfter::Off);
    assert_eq!(player.status().playback, PlaybackState::Stopped);

    // Play continues with the next track.
    player.send(PlayerCommand::Play).unwrap();
    assert_eq!(next_path(&events), files[1]);
}

#[test]
fn repeat_one_plays_the_entry_again() {
    let dir = folder("repeat_one");
    let files = wav_files(&dir, 2, 100);
    let (player, events) = player(wav_output(&dir.join("out.wav")), SinkPacing::Freewheel);
    player
        .send(PlayerCommand::SetRepeat(RepeatMode::One))
        .unwrap();
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 1,
        })
        .unwrap();
    for _ in 0..3 {
        assert_eq!(next_path(&events), files[1]);
    }
    // "Next" still moves on; with repeat-all it wraps to the first entry.
    player
        .send(PlayerCommand::SetRepeat(RepeatMode::All))
        .unwrap();
    player.send(PlayerCommand::Next).unwrap();
    assert_eq!(next_path(&events), files[0]);
    player.send(PlayerCommand::Stop).unwrap();
    until(&events, ended);
}

#[test]
fn shuffled_repeat_plays_every_entry_once_per_cycle() {
    let dir = folder("shuffle_repeat");
    let files = wav_files(&dir, 4, 100);
    let (player, events) = player_with(
        wav_output(&dir.join("out.wav")),
        SinkPacing::Freewheel,
        |config| config.shuffle_seed = Some(11),
    );
    player
        .send(PlayerCommand::SetShuffle(ShuffleMode::Tracks))
        .unwrap();
    player
        .send(PlayerCommand::SetRepeat(RepeatMode::All))
        .unwrap();
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 2,
        })
        .unwrap();
    let played: Vec<_> = (0..12).map(|_| next_path(&events)).collect();
    player.send(PlayerCommand::Stop).unwrap();
    assert_eq!(played[0], files[2], "the chosen track comes first");
    for cycle in played.chunks(4) {
        let distinct: HashSet<_> = cycle.iter().collect();
        assert_eq!(distinct.len(), 4, "{played:?}");
    }
    until(&events, ended);
}

#[test]
fn play_next_remove_and_choose() {
    let dir = folder("edit");
    let files = wav_files(&dir, 4, 20_000);
    let (player, events) = player(null_output(), SinkPacing::RealTime);
    player
        .send(PlayerCommand::PlayFiles {
            files: files[..3].to_vec(),
            start: 0,
        })
        .unwrap();
    assert_eq!(next_path(&events), files[0]);
    player
        .send(PlayerCommand::Enqueue {
            tracks: vec![TrackRef::File(MediaSource::file(&files[3]))],
            at: EnqueueAt::Next,
        })
        .unwrap();
    player.sync().unwrap();
    let queue = player.queue();
    let order: Vec<_> = queue.entries.iter().map(|e| e.track.clone()).collect();
    let file = |i: usize| TrackRef::File(MediaSource::file(&files[i]));
    assert_eq!(order, vec![file(0), file(3), file(1), file(2)]);
    assert!(!queue.entries[1].from_context);
    assert_eq!(queue.upcoming().len(), 3);

    player.send(PlayerCommand::Next).unwrap();
    assert_eq!(next_path(&events), files[3]);
    // Removing the playing entry plays the one after it.
    let queued = player.queue().entries[1].id;
    player.send(PlayerCommand::Remove(vec![queued])).unwrap();
    assert_eq!(next_path(&events), files[1]);
    // Moving the last entry to the front keeps the current one.
    let last = player.queue().entries[2].id;
    player
        .send(PlayerCommand::Move { entry: last, to: 0 })
        .unwrap();
    player.sync().unwrap();
    assert_eq!(player.queue().current, Some(2));
    // Choosing an entry plays it.
    player.send(PlayerCommand::PlayEntry(last)).unwrap();
    assert_eq!(next_path(&events), files[2]);
    // Clear keeps the playing entry.
    player.send(PlayerCommand::Clear).unwrap();
    player.sync().unwrap();
    let queue = player.queue();
    assert_eq!(queue.entries.len(), 1);
    assert_eq!(queue.current_entry().map(|e| e.id), Some(last));
}

#[test]
fn events_come_in_order_and_positions_can_be_thinned() {
    let dir = folder("ordering");
    let files = wav_files(&dir, 2, 1_500);
    let (player, events) = player(null_output(), SinkPacing::RealTime);
    // Past the events of the start, so that both subscriptions see the same.
    player.sync().unwrap();
    let _ = events.try_iter().count();
    let thinned = player
        .subscribe(SubscribeOptions::player().with_position_interval(Duration::from_millis(400)));
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    let (_, seen) = until(&events, ended);
    let kinds: Vec<&str> = seen
        .iter()
        .filter_map(|e| match e {
            PlayerEvent::QueueChanged { .. } => Some("queue"),
            PlayerEvent::TrackChanged { .. } => Some("track"),
            PlayerEvent::State {
                transport: TransportState::Playing,
                ..
            } => Some("playing"),
            _ => None,
        })
        .collect();
    let first_queue = kinds.iter().position(|k| *k == "queue").unwrap();
    let first_track = kinds.iter().position(|k| *k == "track").unwrap();
    assert!(first_queue < first_track, "{kinds:?}");
    assert_eq!(kinds.iter().filter(|k| **k == "track").count(), 2);

    // Positions rise within each track.
    let mut last = None;
    let mut positions = 0;
    for event in &seen {
        if let PlayerEvent::Position {
            index, position, ..
        } = event
        {
            positions += 1;
            if let Some((last_index, last_position)) = last
                && last_index == *index
            {
                assert!(*position >= last_position, "{seen:#?}");
            }
            last = Some((*index, *position));
        }
    }
    // The thinned subscription got the same other events, in the same order, and
    // fewer positions.
    let thinned: Vec<PlayerEvent> = thinned.try_iter().collect();
    let without_positions = |events: &[PlayerEvent]| -> Vec<PlayerEvent> {
        events
            .iter()
            .filter(|e| !matches!(e, PlayerEvent::Position { .. }))
            .cloned()
            .collect()
    };
    let mut all = seen.clone();
    all.push(PlayerEvent::Ended {
        reason: EndReason::QueueFinished,
        played: 2,
        failed: 0,
    });
    assert_eq!(without_positions(&thinned), without_positions(&all));
    let thinned_positions = thinned
        .iter()
        .filter(|e| matches!(e, PlayerEvent::Position { .. }))
        .count();
    assert!(thinned_positions > 0 && thinned_positions < positions);
}
