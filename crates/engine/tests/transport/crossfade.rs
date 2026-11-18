// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Crossfades between tracks and on skips, their policies, and the fade lengths of
//! the settings.

use std::path::{Path, PathBuf};
use std::time::Duration;

use helikodu_engine::output::Pacing;

use helikodu_engine::pipeline::equal_power;
use helikodu_engine::transport::EngineConfig;
use helikodu_model::audio::{MediaTime, SampleRate};
use helikodu_model::player::{
    EngineCommand, EngineEvent, PlaybackItem, TrackEndReason, TransportState,
};
use helikodu_model::settings::{OutputFormat, SampleRatePolicy, TransitionSettings};

use crate::media::{self, Signal};
use crate::session::{Session, item, wav_config};

const RATE: u32 = 44_100;
/// Full scale of the 16-bit test files.
const FULL: f64 = 32_768.0;

/// A constant level on each channel: the gains of a crossfade show as they are.
fn dc(left: f64, right: f64, frames: usize) -> Signal {
    let to = |x: f64| (x * FULL).round() as i16;
    let samples = (0..frames).flat_map(|_| [to(left), to(right)]).collect();
    Signal {
        rate: RATE,
        samples,
    }
}

fn frames_of(millis: u32) -> usize {
    media::fade_frames(millis, RATE)
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

/// Plays `items` as a queue and returns the session once it has stopped.
fn play(config: EngineConfig, items: &[PlaybackItem]) -> Session {
    let mut session = Session::new(config);
    session.play_queue(items);
    session.wait_state(TransportState::Stopped);
    session
}

fn files(tag: &str, a: &Signal, b: &Signal) -> (PathBuf, PathBuf) {
    (
        media::write_wav(&format!("{tag}-a.wav"), a),
        media::write_wav(&format!("{tag}-b.wav"), b),
    )
}

/// The starts and ends of items, in order.
fn starts_and_ends(session: &Session) -> Vec<(u64, Option<TrackEndReason>)> {
    session
        .milestones()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::TrackStarted { item, .. } => Some((item.get(), None)),
            EngineEvent::TrackEnded { item, reason } => Some((item.get(), Some(reason))),
            _ => None,
        })
        .collect()
}

/// Checks the crossfade of `length` frames at `start` of float output `out`:
/// item A at (0.5, 0) going out, item B at (0, 0.5) coming in.
fn assert_crossfade(out: &[f32], start: usize, length: usize) {
    let frame = |i: usize| (f64::from(out[2 * i]), f64::from(out[2 * i + 1]));
    for i in start.saturating_sub(100)..start {
        assert_eq!(frame(i), (0.5, 0.0), "frame {i} before the crossfade");
    }
    for k in 0..length {
        let (l, r) = frame(start + k);
        let (outgoing, incoming) = equal_power(k as u64, length as u64);
        assert!(
            (l - 0.5 * outgoing).abs() < 1e-6 && (r - 0.5 * incoming).abs() < 1e-6,
            "frame {k} of the crossfade: ({l}, {r})"
        );
        // Equal power: the levels' squares add up to the level of either track.
        assert!((l * l + r * r - 0.25).abs() < 1e-6, "frame {k}: power");
    }
    for i in start + length..(start + length + 100).min(out.len() / 2) {
        assert_eq!(frame(i), (0.0, 0.5), "frame {i} after the crossfade");
    }
}

#[test]
fn a_crossfade_has_the_exact_length_and_keeps_the_power() {
    let (a, b) = (dc(0.5, 0.0, RATE as usize), dc(0.0, 0.5, RATE as usize));
    let (path_a, path_b) = files("xfade", &a, &b);
    let out = media::path("xfade-out.wav");
    let session = play(
        config(&out, crossfading(200), true),
        &[item(1, &path_a), item(2, &path_b)],
    );
    let length = frames_of(200);
    let finished = Some(TrackEndReason::Finished);
    assert_eq!(
        starts_and_ends(&session),
        [(1, None), (1, finished), (2, None), (2, finished)],
        "{:#?}",
        session.log
    );
    // The incoming track is heard from the middle of the crossfade, with its
    // position counted from its first frame.
    let started = session
        .log
        .iter()
        .position(|e| matches!(e, EngineEvent::TrackStarted { item, .. } if item.get() == 2))
        .unwrap();
    let first = session.log[started..].iter().find_map(|e| match e {
        EngineEvent::Position { item, position } if item.get() == 2 => Some(*position),
        _ => None,
    });
    let rate = SampleRate::new(RATE).unwrap();
    // (Freewheeling, the whole queue may play between two position ticks.)
    assert!(first.is_none_or(|p| p.frames_at(rate) >= (length / 2) as u64));
    drop(session);

    let written = media::read_output(&out);
    assert_eq!(written.frames(), a.frames() + b.frames() - length);
    assert_crossfade(&written.floats, a.frames() - length, length);
}

