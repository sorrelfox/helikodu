// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What playback makes of the track analysis: crossfades whose overlap comes from
//! it, silence trimmed where tracks join, long silence inside a track skipped, and
//! none of it between tracks of one album or on a bit-perfect path.

use std::path::Path;
use std::sync::Arc;

use helikodu_engine::analysis::analyze_source;
use helikodu_engine::decode::DecoderRegistry;
use helikodu_engine::pipeline::{LoudnessMode, LoudnessSettings, equal_power};
use helikodu_engine::transport::EngineConfig;
use helikodu_model::analysis::{SilentSpan, TrackAnalysis};
use helikodu_model::audio::{MediaTime, OutputMode, SampleRate};
use helikodu_model::library::MediaSource;
use helikodu_model::player::{EngineCommand, EngineEvent, PlaybackItem, TransportState};
use helikodu_model::settings::{OutputFormat, TransitionSettings};

use crate::media::{self, Signal};
use crate::session::{Session, item, wav_config};

const RATE: u32 = 44_100;
const FULL: f64 = 32_768.0;
/// The silence kept around the sound of a trimmed track: 10 ms.
const MARGIN: usize = 441;

fn frames(seconds: f64) -> usize {
    (seconds * f64::from(RATE)).round() as usize
}

/// A constant level on each channel.
fn dc(left: f64, right: f64, frames: usize) -> Signal {
    let to = |x: f64| (x * FULL).round() as i16;
    Signal {
        rate: RATE,
        samples: (0..frames).flat_map(|_| [to(left), to(right)]).collect(),
    }
}

fn silence(rate: u32, frames: usize) -> Signal {
    Signal {
        rate,
        samples: vec![0; frames * 2],
    }
}

fn concat(parts: &[Signal]) -> Signal {
    Signal {
        rate: parts[0].rate,
        samples: parts
            .iter()
            .flat_map(|p| p.samples.iter().copied())
            .collect(),
    }
}

/// The analysis the engine's analyser makes of the file at `path`.
fn analyse(path: &Path) -> Arc<TrackAnalysis> {
    let registry = DecoderRegistry::default();
    let analysed = analyze_source(&registry, &MediaSource::file(path), false, &mut || false);
    Arc::new(analysed.unwrap().analysis)
}

fn analysed(id: u64, path: &Path) -> PlaybackItem {
    item(id, path).with_analysis(Some(analyse(path)))
}

fn config(out: &Path, transitions: TransitionSettings, float: bool) -> EngineConfig {
    let mut config = wav_config(out).with_transitions(transitions);
    if float {
        config.output.format = OutputFormat::F32;
    }
    config
}

fn crossfading(millis: u32) -> TransitionSettings {
    TransitionSettings {
        crossfade: true,
        crossfade_ms: millis,
        ..TransitionSettings::default()
    }
}

fn trimming() -> TransitionSettings {
    TransitionSettings {
        trim_silence: true,
        ..TransitionSettings::default()
    }
}

fn play(config: EngineConfig, items: &[PlaybackItem]) -> Session {
    let mut session = Session::new(config);
    session.play_queue(items);
    session.wait_state(TransportState::Stopped);
    session
}

/// The interleaved samples of `signal` from frame `from` to frame `to`.
fn part(signal: &Signal, from: usize, to: usize) -> Vec<i16> {
    signal.samples[from * 2..to * 2].to_vec()
}

/// The positions sent for `item`, in frames.
fn positions(session: &Session, item: u64, rate: u32) -> Vec<u64> {
    let rate = SampleRate::new(rate).unwrap();
    session
        .log
        .iter()
        .filter_map(|e| match e {
            EngineEvent::Position { item: i, position } if i.get() == item => {
                Some(position.frames_at(rate))
            }
            _ => None,
        })
        .collect()
}

/// Track A at (0.5, 0) for 1.5 s with 0.5 s of silence after it, fading out (as
/// its analysis says) from 1.2 s; track B at (0, 0.5) for 1 s after 0.4 s of
/// silence.
fn overlap_tracks(tag: &str) -> (PlaybackItem, PlaybackItem, Signal, Signal) {
    let a = concat(&[dc(0.5, 0.0, frames(1.5)), silence(RATE, frames(0.5))]);
    let b = concat(&[silence(RATE, frames(0.4)), dc(0.0, 0.5, frames(1.0))]);
    let path_a = media::write_wav(&format!("{tag}-a.wav"), &a);
    let path_b = media::write_wav(&format!("{tag}-b.wav"), &b);
    let analysis_a = TrackAnalysis {
        rate: RATE,
        frames: a.frames() as u64,
        audio: Some(0..frames(1.5) as u64),
        fade_out_start: Some(frames(1.2) as u64),
        ..TrackAnalysis::default()
    };
    let analysis_b = TrackAnalysis {
        rate: RATE,
        frames: b.frames() as u64,
        audio: Some(frames(0.4) as u64..b.frames() as u64),
        ..TrackAnalysis::default()
    };
    (
        item(1, &path_a).with_analysis(Some(Arc::new(analysis_a))),
        item(2, &path_b).with_analysis(Some(Arc::new(analysis_b))),
        a,
        b,
    )
}

