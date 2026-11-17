// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Items that cannot be played, and outputs that cannot be opened.

use std::fs;

use helikodu_model::audio::MediaTime;
use helikodu_model::player::{EngineError, EngineEvent, ItemId, TrackEndReason, TransportState};

use crate::media;
use crate::session::{Session, item, wav_config};

fn broken_file(name: &str) -> std::path::PathBuf {
    let path = media::path(name);
    let mut noise = 0x2545_f491u32;
    let bytes: Vec<u8> = (0..64 * 1024)
        .map(|_| {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            noise as u8
        })
        .collect();
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn a_broken_item_in_the_queue_is_skipped() {
    let signal = media::chirp(44_100, 60_000);
    let a = media::write_wav("skip-a.wav", &signal.slice(0, 25_001));
    let c = media::write_wav("skip-c.wav", &signal.slice(25_001, 60_000));
    let broken = broken_file("skip-b.flac");
    let out = media::path("skip-out.wav");
    let mut session = Session::new(wav_config(&out));
    session.play_queue(&[item(1, &a), item(2, &broken), item(3, &c)]);
    session.wait_state(TransportState::Stopped);

    let log = &session.log;
    let failed = log.iter().position(|e| {
        *e == EngineEvent::TrackEnded {
            item: ItemId::new(2),
            reason: TrackEndReason::Failed,
        }
    });
    let failed = failed.unwrap_or_else(|| panic!("{log:#?}"));
    assert!(matches!(
        &log[failed - 1],
        EngineEvent::Error {
            item: Some(id),
            error: EngineError::Unplayable { .. }
        } if *id == ItemId::new(2)
    ));
    // The broken item never started; the third one follows the first.
    let starts: Vec<u64> = session
        .milestones()
        .iter()
        .filter_map(|e| match e {
            EngineEvent::TrackStarted { item, .. } => Some(item.get()),
            _ => None,
        })
        .collect();
    assert_eq!(starts, [1, 3]);
    drop(session);

    let written = media::read_output(&out).ints;
    assert!(
        written == signal.samples,
        "the join around the broken item is not exact"
    );
}

#[test]
fn an_unplayable_item_stops_with_an_error() {
    let broken = broken_file("unplayable.mp3");
    let out = media::path("unplayable-out.wav");
    let mut session = Session::new(wav_config(&out));
    session.load(&item(7, &broken), MediaTime::ZERO);
    session.wait_state(TransportState::Stopped);
    let milestones = session.milestones();
    assert_eq!(milestones.len(), 4, "{milestones:#?}");
    assert_eq!(
        milestones[0],
        EngineEvent::StateChanged(TransportState::Loading)
    );
    assert!(matches!(
        &milestones[1],
        EngineEvent::Error { item: Some(id), error: EngineError::Unplayable { .. } }
            if *id == ItemId::new(7)
    ));
    assert_eq!(
        milestones[2],
        EngineEvent::TrackEnded {
            item: ItemId::new(7),
            reason: TrackEndReason::Failed
        }
    );
}

#[test]
fn an_output_that_cannot_open_pauses_with_an_error() {
    let path = media::write_wav("no-output-in.wav", &media::sine(44_100, 10_000));
    let out = media::path("no-such-folder/out.wav");
    let mut session = Session::new(wav_config(&out));
    session.load(&item(1, &path), MediaTime::ZERO);
    session.wait_state(TransportState::Paused);
    assert!(
        session.log.iter().any(|e| matches!(
            e,
            EngineEvent::Error {
                item: Some(_),
                error: EngineError::Output { .. }
            }
        )),
        "{:#?}",
        session.log
    );
}