#[test]
fn crossfades_do_not_click() {
    let sine = media::sine(RATE, 4 * RATE as usize);
    // The incoming track starts on another phase of the sine.
    let a = sine.slice(0, 2 * RATE as usize);
    let b = sine.slice(RATE as usize / 3, 3 * RATE as usize);
    let (path_a, path_b) = files("xfade-sine", &a, &b);
    let out = media::path("xfade-sine-out.wav");
    drop(play(
        config(&out, crossfading(500), false),
        &[item(1, &path_a), item(2, &path_b)],
    ));
    let written = media::read_output(&out).ints;
    assert_eq!(written.len() / 2, a.frames() + b.frames() - frames_of(500));
    // Two sines mixed at equal power move at most √2 times as fast as one.
    let limit = 1.5 * media::sine_max_step(RATE);
    let step = media::max_step(&written);
    assert!(step <= limit, "a step of {step} (limit {limit})");
}

#[test]
fn tracks_of_one_album_join_without_a_crossfade() {
    let signal = media::chirp(RATE, 2 * RATE as usize);
    let cut = 50_001;
    let (a, b) = (signal.slice(0, cut), signal.slice(cut, signal.frames()));
    let (path_a, path_b) = files("album", &a, &b);
    let album = Some("Album\u{1f}Artist".to_owned());
    let items = [
        item(1, &path_a).with_album(album.clone()),
        item(2, &path_b).with_album(album),
    ];
    let run = |tag: &str, transitions: TransitionSettings| {
        let out = media::path(&format!("album-{tag}.wav"));
        drop(play(config(&out, transitions, false), &items));
        media::read_output(&out).ints
    };
    // Kept gapless: bit-exact, as with crossfades off.
    assert!(run("kept", crossfading(300)) == signal.samples);
    assert!(run("off", TransitionSettings::default()) == signal.samples);
    // Without the policy, the album is crossfaded too.
    let crossfaded = run(
        "crossfaded",
        TransitionSettings {
            keep_albums_gapless: false,
            ..crossfading(300)
        },
    );
    assert_eq!(crossfaded.len() / 2, signal.frames() - frames_of(300));
}

#[test]
fn a_skip_crossfades_into_the_new_track() {
    let (a, b) = (dc(0.5, 0.0, 3 * RATE as usize), dc(0.0, 0.5, RATE as usize));
    let (path_a, path_b) = files("skip-xfade", &a, &b);
    let out = media::path("skip-xfade-out.wav");
    let transitions = TransitionSettings {
        skip_crossfade: true,
        skip_crossfade_ms: 100,
        ..TransitionSettings::default()
    };
    let mut session = Session::new(config(&out, transitions, true));
    session.load(&item(1, &path_a), MediaTime::ZERO);
    session.wait_state(TransportState::Playing);
    session.load(&item(2, &path_b), MediaTime::ZERO);
    session.wait_state(TransportState::Stopped);
    let interrupted = Some(TrackEndReason::Interrupted);
    let finished = Some(TrackEndReason::Finished);
    assert_eq!(
        starts_and_ends(&session),
        [(1, None), (1, interrupted), (2, None), (2, finished)],
        "{:#?}",
        session.log
    );
    let loading = session
        .log
        .iter()
        .filter(|e| **e == EngineEvent::StateChanged(TransportState::Loading))
        .count();
    assert_eq!(loading, 1, "a crossfaded skip loads nothing more");
    drop(session);

    let written = media::read_output(&out);
    let start = written.frames() - b.frames();
    assert!(start > 0 && start < a.frames() / 2, "{start}");
    assert_crossfade(&written.floats, start, frames_of(100));
}