#[test]
fn the_overlap_comes_from_the_analysis() {
    let (a, b, _, signal_b) = overlap_tracks("overlap");
    let out = media::path("overlap-out.wav");
    drop(play(config(&out, crossfading(2000), true), &[a, b]));
    let written = media::read_output(&out).floats;
    let frame = |i: usize| (f64::from(written[2 * i]), f64::from(written[2 * i + 1]));

    // A ends 10 ms after its sound, B starts 10 ms before its sound, and the
    // window runs from A's fade to A's end: 0.3 s and the margin.
    let end_a = frames(1.5) + MARGIN;
    let length = frames(1.5) - frames(1.2) + MARGIN;
    let window = end_a - length;
    let start_b = frames(0.4) - MARGIN;
    assert_eq!(written.len() / 2, window + (signal_b.frames() - start_b));
    for i in window - 100..window {
        assert_eq!(frame(i), (0.5, 0.0), "frame {i} before the crossfade");
    }
    for k in 0..length {
        let (l, r) = frame(window + k);
        let (outgoing, incoming) = equal_power(k as u64, length as u64);
        let a = if window + k < frames(1.5) { 0.5 } else { 0.0 };
        let b = if k < MARGIN { 0.0 } else { 0.5 };
        assert!(
            (l - a * outgoing).abs() < 1e-6 && (r - b * incoming).abs() < 1e-6,
            "frame {k} of the crossfade: ({l}, {r})"
        );
    }
    for i in window + length..written.len() / 2 {
        assert_eq!(frame(i), (0.0, 0.5), "frame {i} after the crossfade");
    }
}

#[test]
fn without_an_analysis_the_crossfade_keeps_its_length() {
    let (a, b, signal_a, signal_b) = overlap_tracks("fixed");
    // The fixed crossfade: half of the shorter track, here less than the 2 s.
    let length = signal_b.frames() / 2;
    let expected = signal_a.frames() + signal_b.frames() - length;
    for (tag, items, transitions) in [
        (
            "one-side",
            [a.clone(), b.clone().with_analysis(None)],
            crossfading(2000),
        ),
        (
            "other-side",
            [a.clone().with_analysis(None), b.clone()],
            crossfading(2000),
        ),
        (
            "setting-off",
            [a.clone(), b.clone()],
            TransitionSettings {
                crossfade_from_analysis: false,
                ..crossfading(2000)
            },
        ),
    ] {
        let out = media::path(&format!("fixed-{tag}.wav"));
        drop(play(config(&out, transitions, true), &items));
        assert_eq!(media::read_output(&out).frames(), expected, "{tag}");
    }
}

#[test]
fn tracks_of_one_album_are_never_trimmed_or_crossfaded() {
    let signal = concat(&[
        silence(RATE, frames(0.3)),
        media::chirp(RATE, frames(1.0)),
        silence(RATE, frames(0.6)),
        media::chirp(RATE, frames(1.0)),
        silence(RATE, frames(0.2)),
    ]);
    let cut = frames(1.6);
    let (a, b) = (signal.slice(0, cut), signal.slice(cut, signal.frames()));
    let path_a = media::write_wav("album-trim-a.wav", &a);
    let path_b = media::write_wav("album-trim-b.wav", &b);
    let album = Some("Album\u{1f}Artist".to_owned());
    let items = [
        analysed(1, &path_a).with_album(album.clone()),
        analysed(2, &path_b).with_album(album),
    ];
    assert!(items.iter().all(|i| {
        let analysis = i.analysis.as_deref().unwrap();
        analysis.leading_silence() > 0 && analysis.trailing_silence() > 0
    }));
    let transitions = TransitionSettings {
        trim_silence: true,
        ..crossfading(500)
    };
    let out = media::path("album-trim-out.wav");
    drop(play(config(&out, transitions, false), &items));
    let written = media::read_output(&out).ints;
    // Bit for bit, the silence between the two included. Only the very start
    // of the run and its end are trimmed.
    let first = items[0].analysis.as_deref().unwrap();
    let last = items[1].analysis.as_deref().unwrap();
    let from = first.audio.as_ref().unwrap().start as usize - MARGIN;
    let to = cut + last.audio.as_ref().unwrap().end as usize + MARGIN;
    assert!(written == part(&signal, from, to));
}

