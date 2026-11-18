// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSP chain in a playing engine: off it leaves the samples bit-identical, a
//! change while playing crossfades without a click, and the limiter's lookahead
//! neither loses nor adds audio.

use helikodu_model::audio::MediaTime;
use helikodu_model::player::{EngineCommand, TransportState};
use helikodu_model::settings::{
    ChannelMode, DspSettings, EqMode, LoudnessMode, LoudnessSettings, OutputSettings,
};

use crate::media;
use crate::session::{Session, item, wav_config};

const RATE: u32 = 44_100;
const FRAMES: usize = 3 * 44_100;

fn update(output: &OutputSettings, dsp: &DspSettings) -> EngineCommand {
    EngineCommand::UpdateSettings {
        output: output.clone(),
        loudness: LoudnessSettings {
            mode: LoudnessMode::Off,
            ..LoudnessSettings::default()
        },
        dsp: Box::new(dsp.clone()),
    }
}

#[test]
fn stages_that_change_nothing_keep_the_samples_exact() {
    let signal = media::chirp(RATE, FRAMES);
    let path = media::write_wav("dsp-neutral-in.wav", &signal);
    let out = media::path("dsp-neutral-out.wav");
    let mut config = wav_config(&out);
    // Switched on, but flat and centred.
    config.dsp.equalizer.enabled = true;
    config.dsp.equalizer.mode = EqMode::Graphic31;
    config.dsp.channel_mix.enabled = true;
    let mut session = Session::new(config);
    session.load(&item(1, &path), MediaTime::ZERO);
    session.wait_state(TransportState::Stopped);
    drop(session);
    assert_eq!(media::read_output(&out).ints, signal.samples);
}

#[test]
fn a_change_while_playing_crossfades_without_a_click() {
    let signal = media::sine(RATE, FRAMES);
    let path = media::write_wav("dsp-change-in.wav", &signal);
    let out = media::path("dsp-change-out.wav");
    let config = wav_config(&out);
    let output = config.output.clone();
    let mut session = Session::new(config);
    session.load(&item(1, &path), MediaTime::ZERO);
    let mut dsp = DspSettings::default();
    dsp.channel_mix.enabled = true;
    dsp.channel_mix.channels = ChannelMode::Swap;
    dsp.channel_mix.balance_db = 20.0 * 0.5f64.log10();
    session.send(update(&output, &dsp));
    session.wait_state(TransportState::Stopped);
    assert_eq!(session.engine.stats().snapshot().streams_opened, 1);
    drop(session);

    let written = media::read_output(&out).ints;
    assert_eq!(
        written.len(),
        signal.samples.len(),
        "audio was lost or added"
    );
    // Bit-identical up to the change.
    let change = (0..FRAMES)
        .find(|&i| written[2 * i..2 * i + 2] != signal.samples[2 * i..2 * i + 2])
        .expect("the settings never applied");
    assert!(change > 0, "changed at the start");
    // No step larger than the sine's own (plus dither and rounding) anywhere.
    let limit = media::sine_max_step(RATE) * 1.15 + 3.0;
    let step = media::max_step(&written);
    assert!(step <= limit, "a step of {step} (limit {limit})");
    // After the 20 ms crossfade: swapped, and 6 dB to the left (the right output,
    // which now carries the left input, at half).
    let fade = media::fade_frames(20, RATE);
    for i in change + fade..FRAMES {
        let (l, r) = (
            f64::from(signal.samples[2 * i]),
            f64::from(signal.samples[2 * i + 1]),
        );
        let (got_l, got_r) = (f64::from(written[2 * i]), f64::from(written[2 * i + 1]));
        assert!((got_l - r).abs() <= 2.0, "frame {i} left: {got_l}");
        assert!((got_r - l * 0.5).abs() <= 2.0, "frame {i} right: {got_r}");
    }
}

#[test]
fn the_limiter_delays_but_loses_nothing() {
    let signal = media::sine(RATE, FRAMES);
    let path = media::write_wav("dsp-limiter-in.wav", &signal);
    let out = media::path("dsp-limiter-out.wav");
    let mut config = wav_config(&out);
    config.dsp.limiter.enabled = true;
    let mut session = Session::new(config);
    session.load(&item(1, &path), MediaTime::ZERO);
    session.wait_state(TransportState::Stopped);
    drop(session);

    let written = media::read_output(&out).ints;
    // The lookahead's delay is played out at the end, so the file is longer by
    // exactly the delay, and the signal comes through (the sine is at -6 dBFS, below
    // the ceiling) with only dither added.
    let delay = (written.len() - signal.samples.len()) / 2;
    assert!(delay > 0 && delay < media::fade_frames(5, RATE), "{delay}");
    assert!(written[..2 * delay].iter().all(|s| s.abs() <= 1));
    for (i, (got, want)) in written[2 * delay..].iter().zip(&signal.samples).enumerate() {
        assert!(
            (i32::from(*got) - i32::from(*want)).abs() <= 1,
            "sample {i}"
        );
    }
}
