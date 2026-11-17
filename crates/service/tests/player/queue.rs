// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Playing a queue to its end, freewheeling into the WAV sink.

use std::fs;

use helikodu_service::player::{EndReason, PlayerCommand, PlayerEvent, SinkPacing};

use crate::media::{folder, next_track, player, read_wav, samples, until, wav_output, write_wav};

fn ended(event: &PlayerEvent) -> bool {
    matches!(event, PlayerEvent::Ended { .. })
}

#[test]
fn queue_plays_in_order_without_gaps() {
    let dir = folder("in_order");
    let parts: Vec<Vec<i16>> = [22_050, 13_230, 30_870]
        .iter()
        .zip(1..)
        .map(|(&frames, seed)| samples(frames, seed))
        .collect();
    let mut files = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let path = dir.join(format!("{i}.wav"));
        write_wav(&path, part);
        files.push(path);
    }
    let out = dir.join("out.wav");
    let (player, events) = player(wav_output(&out), SinkPacing::Freewheel);
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    for (expected, file) in files.iter().enumerate() {
        let (event, _) = until(&events, |e| matches!(e, PlayerEvent::TrackChanged { .. }));
        let PlayerEvent::TrackChanged {
            index, count, path, ..
        } = event
        else {
            unreachable!()
        };
        assert_eq!((index, count), (expected, 3));
        assert_eq!(path, *file);
    }
    let (end, _) = until(&events, ended);
    assert_eq!(
        end,
        PlayerEvent::Ended {
            reason: EndReason::QueueFinished,
            played: 3,
            failed: 0
        }
    );
    drop(player);
    assert!(read_wav(&out) == parts.concat(), "the joins are not exact");
}

#[test]
fn unplayable_files_are_skipped() {
    let dir = folder("skipped");
    let first = samples(20_000, 7);
    let last = samples(15_000, 8);
    let good_a = dir.join("a.wav");
    let broken = dir.join("broken.flac");
    let missing = dir.join("missing.wav");
    let good_b = dir.join("b.wav");
    write_wav(&good_a, &first);
    fs::write(&broken, b"this is not audio at all, only text").unwrap();
    write_wav(&good_b, &last);
    let out = dir.join("out.wav");
    let (player, events) = player(wav_output(&out), SinkPacing::Freewheel);
    let files = vec![good_a, broken.clone(), missing.clone(), good_b];
    player
        .send(PlayerCommand::PlayFiles { files, start: 0 })
        .unwrap();
    let (end, seen) = until(&events, ended);
    assert_eq!(
        end,
        PlayerEvent::Ended {
            reason: EndReason::QueueFinished,
            played: 2,
            failed: 2
        }
    );
    let problems: Vec<_> = seen
        .iter()
        .filter_map(|e| match e {
            PlayerEvent::Problem { file, .. } => file.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(problems, vec![(1, broken), (2, missing)]);
    let started: Vec<_> = seen
        .iter()
        .filter_map(|e| match e {
            PlayerEvent::TrackChanged { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(started, vec![0, 3]);
    drop(player);
    assert!(read_wav(&out) == [first, last].concat());
}

#[test]
fn unplayable_first_file_is_skipped() {
    let dir = folder("first_skipped");
    let broken = dir.join("broken.mp3");
    fs::write(&broken, [0u8; 64]).unwrap();
    let good = dir.join("good.wav");
    write_wav(&good, &samples(10_000, 3));
    let (player, events) = player(wav_output(&dir.join("out.wav")), SinkPacing::Freewheel);
    player
        .send(PlayerCommand::PlayFiles {
            files: vec![broken, good],
            start: 0,
        })
        .unwrap();
    assert_eq!(next_track(&events), 1);
    let (end, _) = until(&events, ended);
    assert_eq!(
        end,
        PlayerEvent::Ended {
            reason: EndReason::QueueFinished,
            played: 1,
            failed: 1
        }
    );
}

#[test]
fn nothing_playable() {
    let dir = folder("nothing");
    let broken = dir.join("broken.ogg");
    fs::write(&broken, b"OggS but not really").unwrap();
    let (player, events) = player(wav_output(&dir.join("out.wav")), SinkPacing::Freewheel);
    player
        .send(PlayerCommand::PlayFiles {
            files: vec![dir.join("missing.flac"), broken],
            start: 0,
        })
        .unwrap();
    let (end, seen) = until(&events, ended);
    assert_eq!(
        end,
        PlayerEvent::Ended {
            reason: EndReason::QueueFinished,
            played: 0,
            failed: 2
        }
    );
    let problems = seen
        .iter()
        .filter(|e| matches!(e, PlayerEvent::Problem { .. }))
        .count();
    assert_eq!(problems, 2);
}

#[test]
fn empty_queue_ends_at_once() {
    let dir = folder("empty");
    let (player, events) = player(wav_output(&dir.join("out.wav")), SinkPacing::Freewheel);
    player
        .send(PlayerCommand::PlayFiles {
            files: Vec::new(),
            start: 0,
        })
        .unwrap();
    let (end, _) = until(&events, ended);
    assert_eq!(
        end,
        PlayerEvent::Ended {
            reason: EndReason::QueueFinished,
            played: 0,
            failed: 0
        }
    );
}