#[test]
fn silence_is_trimmed_where_tracks_join() {
    let a = concat(&[
        silence(RATE, frames(0.5)),
        media::chirp(RATE, frames(1.0)),
        silence(RATE, frames(0.7)),
    ]);
    let b = concat(&[
        silence(RATE, frames(0.3)),
        media::chirp(RATE, frames(1.0)),
        silence(RATE, frames(0.2)),
    ]);
    let path_a = media::write_wav("trim-a.wav", &a);
    let path_b = media::write_wav("trim-b.wav", &b);
    let items = [analysed(1, &path_a), analysed(2, &path_b)];
    let out = media::path("trim-out.wav");
    let session = play(config(&out, trimming(), false), &items);

    let sound = |i: usize| items[i].analysis.as_deref().unwrap().audio.clone().unwrap();
    let (sound_a, sound_b) = (sound(0), sound(1));
    let a_from = sound_a.start as usize - MARGIN;
    let mut expected = part(&a, a_from, sound_a.end as usize + MARGIN);
    expected.extend(part(
        &b,
        sound_b.start as usize - MARGIN,
        sound_b.end as usize + MARGIN,
    ));
    let written = media::read_output(&out).ints;
    assert_eq!(written.len(), expected.len());
    assert!(
        written == expected,
        "the trimmed tracks are not joined exactly"
    );

    // Positions are those of the file: the first one is where the sound starts.
    let first = positions(&session, 1, RATE);
    assert_eq!(first.first().copied(), Some(a_from as u64), "{first:?}");
    assert!(first.iter().all(|&p| p >= a_from as u64));
    let lengths: Vec<_> = session
        .log
        .iter()
        .filter_map(|e| match e {
            EngineEvent::TrackStarted { length, .. } => *length,
            _ => None,
        })
        .collect();
    let rate = SampleRate::new(RATE).unwrap();
    assert_eq!(
        lengths,
        [
            MediaTime::from_frames(a.frames() as u64, rate),
            MediaTime::from_frames(b.frames() as u64, rate)
        ],
        "the lengths are the files'"
    );
    drop(session);

    // Off by default: nothing is trimmed.
    let out = media::path("trim-off-out.wav");
    drop(play(
        config(&out, TransitionSettings::default(), false),
        &items,
    ));
    assert_eq!(media::read_output(&out).frames(), a.frames() + b.frames());
}

const SKIP_RATE: u32 = 8_000;

/// 16 s of sound, 12 s of silence, 2 s of sound, at 8 kHz.
fn hidden_track() -> (Signal, PlaybackItem, SilentSpan) {
    let s = |seconds: f64| (seconds * f64::from(SKIP_RATE)) as usize;
    let signal = concat(&[
        media::sine(SKIP_RATE, s(16.0)),
        silence(SKIP_RATE, s(12.0)),
        media::sine(SKIP_RATE, s(2.0)),
    ]);
    let path = media::write_wav("hidden.wav", &signal);
    let item = analysed(1, &path);
    let span = item.analysis.as_deref().unwrap().silences[0];
    assert!(span.len() as usize > s(11.0), "{span:?}");
    (signal, item, span)
}

fn skipping() -> TransitionSettings {
    TransitionSettings {
        skip_silence: true,
        skip_silence_min_s: 10,
        ..TransitionSettings::default()
    }
}

#[test]
fn long_silence_inside_a_track_is_skipped() {
    let (signal, item, span) = hidden_track();
    let keep = SKIP_RATE as usize;
    let (from, to) = (span.start as usize + keep, span.end as usize - keep);
    let out = media::path("hidden-out.wav");
    let session = play(config(&out, skipping(), false), std::slice::from_ref(&item));
    let mut expected = part(&signal, 0, from);
    expected.extend(part(&signal, to, signal.frames()));
    let written = media::read_output(&out).ints;
    assert_eq!(written.len(), expected.len());
    assert!(written == expected, "the jump is not at the right frames");
    // No position lies in the stretch jumped over.
    let skipped = positions(&session, 1, SKIP_RATE);
    assert!(
        skipped.iter().all(|&p| p <= from as u64 || p >= to as u64),
        "{skipped:?}"
    );
    drop(session);

    // Shorter stretches than the setting asks for stay.
    let out = media::path("hidden-kept-out.wav");
    let transitions = TransitionSettings {
        skip_silence_min_s: 30,
        ..skipping()
    };
    drop(play(config(&out, transitions, false), &[item]));
    assert_eq!(media::read_output(&out).frames(), signal.frames());
}