/// The fades in the left channel of 16-bit output from a signal at half scale:
/// whether each rises, and its length, from the frames between three and one
/// quarters of the level (a third of a raised cosine).
fn fades(out: &[i16]) -> Vec<(bool, usize)> {
    let mut found = Vec::new();
    let mut high = true;
    let mut mark = None;
    for (i, frame) in out.as_chunks::<2>().0.iter().enumerate() {
        let level = f64::from(frame[0]) / (0.5 * FULL);
        if high {
            if level < 0.75 && mark.is_none() {
                mark = Some(i);
            }
            if level < 0.25 {
                found.push((false, 3 * (i - mark.unwrap_or(i))));
                (high, mark) = (false, None);
            }
        } else {
            if level > 0.25 && mark.is_none() {
                mark = Some(i);
            }
            if level > 0.75 {
                found.push((true, 3 * (i - mark.unwrap_or(i))));
                (high, mark) = (true, None);
            }
        }
    }
    found
}

fn assert_fades(found: &[(bool, usize)], expected: &[(bool, u32)]) {
    assert_eq!(found.len(), expected.len(), "{found:?}");
    for (&(rising, length), &(want_rising, millis)) in found.iter().zip(expected) {
        let want = frames_of(millis);
        assert!(
            rising == want_rising && length.abs_diff(want) <= 6,
            "{found:?} instead of {expected:?}"
        );
    }
}

#[test]
fn pause_seek_and_stop_fade_as_the_settings_say() {
    let signal = dc(0.5, 0.5, 3 * RATE as usize);
    let path = media::write_wav("fades-in.wav", &signal);
    let out = media::path("fades-out.wav");
    let transitions = TransitionSettings {
        pause_fade_ms: 100,
        seek_fade_ms: 40,
        stop_fade_ms: 200,
        ..TransitionSettings::default()
    };
    // In real time, so that each fade is heard to its end before the next command.
    let mut config = config(&out, transitions, false);
    config.sink_pacing = Pacing::RealTime;
    let settle = || std::thread::sleep(Duration::from_millis(300));
    let mut session = Session::new(config);
    session.load(&item(1, &path), MediaTime::ZERO);
    settle();
    session.send(EngineCommand::Pause);
    session.wait_state(TransportState::Paused);
    session.send(EngineCommand::Play);
    session.wait_state(TransportState::Playing);
    settle();
    session.send(EngineCommand::Seek(MediaTime::from_millis(2000)));
    session.wait_state(TransportState::Seeking);
    session.wait_state(TransportState::Playing);
    settle();
    session.send(EngineCommand::Stop);
    session.wait_state(TransportState::Stopped);
    drop(session);
    assert_fades(
        &fades(&media::read_output(&out).ints),
        &[
            (false, 100),
            (true, 100),
            (false, 40),
            (true, 40),
            (false, 200),
        ],
    );
}

#[test]
fn a_skip_without_a_crossfade_fades_out_as_the_settings_say() {
    let a = dc(0.5, 0.5, 3 * RATE as usize);
    let b = dc(-0.5, -0.5, RATE as usize);
    let (path_a, path_b) = files("skip-fade", &a, &b);
    let out = media::path("skip-fade-out.wav");
    let transitions = TransitionSettings {
        skip_fade_ms: 30,
        ..TransitionSettings::default()
    };
    let mut session = Session::new(config(&out, transitions, false));
    session.load(&item(1, &path_a), MediaTime::ZERO);
    session.wait_state(TransportState::Playing);
    session.load(&item(2, &path_b), MediaTime::ZERO);
    session.wait_state(TransportState::Stopped);
    drop(session);
    let written = media::read_output(&out).ints;
    // The new track starts at full level, as every track from its beginning.
    let start = written.len() - b.samples.len();
    assert!(written[start..] == b.samples[..]);
    assert_fades(&fades(&written[..start]), &[(false, 30)]);
}

#[test]
fn transitions_change_while_playing() {
    let (a, b) = (dc(0.5, 0.0, RATE as usize), dc(0.0, 0.5, RATE as usize));
    let (path_a, path_b) = files("live", &a, &b);
    let out = media::path("live-out.wav");
    let mut session = Session::new(config(&out, TransitionSettings::default(), true));
    session.play_queue(&[item(1, &path_a), item(2, &path_b)]);
    session.wait_state(TransportState::Playing);
    session.send(EngineCommand::SetTransitions(crossfading(250)));
    session.wait_state(TransportState::Stopped);
    drop(session);
    let written = media::read_output(&out);
    let length = frames_of(250);
    assert_eq!(written.frames(), a.frames() + b.frames() - length);
    assert_crossfade(&written.floats, a.frames() - length, length);
}

