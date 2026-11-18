// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Output and loudness settings changed while playing: a new stream format reopens
//! the stream at the same position, a loudness change ramps the gain on the open
//! stream, and neither clicks.

use helikodu_model::audio::MediaTime;
use helikodu_model::player::{EngineCommand, EngineEvent, Loudness, TransportState};
use helikodu_model::settings::{
    DeviceLostPolicy, LoudnessMode, LoudnessSettings, OutputFormat, OutputSettings,
};

use crate::media;
use crate::session::{Session, item, null_config, wav_config};

const RATE: u32 = 44_100;
const FRAMES: usize = 3 * 44_100;

/// Loudness normalization off, as the engine starts by default.
fn loudness_off() -> LoudnessSettings {
    LoudnessSettings {
        mode: LoudnessMode::Off,
        ..LoudnessSettings::default()
    }
}

fn update(output: &OutputSettings, loudness: LoudnessSettings) -> EngineCommand {
    EngineCommand::UpdateSettings {
        output: output.clone(),
        loudness,
        dsp: Default::default(),
    }
}

fn reconfigurations(session: &Session) -> usize {
    session
        .log
        .iter()
        .filter(|e| **e == EngineEvent::StateChanged(TransportState::Reconfiguring))
        .count()
}

#[test]
fn a_new_sample_format_reopens_the_stream_at_the_same_position() {
    let signal = media::chirp(RATE, FRAMES);
    let path = media::write_wav("format-in.wav", &signal);
    let out = media::path("format-out.wav");
    let config = wav_config(&out);
    let mut output = config.output.clone();
    output.format = OutputFormat::F32;
    let mut session = Session::new(config);
    session.load(&item(1, &path), MediaTime::ZERO);
    session.send(update(&output, loudness_off()));
    session.wait_state(TransportState::Reconfiguring);
    session.wait_state(TransportState::Playing);
    let at = session.wait_position(RATE) as usize;
    session.wait_state(TransportState::Stopped);
    assert_eq!(session.engine.stats().snapshot().streams_opened, 2);
    drop(session);

    // The reopened stream rewrote the file, in the new format, from the position
    // where the first one fell silent.
    let written = media::read_output(&out);
    assert!(written.ints.is_empty());
    assert_eq!(
        written.frames(),
        FRAMES - at,
        "the new stream starts elsewhere"
    );
    let fade = media::fade_frames(10, RATE);
    let expected = |i: usize| f32::from(signal.samples[i]) / 32_768.0;
    for (i, got) in written.floats.iter().enumerate() {
        let original = expected(2 * at + i);
        if i < 2 * fade {
            assert!(got.abs() <= original.abs() + 1e-6, "fade in sample {i}");
        } else {
            assert_eq!(*got, original, "sample {i} after the fade in");
        }
    }
}

#[test]
fn a_loudness_change_ramps_the_gain_on_the_open_stream() {
    let signal = media::sine(RATE, FRAMES);
    let path = media::write_wav("loudness-in.wav", &signal);
    let out = media::path("loudness-out.wav");
    let config = wav_config(&out);
    let output = config.output.clone();
    let half = 20.0 * 0.5f64.log10();
    let track = item(1, &path).with_loudness(
        Loudness {
            track_gain_db: Some(half),
            ..Loudness::UNKNOWN
        },
        false,
    );
    let mut session = Session::new(config);
    session.load(&track, MediaTime::ZERO);
    let loudness = LoudnessSettings {
        mode: LoudnessMode::Track,
        preamp_db: 0.0,
        ..LoudnessSettings::default()
    };
    session.send(update(&output, loudness));
    session.wait_state(TransportState::Stopped);
    assert_eq!(session.engine.stats().snapshot().streams_opened, 1);
    assert_eq!(reconfigurations(&session), 0);
    drop(session);

    let written = media::read_output(&out).ints;
    assert_eq!(
        written.len(),
        signal.samples.len(),
        "audio was lost or added"
    );
    // Untouched up to the change, then a 20 ms ramp to half the level.
    let change = (0..FRAMES)
        .find(|&i| written[2 * i..2 * i + 2] != signal.samples[2 * i..2 * i + 2])
        .expect("the gain never changed");
    let ramp = media::fade_frames(20, RATE);
    assert!(
        change > 0 && change + ramp < FRAMES,
        "changed at frame {change}"
    );
    for i in change..FRAMES {
        for c in 0..2 {
            let got = f64::from(written[2 * i + c]);
            let original = f64::from(signal.samples[2 * i + c]);
            if i < change + ramp {
                assert!(got.abs() <= original.abs() + 2.0, "ramp frame {i}");
            } else {
                assert!(
                    (got - original * 0.5).abs() <= 2.5,
                    "frame {i}: {got} instead of {}",
                    original * 0.5
                );
            }
        }
    }
    let limit = 1.5 * media::sine_max_step(RATE);
    let step = media::max_step(&written);
    assert!(step <= limit, "a step of {step} (limit {limit})");
}

#[test]
fn only_settings_of_the_stream_reopen_it() {
    let path = media::write_wav("settings-null.wav", &media::sine(RATE, FRAMES));
    let config = null_config();
    let mut output = config.output.clone();
    let mut session = Session::new(config);
    session.load(&item(1, &path), MediaTime::ZERO);
    let first = session.wait_position(RATE);

    // Policies apply without touching the stream.
    output.on_device_lost = DeviceLostPolicy::Stop;
    output.exclusive_release_s = 1;
    output.preroll_ms = 100;
    session.send(update(&output, loudness_off()));
    // A new buffer length takes another stream, at the position heard.
    output.buffer_ms *= 2;
    session.send(update(&output, loudness_off()));
    session.wait_state(TransportState::Reconfiguring);
    session.wait_state(TransportState::Playing);
    let resumed = session.wait_position(RATE);
    assert!(resumed >= first, "{resumed} < {first}");
    session.wait_state(TransportState::Stopped);
    assert_eq!(reconfigurations(&session), 1);
    assert_eq!(session.engine.stats().snapshot().streams_opened, 2);
}

#[test]
fn a_new_format_while_paused_opens_the_new_stream_on_play() {
    let signal = media::chirp(RATE, FRAMES);
    let path = media::write_wav("paused-format-in.wav", &signal);
    let out = media::path("paused-format-out.wav");
    let config = wav_config(&out);
    let mut output = config.output.clone();
    output.format = OutputFormat::F32;
    let mut session = Session::new(config);
    session.load(&item(1, &path), MediaTime::ZERO);
    session.send(EngineCommand::Pause);
    session.wait_state(TransportState::Paused);
    let paused_at = session.wait_position(RATE) as usize;
    session.send(update(&output, loudness_off()));
    session.send(EngineCommand::Play);
    session.wait_state(TransportState::Playing);
    assert_eq!(session.wait_position(RATE) as usize, paused_at);
    session.wait_state(TransportState::Stopped);
    assert_eq!(reconfigurations(&session), 0);
    drop(session);

    let written = media::read_output(&out);
    assert_eq!(written.frames(), FRAMES - paused_at);
    let fade = media::fade_frames(10, RATE);
    let tail = &written.floats[2 * fade..];
    let original = &signal.samples[2 * (paused_at + fade)..];
    assert!(
        tail.iter()
            .zip(original)
            .all(|(got, s)| *got == f32::from(*s) / 32_768.0),
        "the audio after the fade in differs"
    );
}
