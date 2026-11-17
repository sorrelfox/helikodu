// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Gapless joins: a continuous signal cut into three files plays back as one.

use std::path::PathBuf;

use helikodu_model::player::{EngineEvent, ItemId, TrackEndReason, TransportState};

use crate::media::{self, Signal};
use crate::session::{Session, item, null_config, wav_config};

/// Cut points that are no multiple of any codec's frame size.
const CUTS: [usize; 2] = [70_001, 150_013];
const FRAMES_44K: usize = 220_757;
const FRAMES_48K: usize = 240_263;

/// Writes the three parts of `signal` and encodes them with `encode`.
fn parts(
    signal: &Signal,
    tag: &str,
    encode: impl Fn(&std::path::Path, &str) -> Option<PathBuf>,
) -> Option<Vec<PathBuf>> {
    let bounds = [0, CUTS[0], CUTS[1], signal.frames()];
    let mut paths = Vec::new();
    for (i, w) in bounds.windows(2).enumerate() {
        let wav = media::write_wav(&format!("{tag}-part{i}.wav"), &signal.slice(w[0], w[1]));
        paths.push(encode(&wav, &format!("{tag}-part{i}"))?);
    }
    Some(paths)
}

/// Plays `files` as a queue to a WAV file and returns what was written, after
/// checking the events of the joins.
fn play(files: &[PathBuf], tag: &str) -> media::Written {
    let out = media::path(&format!("{tag}-out.wav"));
    let mut session = Session::new(wav_config(&out));
    let items: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(i, f)| item(i as u64 + 1, f))
        .collect();
    session.play_queue(&items);
    session.wait_state(TransportState::Stopped);

    let ends: Vec<_> = session
        .milestones()
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                EngineEvent::TrackStarted { .. } | EngineEvent::TrackEnded { .. }
            )
        })
        .map(|e| match e {
            EngineEvent::TrackStarted { item, .. } => (item.get(), None),
            EngineEvent::TrackEnded { item, reason } => (item.get(), Some(reason)),
            _ => unreachable!(),
        })
        .collect();
    let finished = Some(TrackEndReason::Finished);
    assert_eq!(
        ends,
        [
            (1, None),
            (1, finished),
            (2, None),
            (2, finished),
            (3, None),
            (3, finished)
        ],
        "{:#?}",
        session.log
    );
    drop(session);
    media::read_output(&out)
}

#[test]
fn wav_parts_join_bit_exact() {
    let signal = media::chirp(44_100, FRAMES_44K);
    let files = parts(&signal, "gapless-wav", |wav, _| Some(wav.to_owned())).unwrap();
    let written = play(&files, "gapless-wav");
    assert_eq!(written.rate, 44_100);
    assert_eq!(written.ints.len(), signal.samples.len());
    assert!(written.ints == signal.samples, "the joins changed samples");
}

#[test]
fn flac_parts_join_bit_exact() {
    let signal = media::chirp(44_100, FRAMES_44K);
    let Some(files) = parts(&signal, "gapless-flac", |wav, name| {
        media::encode(wav, &format!("{name}.flac"), "flac", &["-c:a", "flac"])
    }) else {
        return;
    };
    let written = play(&files, "gapless-flac");
    assert_eq!(written.ints.len(), signal.samples.len());
    let first_difference = written
        .ints
        .iter()
        .zip(&signal.samples)
        .position(|(a, b)| a != b);
    assert_eq!(first_difference, None, "the joins changed samples");
}

#[test]
fn mp3_parts_join_with_the_exact_length() {
    let signal = media::chirp(44_100, FRAMES_44K);
    let Some(files) = parts(&signal, "gapless-mp3", |wav, name| {
        media::encode(
            wav,
            &format!("{name}.mp3"),
            "libmp3lame",
            &["-c:a", "libmp3lame", "-b:a", "192k"],
        )
    }) else {
        return;
    };
    let written = play(&files, "gapless-mp3");
    assert_eq!(written.frames(), FRAMES_44K);
}

#[test]
fn opus_parts_join_with_the_exact_length() {
    let signal = media::chirp(48_000, FRAMES_48K);
    let Some(files) = parts(&signal, "gapless-opus", |wav, name| {
        media::encode(
            wav,
            &format!("{name}.opus"),
            "libopus",
            &["-c:a", "libopus", "-b:a", "128k"],
        )
    }) else {
        return;
    };
    let written = play(&files, "gapless-opus");
    assert_eq!(written.rate, 48_000);
    assert_eq!(written.frames(), FRAMES_48K);
}

#[test]
fn a_change_of_rate_reopens_the_device_between_items() {
    let a = media::write_wav("rates-a.wav", &media::sine(44_100, 20_000));
    let b = media::write_wav("rates-b.wav", &media::sine(48_000, 20_000));
    let mut session = Session::new(null_config());
    session.play_queue(&[item(1, &a), item(2, &b)]);
    session.wait_state(TransportState::Stopped);
    let milestones = session.milestones();
    let one = ItemId::new(1);
    let two = ItemId::new(2);
    let at = |wanted: &dyn Fn(&EngineEvent) -> bool| {
        milestones
            .iter()
            .position(wanted)
            .unwrap_or_else(|| panic!("{milestones:#?}"))
    };
    let ended = at(&|e| matches!(e, EngineEvent::TrackEnded { item, .. } if *item == one));
    let reconfiguring = at(&|e| *e == EngineEvent::StateChanged(TransportState::Reconfiguring));
    let started = at(&|e| matches!(e, EngineEvent::TrackStarted { item, .. } if *item == two));
    assert!(
        ended < reconfiguring && reconfiguring < started,
        "{milestones:#?}"
    );
}
