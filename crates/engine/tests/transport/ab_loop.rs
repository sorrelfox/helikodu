// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The A-B loop: the frame after the loop's last one is its first one, exactly, on
//! every pass, the heard position stays inside it, and ending it lets playback go
//! on to the end of the item.

use std::path::PathBuf;

use helikodu_model::audio::{MediaTime, SampleRate};
use helikodu_model::player::{EngineCommand, ItemId, LoopRange, TransportState};

use crate::media::{self, Signal};
use crate::session::{Session, item, wav_config};

const RATE: u32 = 44_100;
const FRAMES: usize = 3 * 44_100;

fn rate() -> SampleRate {
    SampleRate::new(RATE).unwrap()
}

fn range(start: u64, end: u64) -> LoopRange {
    LoopRange::new(
        MediaTime::from_frames(start, rate()),
        MediaTime::from_frames(end, rate()),
    )
    .unwrap()
}

/// A signal whose frames carry their own index: the low 15 bits on the left, the
/// rest (plus one, so no frame is silent) on the right.
fn counting(frames: usize) -> Signal {
    let samples = (0..frames)
        .flat_map(|i| [(i % 32_768) as i16, (i / 32_768 + 1) as i16])
        .collect();
    Signal {
        rate: RATE,
        samples,
    }
}

fn counting_file(name: &str) -> PathBuf {
    media::write_wav(name, &counting(FRAMES))
}

/// The frame indices the WAV sink wrote.
fn indices(out: &std::path::Path) -> Vec<u64> {
    let written = media::read_output(out);
    assert_eq!(written.rate, RATE);
    written
        .ints
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[left, right]| {
            assert!(right >= 1, "a silent or broken frame: ({left}, {right})");
            u64::from(left as u16) + (u64::from(right as u16) - 1) * 32_768
        })
        .collect()
}

/// The runs of consecutive frames, as (first, last).
fn runs(indices: &[u64]) -> Vec<(u64, u64)> {
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for &i in indices {
        match runs.last_mut() {
            Some((_, last)) if *last + 1 == i => *last = i,
            _ => runs.push((i, i)),
        }
    }
    runs
}

/// Lets playback loop for a few position ticks, checks that the positions stay in
/// `start .. end` once inside it, then ends the loop and waits for the end.
fn loop_then_end(session: &mut Session, start: u64, end: u64) {
    let one = ItemId::new(1);
    let mut inside = false;
    let mut ticks = 0;
    while ticks < 3 {
        let position = session.wait_position(RATE);
        assert!(position < end, "position {position} past the loop's end");
        if position >= start {
            inside = true;
        } else {
            assert!(!inside, "position {position} before the loop's start");
        }
        ticks += inside as u32;
    }
    session.send(EngineCommand::SetLoop {
        item: one,
        range: None,
    });
    session.wait_state(TransportState::Stopped);
}

#[test]
fn a_loop_joins_its_end_to_its_start_exactly() {
    let path = counting_file("ab-loop-in.wav");
    let out = media::path("ab-loop-out.wav");
    let mut session = Session::new(wav_config(&out));
    let (start, end) = (10_000, 30_000);
    session.load(&item(1, &path), MediaTime::ZERO);
    // The first fill renders one buffer length (200 ms), short of the end.
    session.send(EngineCommand::SetLoop {
        item: ItemId::new(1),
        range: Some(range(start, end)),
    });
    loop_then_end(&mut session, start, end);
    drop(session);

    let runs = runs(&indices(&out));
    assert!(runs.len() >= 4, "too few passes: {runs:?}");
    assert_eq!(runs[0], (0, end - 1), "{runs:?}");
    for pass in &runs[1..runs.len() - 1] {
        assert_eq!(*pass, (start, end - 1), "{runs:?}");
    }
    assert_eq!(runs[runs.len() - 1], (start, FRAMES as u64 - 1), "{runs:?}");
}

#[test]
fn a_loop_whose_end_was_rendered_already_drops_what_follows() {
    let path = counting_file("ab-loop-behind-in.wav");
    let out = media::path("ab-loop-behind-out.wav");
    let mut session = Session::new(wav_config(&out));
    // The first fill renders 8820 frames: the end is behind them, and maybe behind
    // what the sink has taken too, which makes the jump come at once.
    let (start, end) = (1_000, 4_000);
    session.load(&item(1, &path), MediaTime::ZERO);
    session.send(EngineCommand::SetLoop {
        item: ItemId::new(1),
        range: Some(range(start, end)),
    });
    loop_then_end(&mut session, start, end);
    drop(session);

    let runs = runs(&indices(&out));
    assert!(runs.len() >= 4, "too few passes: {runs:?}");
    // Up to the end, or up to where the sink was when the loop came; then the start.
    assert_eq!(runs[0].0, 0);
    assert!(runs[0].1 >= end - 1, "{runs:?}");
    for pass in &runs[1..runs.len() - 1] {
        assert_eq!(*pass, (start, end - 1), "{runs:?}");
    }
    assert_eq!(runs[runs.len() - 1], (start, FRAMES as u64 - 1), "{runs:?}");
}

#[test]
fn a_loop_set_while_paused_plays_on_resume() {
    let path = counting_file("ab-loop-paused-in.wav");
    let out = media::path("ab-loop-paused-out.wav");
    let mut session = Session::new(wav_config(&out));
    let (start, end) = (20_000, 50_000);
    session.send(EngineCommand::Load {
        item: item(1, &path),
        start: MediaTime::ZERO,
        paused: true,
    });
    session.wait_state(TransportState::Paused);
    session.send(EngineCommand::SetLoop {
        item: ItemId::new(1),
        range: Some(range(start, end)),
    });
    // Another item's loop is ignored.
    session.send(EngineCommand::SetLoop {
        item: ItemId::new(7),
        range: None,
    });
    session.send(EngineCommand::Play);
    session.wait_state(TransportState::Playing);
    loop_then_end(&mut session, start, end);
    drop(session);

    let runs = runs(&indices(&out));
    assert!(runs.len() >= 3, "too few passes: {runs:?}");
    assert_eq!(runs[0], (0, end - 1), "{runs:?}");
    for pass in &runs[1..runs.len() - 1] {
        assert_eq!(*pass, (start, end - 1), "{runs:?}");
    }
    assert_eq!(runs[runs.len() - 1], (start, FRAMES as u64 - 1), "{runs:?}");
}
