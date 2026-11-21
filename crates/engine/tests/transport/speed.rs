// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The playback speed: the output is shorter or longer by the speed, exactly; the
//! tempo kind keeps the pitch and the tape kind moves it; positions, seeks, the A-B
//! loop and crossfades stay right in the track's own time; a change while playing
//! goes on where playback was.

use std::f64::consts::TAU;
use std::path::Path;

use helikodu_engine::output::Pacing;
use helikodu_engine::transport::EngineConfig;
use helikodu_model::audio::{MediaTime, SampleRate};
use helikodu_model::player::{
    EngineCommand, EngineEvent, ItemId, LoopRange, TrackEndReason, TransportState,
};
use helikodu_model::settings::{
    LoudnessMode, LoudnessSettings, OutputFormat, SpeedSettings, TransitionSettings,
};

use crate::media::{self, Signal};
use crate::session::{Session, item, wav_config};

const RATE: u32 = 44_100;

fn rate() -> SampleRate {
    SampleRate::new(RATE).unwrap()
}

fn seconds(s: f64) -> usize {
    (s * f64::from(RATE)).round() as usize
}

fn to_i16(x: f64) -> i16 {
    (x * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
}

/// A sine of `hz` on both channels.
fn tone(hz: f64, frames: usize) -> Signal {
    let samples = (0..frames)
        .flat_map(|i| {
            let x = to_i16(0.5 * (TAU * hz * i as f64 / f64::from(RATE)).sin());
            [x, x]
        })
        .collect();
    Signal {
        rate: RATE,
        samples,
    }
}

/// Silence with a short 1 kHz burst at each of `at` (frames).
fn bursts(frames: usize, at: &[usize]) -> Signal {
    let length = seconds(0.03);
    let samples = (0..frames)
        .flat_map(|i| {
            let x = at
                .iter()
                .find(|&&a| (a..a + length).contains(&i))
                .map_or(0, |&a| {
                    to_i16(0.5 * (TAU * 1000.0 * (i - a) as f64 / f64::from(RATE)).sin())
                });
            [x, x]
        })
        .collect();
    Signal {
        rate: RATE,
        samples,
    }
}

/// A constant level on each channel.
fn dc(left: f64, right: f64, frames: usize) -> Signal {
    let samples = (0..frames)
        .flat_map(|_| [to_i16(left), to_i16(right)])
        .collect();
    Signal {
        rate: RATE,
        samples,
    }
}

/// The WAV sink at `out`, float, at `speed`, without loudness gain.
fn config(out: &Path, speed: f64, keep_pitch: bool) -> EngineConfig {
    let mut config = wav_config(out);
    config.output.format = OutputFormat::F32;
    config.dsp.speed = SpeedSettings::at(speed, keep_pitch);
    config.processing.loudness = LoudnessSettings {
        mode: LoudnessMode::Off,
        ..LoudnessSettings::default()
    }
    .into();
    config.processing = helikodu_engine::dsp::processing_settings(config.processing, &config.dsp);
    config
}

/// The frames `frames` source frames make at `speed` on a stream at the source rate:
/// the played rate is the source rate times the speed, to the hertz.
fn rendered(frames: usize, speed: f64) -> usize {
    let played = (f64::from(RATE) * speed).round() as u64;
    usize::try_from((frames as u64 * u64::from(RATE)).div_ceil(played)).unwrap()
}

/// The left channel of the float output at `out`.
fn left(out: &Path) -> Vec<f32> {
    let written = media::read_output(out);
    assert_eq!(written.rate, RATE);
    written.floats.iter().step_by(2).copied().collect()
}

/// The frequency of a tone, by its rising zero crossings.
fn frequency(x: &[f32]) -> f64 {
    let crossings = x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    crossings as f64 * f64::from(RATE) / x.len() as f64
}

/// Where the bursts of `x` begin: a loud sample after at least 50 ms of quiet.
fn onsets(x: &[f32]) -> Vec<usize> {
    let quiet = seconds(0.05);
    let mut onsets = Vec::new();
    let mut last_loud: Option<usize> = None;
    for (i, s) in x.iter().enumerate() {
        if s.abs() > 0.05 {
            if last_loud.is_none_or(|l| i - l > quiet) {
                onsets.push(i);
            }
            last_loud = Some(i);
        }
    }
    onsets
}

fn play_one(config: EngineConfig, path: &Path) -> Session {
    let mut session = Session::new(config);
    session.load(&item(1, path), MediaTime::ZERO);
    session.wait_state(TransportState::Stopped);
    session
}

#[test]
fn the_output_is_as_much_shorter_as_the_speed_is_higher() {
    let frames = 2 * RATE as usize + 77;
    let path = media::write_wav("speed-length-in.wav", &tone(440.0, frames));
    for (speed, keep_pitch) in [(1.5, true), (1.5, false), (0.75, true), (0.75, false)] {
        let out = media::path(&format!("speed-length-{speed}-{keep_pitch}.wav"));
        let session = play_one(config(&out, speed, keep_pitch), &path);
        let ended = session.log.iter().any(|e| {
            matches!(
                e,
                EngineEvent::TrackEnded {
                    reason: TrackEndReason::Finished,
                    ..
                }
            )
        });
        assert!(ended, "{:#?}", session.milestones());
        // Positions are the track's own: never past its length.
        let length = MediaTime::from_frames(frames as u64, rate());
        for event in &session.log {
            if let EngineEvent::Position { position, .. } = event {
                assert!(*position <= length, "{speed}: position {position:?}");
            }
        }
        drop(session);
        let x = left(&out);
        assert_eq!(x.len(), rendered(frames, speed), "{speed} {keep_pitch}");
        let middle = &x[x.len() / 4..3 * x.len() / 4];
        let f = frequency(middle);
        let expected = if keep_pitch { 440.0 } else { 440.0 * speed };
        assert!(
            (f - expected).abs() < expected * 0.01,
            "{speed} {keep_pitch}: {f} Hz, not {expected}"
        );
        let peak = middle.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            (0.4..0.6).contains(&peak),
            "{speed} {keep_pitch}: peak {peak}"
        );
    }
}