/// Loads `item` paused, seeks to `seconds`, and plays to the end. Returns the
/// position reported after the seek and the frames written.
fn seek_and_play(item: &PlaybackItem, seconds: f64, tag: &str) -> (u64, usize) {
    let out = media::path(&format!("hidden-seek-{tag}.wav"));
    let mut session = Session::new(config(&out, skipping(), false));
    session.send(EngineCommand::Load {
        item: item.clone(),
        start: MediaTime::ZERO,
        paused: true,
    });
    session.wait_state(TransportState::Paused);
    let target = MediaTime::from_duration(std::time::Duration::from_secs_f64(seconds));
    session.send(EngineCommand::Seek(target));
    session.wait_state(TransportState::Seeking);
    session.wait_state(TransportState::Paused);
    let position = session.wait_position(SKIP_RATE);
    session.send(EngineCommand::Play);
    session.wait_state(TransportState::Stopped);
    drop(session);
    (position, media::read_output(&out).frames())
}

#[test]
fn seeks_and_positions_stay_in_file_time_around_a_skip() {
    let (signal, item, span) = hidden_track();
    let keep = SKIP_RATE as u64;
    let skipped = (span.end - keep) - (span.start + keep);
    let total = signal.frames() as u64;
    let s = |seconds: f64| (seconds * f64::from(SKIP_RATE)) as u64;
    // Before the stretch: the position is the file's, and the jump still comes.
    let (position, written) = seek_and_play(&item, 5.0, "before");
    assert_eq!(position, s(5.0));
    assert_eq!(written as u64, total - s(5.0) - skipped);
    // Into the stretch: playback goes where it was asked, silence and all.
    let (position, written) = seek_and_play(&item, 20.0, "inside");
    assert_eq!(position, s(20.0));
    assert_eq!(written as u64, total - s(20.0));
    // After it: nothing more to skip.
    let (position, written) = seek_and_play(&item, 29.0, "after");
    assert_eq!(position, s(29.0));
    assert_eq!(written as u64, total - s(29.0));
}

#[test]
fn a_load_from_the_start_skips_the_leading_silence_only_when_trimming() {
    let signal = concat(&[silence(RATE, frames(0.8)), media::chirp(RATE, frames(1.0))]);
    let path = media::write_wav("lead.wav", &signal);
    let item = analysed(7, &path);
    let start = item.analysis.as_deref().unwrap().leading_silence() as usize - MARGIN;
    for (tag, transitions, from) in [
        ("on", trimming(), start),
        ("off", TransitionSettings::default(), 0),
    ] {
        let out = media::path(&format!("lead-{tag}.wav"));
        let mut session = Session::new(config(&out, transitions, false));
        session.load(&item, MediaTime::ZERO);
        session.wait_state(TransportState::Stopped);
        let first = positions(&session, 7, RATE);
        assert_eq!(
            first.first().copied(),
            Some(from as u64),
            "{tag}: {first:?}"
        );
        drop(session);
        let written = media::read_output(&out).ints;
        assert!(written == part(&signal, from, signal.frames()), "{tag}");
    }
}

#[test]
fn bit_perfect_output_turns_it_all_off() {
    let a = concat(&[
        silence(RATE, frames(0.4)),
        media::chirp(RATE, frames(1.0)),
        silence(RATE, frames(0.5)),
    ]);
    let b = concat(&[silence(RATE, frames(0.3)), media::chirp(RATE, frames(1.0))]);
    let path_a = media::write_wav("bitperfect-a.wav", &a);
    let path_b = media::write_wav("bitperfect-b.wav", &b);
    let items = [analysed(1, &path_a), analysed(2, &path_b)];
    let transitions = TransitionSettings {
        trim_silence: true,
        skip_silence: true,
        ..crossfading(500)
    };
    let run = |mode: OutputMode| {
        let out = media::path(&format!("bitperfect-{mode}.wav"));
        let mut config = config(&out, transitions, false);
        config.output.mode = mode;
        // The leveller on, and nothing else that changes the samples.
        config.processing.loudness = LoudnessSettings {
            mode: LoudnessMode::Auto,
            untagged_preamp_db: 0.0,
            leveller: true,
            ..LoudnessSettings::default()
        };
        let session = play(config, &items);
        let path: Vec<String> = session
            .log
            .iter()
            .filter_map(|e| match e {
                EngineEvent::SignalPath(Some(path)) => Some(format!("{path:?}")),
                _ => None,
            })
            .collect();
        drop(session);
        (media::read_output(&out).ints, path)
    };
    // Exclusive and bit-perfect: the files as they are, one after the other.
    let (written, path) = run(OutputMode::Exclusive);
    assert!(written == concat(&[a.clone(), b.clone()]).samples);
    assert!(path.iter().all(|p| !p.contains("leveller")), "{path:?}");
    // Shared: trimmed, crossfaded and levelled.
    let (written, path) = run(OutputMode::Shared);
    assert!(written.len() / 2 < a.frames() + b.frames() - frames(0.6));
    assert!(path.iter().any(|p| p.contains("leveller")), "{path:?}");
}
