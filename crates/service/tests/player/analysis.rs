// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The track analysis the library keeps reaches playback: a library track whose
//! stored analysis has leading and trailing silence is trimmed when the settings
//! ask for it, and played whole otherwise.

use std::sync::Arc;

use helikodu_library::{AnalysisOutcome, DatabaseConfig, Library};
use helikodu_model::analysis::TrackAnalysis;
use helikodu_model::library::TrackRef;
use helikodu_model::player::QueueContext;
use helikodu_model::settings::TransitionSettings;
use helikodu_service::library::MusicLibrary;
use helikodu_service::player::{PlayerCommand, PlayerEvent, SinkPacing};

use crate::media::{RATE, folder, player_with, read_wav, samples, until, wav_output, write_wav};
use crate::stats::add;

/// The silence kept around a trimmed track's sound: 10 ms.
const MARGIN: usize = RATE as usize / 100;

fn play(trim: bool, tag: &str) -> (Vec<i16>, Vec<i16>, std::ops::Range<usize>) {
    let dir = folder(tag);
    let path = dir.join("track.wav");
    let (lead, sound, tail) = (RATE as usize / 2, RATE as usize, RATE as usize / 4);
    let mut signal = vec![0; lead * 2];
    signal.extend(samples(sound, 7));
    signal.extend(vec![0; tail * 2]);
    write_wav(&path, &signal);
    let library = Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap());
    let track = add(&library, &dir, &path, 1750);
    let frames = (lead + sound + tail) as u64;
    let analysis = TrackAnalysis {
        rate: RATE,
        frames,
        audio: Some(lead as u64..(lead + sound) as u64),
        ..TrackAnalysis::default()
    };
    library
        .store_analysis(track, 1, AnalysisOutcome::Analyzed(analysis))
        .unwrap();
    let out = dir.join("out.wav");
    let (player, events) = player_with(wav_output(&out), SinkPacing::Freewheel, |config| {
        config.library = Some(MusicLibrary::from_library(Arc::clone(&library), ":memory:"));
        config.transitions = TransitionSettings {
            trim_silence: trim,
            ..TransitionSettings::default()
        };
    });
    player
        .send(PlayerCommand::PlayTracks {
            context: QueueContext::files(),
            tracks: vec![TrackRef::Library(track)],
            start: 0,
        })
        .unwrap();
    until(&events, |e| matches!(e, PlayerEvent::Ended { .. }));
    drop(player);
    (read_wav(&out), signal, lead - MARGIN..lead + sound + MARGIN)
}

#[test]
fn the_stored_analysis_trims_a_library_track() {
    let (written, signal, kept) = play(true, "analysis_trim");
    assert!(written == signal[kept.start * 2..kept.end * 2]);
    let (written, signal, _) = play(false, "analysis_whole");
    assert!(written == signal);
}