#[test]
fn a_seek_goes_to_the_track_time_asked_for() {
    // A burst at 1.5 s; the seek goes to 1.0 s.
    let path = media::write_wav("speed-seek-in.wav", &bursts(seconds(3.0), &[seconds(1.5)]));
    for (speed, keep_pitch) in [(1.5, true), (0.75, false)] {
        let out = media::path(&format!("speed-seek-{speed}.wav"));
        let mut session = Session::new(config(&out, speed, keep_pitch));
        session.send(EngineCommand::Load {
            item: item(1, &path),
            start: MediaTime::ZERO,
            paused: true,
        });
        session.wait_state(TransportState::Paused);
        let to = MediaTime::from_millis(1_000);
        session.send(EngineCommand::Seek(to));
        // The position of the load comes first, then the one of the seek.
        while session.wait_position(RATE) != to.frames_at(rate()) {}
        session.send(EngineCommand::Play);
        session.wait_state(TransportState::Stopped);
        drop(session);
        // The output starts at 1.0 s: the burst is 0.5 s of the track later, which
        // the speed makes 0.5 / speed seconds of listening.
        let found = onsets(&left(&out));
        let expected = seconds(0.5 / speed);
        assert_eq!(found.len(), 1, "{speed}: {found:?}");
        assert!(
            found[0].abs_diff(expected) < seconds(0.01),
            "{speed}: the burst at {}, not {expected}",
            found[0]
        );
    }
}

#[test]
fn the_position_heard_is_track_time() {
    let path = media::write_wav("speed-position-in.wav", &tone(440.0, seconds(4.0)));
    for (speed, keep_pitch) in [(1.5, true), (0.75, false)] {
        let out = media::path(&format!("speed-position-{speed}.wav"));
        let mut config = config(&out, speed, keep_pitch);
        config.sink_pacing = Pacing::RealTime;
        let mut session = Session::new(config);
        session.load(&item(1, &path), MediaTime::ZERO);
        while session.wait_position(RATE) < seconds(0.4) as u64 {}
        session.send(EngineCommand::Pause);
        session.wait_state(TransportState::Paused);
        // The position where the sound stopped.
        let paused = session.wait_position(RATE);
        drop(session);
        // The output holds what was heard up to there.
        let x = left(&out);
        let heard = x.iter().rposition(|s| s.abs() > 1e-4).unwrap_or(0);
        let expected = heard as f64 * speed;
        assert!(
            (paused as f64 - expected).abs() < f64::from(RATE) * 0.03,
            "{speed}: paused at {paused}, heard {heard} frames, {expected} expected"
        );
    }
}