#[test]
fn a_resampled_crossfade_has_the_exact_length() {
    let (a, b) = (dc(0.5, 0.0, RATE as usize), dc(0.0, 0.5, RATE as usize));
    let (path_a, path_b) = files("xfade-48k", &a, &b);
    let out = media::path("xfade-48k-out.wav");
    let mut config = config(&out, crossfading(200), true);
    config.output.sample_rate = SampleRatePolicy::Fixed(SampleRate::HZ_48000);
    drop(play(config, &[item(1, &path_a), item(2, &path_b)]));
    let written = media::read_output(&out);
    assert_eq!(written.rate, 48_000);
    // Each second becomes 48 000 frames; the crossfade takes 200 ms of them.
    let length = 9600;
    assert_eq!(written.frames(), 2 * 48_000 - length);
    let start = 48_000 - length;
    // Away from the ringing of the incoming track's first edge, the power holds.
    for k in 1000..length - 1000 {
        let (l, r) = (
            f64::from(written.floats[2 * (start + k)]),
            f64::from(written.floats[2 * (start + k) + 1]),
        );
        assert!((l * l + r * r - 0.25).abs() < 2e-3, "frame {k}: ({l}, {r})");
    }
}

#[test]
fn another_stream_fades_out_and_in_instead() {
    let a = dc(0.5, 0.0, RATE as usize);
    let b = Signal {
        rate: 48_000,
        ..dc(0.0, 0.5, 48_000)
    };
    let (path_a, path_b) = files("fade-through", &a, &b);
    let out = media::path("fade-through-out.wav");
    let session = play(
        config(&out, crossfading(100), true),
        &[item(1, &path_a), item(2, &path_b)],
    );
    let reconfiguring = session
        .log
        .contains(&EngineEvent::StateChanged(TransportState::Reconfiguring));
    assert!(reconfiguring, "{:#?}", session.log);
    drop(session);
    // The stream reopened at 48 kHz holds the second track, faded in over 100 ms.
    let written = media::read_output(&out);
    assert_eq!(written.rate, 48_000);
    assert_eq!(written.frames(), b.frames());
    let right = |i: usize| written.floats[2 * i + 1];
    assert!(right(0) < 0.01);
    assert!((right(2400) - 0.25).abs() < 0.01, "{}", right(2400));
    assert_eq!(right(4900), 0.5);
}

#[test]
fn a_pause_during_a_crossfade_goes_on_without_a_click() {
    let sine = media::sine(RATE, 3 * RATE as usize);
    let a = sine.slice(0, RATE as usize);
    let b = sine.slice(RATE as usize / 3, 2 * RATE as usize);
    let (path_a, path_b) = files("xfade-pause", &a, &b);
    // Before and after the middle of a crossfade from 0.4 s to 1 s.
    for tag in ["before", "after"] {
        let out = media::path(&format!("xfade-pause-{tag}.wav"));
        let mut config = config(&out, crossfading(600), false);
        config.sink_pacing = Pacing::RealTime;
        let mut session = Session::new(config);
        session.play_queue(&[item(1, &path_a), item(2, &path_b)]);
        // Waiting for events serves `NearEnd` on the way.
        let into_window = MediaTime::from_millis(550);
        session.wait_for(|e| match e {
            EngineEvent::Position { item, position } if tag == "before" => {
                item.get() == 1 && *position >= into_window
            }
            EngineEvent::TrackStarted { item, .. } => tag == "after" && item.get() == 2,
            _ => false,
        });
        session.send(EngineCommand::Pause);
        session.wait_state(TransportState::Paused);
        session.send(EngineCommand::Play);
        session.wait_state(TransportState::Stopped);
        let finished = Some(TrackEndReason::Finished);
        assert_eq!(
            starts_and_ends(&session),
            [(1, None), (1, finished), (2, None), (2, finished)],
            "{tag}: {:#?}",
            session.log
        );
        drop(session);
        let mut written = media::read_output(&out).ints;
        // A sink in real time writes silence after the last track's (abrupt) end.
        while written.len() >= 2 && written[written.len() - 2..] == [0, 0] {
            written.truncate(written.len() - 2);
        }
        let limit = 1.5 * media::sine_max_step(RATE);
        let step = media::max_step(&written);
        assert!(step <= limit, "{tag}: a step of {step} (limit {limit})");
    }
}