#[test]
fn an_a_b_loop_repeats_its_track_time() {
    // A burst at 0.6 s inside a loop over 0.4 .. 1.0 s.
    let path = media::write_wav("speed-loop-in.wav", &bursts(seconds(3.0), &[seconds(0.6)]));
    for (speed, keep_pitch) in [(1.5, true), (0.75, false)] {
        let out = media::path(&format!("speed-loop-{speed}.wav"));
        let mut session = Session::new(config(&out, speed, keep_pitch));
        let (start, end) = (seconds(0.4) as u64, seconds(1.0) as u64);
        let range = LoopRange::new(
            MediaTime::from_frames(start, rate()),
            MediaTime::from_frames(end, rate()),
        )
        .unwrap();
        session.load(&item(1, &path), MediaTime::ZERO);
        session.send(EngineCommand::SetLoop {
            item: ItemId::new(1),
            range: Some(range),
        });
        let mut inside = 0;
        while inside < 4 {
            let position = session.wait_position(RATE);
            assert!(position < end, "{speed}: position {position} past the loop");
            inside += u32::from(position >= start);
        }
        session.send(EngineCommand::SetLoop {
            item: ItemId::new(1),
            range: None,
        });
        session.wait_state(TransportState::Stopped);
        drop(session);
        let found = onsets(&left(&out));
        assert!(found.len() >= 3, "{speed}: too few passes: {found:?}");
        assert!(
            found[0].abs_diff(seconds(0.6 / speed)) < seconds(0.01),
            "{speed}: {found:?}"
        );
        // Each pass lasts the loop's 0.6 s of track time at the speed.
        let pass = seconds(0.6 / speed);
        for pair in found.windows(2) {
            let gap = pair[1] - pair[0];
            assert!(
                gap.abs_diff(pass) < seconds(0.01),
                "{speed}: {found:?}, a pass of {gap}"
            );
        }
    }
}

#[test]
fn a_crossfade_lasts_its_listening_time() {
    let (a, b) = (dc(0.5, 0.0, RATE as usize), dc(0.0, 0.5, RATE as usize));
    let path_a = media::write_wav("speed-xfade-a.wav", &a);
    let path_b = media::write_wav("speed-xfade-b.wav", &b);
    let transitions = TransitionSettings {
        crossfade: true,
        crossfade_ms: 200,
        ..TransitionSettings::default()
    };
    let length = media::fade_frames(200, RATE);
    for (speed, keep_pitch) in [(1.5, true), (0.75, false)] {
        let out = media::path(&format!("speed-xfade-{speed}.wav"));
        let config = config(&out, speed, keep_pitch).with_transitions(transitions);
        let mut session = Session::new(config);
        session.play_queue(&[item(1, &path_a), item(2, &path_b)]);
        session.wait_state(TransportState::Stopped);
        let ends = session
            .log
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    EngineEvent::TrackEnded {
                        reason: TrackEndReason::Finished,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(ends, 2, "{:#?}", session.milestones());
        drop(session);
        let written = media::read_output(&out);
        let (one, two) = (rendered(a.frames(), speed), rendered(b.frames(), speed));
        assert_eq!(written.frames(), one + two - length, "{speed}");
        // A alone up to the window, B alone after it.
        let frame = |i: usize| (written.floats[2 * i], written.floats[2 * i + 1]);
        let start = one - length;
        for i in [start / 2, start - 200] {
            let (l, r) = frame(i);
            assert!(
                (l - 0.5).abs() < 0.02 && r.abs() < 0.02,
                "{speed}: frame {i}: ({l}, {r})"
            );
        }
        for i in [start + length + 200, start + length + (two - length) / 2] {
            let (l, r) = frame(i);
            assert!(
                l.abs() < 0.02 && (r - 0.5).abs() < 0.02,
                "{speed}: frame {i}: ({l}, {r})"
            );
        }
    }
}

#[test]
fn a_change_while_playing_goes_on_where_playback_was() {
    let frames = seconds(1.5);
    let path = media::write_wav("speed-change-in.wav", &tone(440.0, frames));
    let out = media::path("speed-change-out.wav");
    let mut config = config(&out, 1.0, true);
    config.sink_pacing = Pacing::RealTime;
    let output = config.output.clone();
    let mut dsp = config.dsp.clone();
    let mut session = Session::new(config);
    session.load(&item(1, &path), MediaTime::ZERO);
    while session.wait_position(RATE) < seconds(0.3) as u64 {}
    dsp.speed = SpeedSettings::at(2.0, true);
    session.send(EngineCommand::UpdateSettings {
        output,
        loudness: helikodu_model::settings::LoudnessSettings {
            mode: helikodu_model::settings::LoudnessMode::Off,
            ..helikodu_model::settings::LoudnessSettings::default()
        },
        dsp: Box::new(dsp),
    });
    session.wait_state(TransportState::Reconfiguring);
    session.wait_state(TransportState::Stopped);
    let errors = session
        .log
        .iter()
        .filter(|e| matches!(e, EngineEvent::Error { .. }))
        .count();
    assert_eq!(errors, 0, "{:#?}", session.milestones());
    drop(session);
    // Some 0.3 s at the normal speed, the rest twice as fast from where it was
    // (from the start again it would be 0.3 + 0.75 s).
    let written = left(&out).len();
    assert!(
        written > frames / 2 + seconds(0.1) && written < frames / 2 + seconds(0.25),
        "{written} frames written"
    );
}

#[test]
fn bit_perfect_output_plays_at_the_normal_speed() {
    let signal = media::chirp(RATE, seconds(1.0));
    let path = media::write_wav("speed-bitperfect-in.wav", &signal);
    let run = |mode: helikodu_model::audio::OutputMode| {
        let out = media::path(&format!("speed-bitperfect-{mode}.wav"));
        let mut config = config(&out, 1.5, true);
        config.output.format = OutputFormat::default();
        config.output.mode = mode;
        drop(play_one(config, &path));
        media::read_output(&out).ints
    };
    // Exclusive, and nothing else changes the samples: the file as it is.
    let written = run(helikodu_model::audio::OutputMode::Exclusive);
    assert!(written == signal.samples, "{} samples", written.len());
    // Shared: at the speed asked for.
    let written = run(helikodu_model::audio::OutputMode::Shared);
    assert_eq!(written.len() / 2, rendered(signal.frames(), 1.5));
}

/// The level of `x` in windows of `window` frames, one value per window.
fn levels(x: &[f32], window: usize) -> Vec<f64> {
    x.chunks_exact(window)
        .map(|w| (w.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / window as f64).sqrt())
        .collect()
}

#[test]
fn a_gapless_join_at_another_tempo_has_no_dip() {
    // One tone cut into two files, joined gaplessly at 1.25× with the pitch kept.
    let cut = seconds(0.6) + 13;
    let signal = tone(440.0, seconds(1.2));
    let first = media::write_wav("speed-join-0.wav", &signal.slice(0, cut));
    let second = media::write_wav("speed-join-1.wav", &signal.slice(cut, signal.frames()));
    let out = media::path("speed-join-out.wav");
    let mut session = Session::new(config(&out, 1.25, true));
    session.play_queue(&[item(1, &first), item(2, &second)]);
    session.wait_state(TransportState::Stopped);
    drop(session);
    let x = left(&out);
    // Each track keeps its exact length.
    let join = rendered(cut, 1.25);
    assert_eq!(x.len(), join + rendered(signal.frames() - cut, 1.25));
    // The level around the join is the level elsewhere: no fade out and in.
    let window = seconds(0.005);
    let steady = levels(&x[seconds(0.1)..join - seconds(0.1)], window);
    let typical = steady.iter().sum::<f64>() / steady.len() as f64;
    let around = levels(&x[join - seconds(0.04)..join + seconds(0.04)], window);
    let lowest = around.iter().copied().fold(f64::INFINITY, f64::min);
    let lowest_steady = steady.iter().copied().fold(f64::INFINITY, f64::min);
    assert!(
        lowest > 0.9 * typical,
        "lowest {lowest:.4} around the join, {typical:.4} typical, {lowest_steady:.4} lowest elsewhere"
    );
}
