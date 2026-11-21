// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSP chain in a playing engine: off it leaves the samples bit-identical, a
//! change while playing crossfades without a click, and the limiter's lookahead
//! neither loses nor adds audio, and its activity is reported, paced, and only
//! when it acts.

use helikodu_engine::output::Pacing;
use helikodu_model::audio::MediaTime;
use helikodu_model::player::{EngineCommand, EngineEvent, LimiterActivity, TransportState};
use helikodu_model::settings::{
    ChannelMode, DspSettings, EqMode, LoudnessMode, LoudnessSettings, OutputSettings,
};

use crate::media;
use crate::session::{Session, item, null_config, wav_config};

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

/// Loud bursts every 300 ms in a quiet sine, so the limiter acts a few times.
fn bursts(rate: u32, frames: usize) -> media::Signal {
    let mut signal = media::sine(rate, frames);
    let period = rate as usize * 3 / 10;
    let burst = rate as usize / 50;
    for i in 0..frames {
        if i % period < burst {
            let t = i as f64 / f64::from(rate);
            let v = (0.99 * (std::f64::consts::TAU * 1000.0 * t).sin() * 32_767.0) as i16;
            signal.samples[2 * i] = v;
            signal.samples[2 * i + 1] = v;
        }
    }
    signal
}

fn limiter_events(log: &[EngineEvent]) -> Vec<Option<LimiterActivity>> {
    log.iter()
        .filter_map(|e| match e {
            EngineEvent::Limiter(a) => Some(*a),
            _ => None,
        })
        .collect()
}

#[test]
fn the_limiter_reports_its_activity_a_few_times_a_second() {
    let frames = RATE as usize * 3 / 2;
    let path = media::write_wav("dsp-limiter-bursts.wav", &bursts(RATE, frames));
    let mut config = null_config();
    config.sink_pacing = Pacing::RealTime;
    config.dsp.limiter.enabled = true;
    let mut session = Session::new(config);
    let started = std::time::Instant::now();
    session.load(&item(1, &path), MediaTime::ZERO);
    session.wait_state(TransportState::Stopped);
    let seconds = started.elapsed().as_secs_f64();
    let events = limiter_events(&session.log);
    let acted = events
        .iter()
        .flatten()
        .find(|a| a.times > 0)
        .expect("the limiter's activity was reported");
    assert!(acted.largest_db > 0.0, "{acted:?}");
    // Paced: at most one report every 200 ms, plus the first.
    let most = (seconds / 0.2).ceil() as usize + 2;
    assert!(
        events.len() <= most,
        "{} reports in {seconds:.2} s",
        events.len()
    );
}

#[test]
fn an_idle_limiter_reports_nothing() {
    let signal = media::sine(RATE, RATE as usize);
    let path = media::write_wav("dsp-limiter-idle.wav", &signal);
    let mut config = null_config();
    config.sink_pacing = Pacing::RealTime;
    config.dsp.limiter.enabled = true;
    let mut session = Session::new(config);
    session.load(&item(1, &path), MediaTime::ZERO);
    session.wait_state(TransportState::Stopped);
    // Only that it runs, never anything it did.
    let events = limiter_events(&session.log);
    assert!(events.len() <= 2, "{events:?}");
    assert!(
        events
            .iter()
            .all(|a| a.is_none_or(|a| a == LimiterActivity::default())),
        "{events:?}"
    );
}

#[test]
fn a_track_with_its_own_equalizer_plays_with_it() {
    use helikodu_model::settings::EqualizerSettings;

    let signal = media::sine(RATE, FRAMES);
    let first = media::write_wav("dsp-track-eq-1.wav", &signal);
    let second = media::write_wav("dsp-track-eq-2.wav", &signal);
    let out = media::path("dsp-track-eq-out.wav");
    let mut session = Session::new(wav_config(&out));
    // A preamp of -6 dB and no filters: the track plays at half.
    let half = EqualizerSettings {
        enabled: true,
        auto_preamp: false,
        preamp_db: 20.0 * 0.5f64.log10(),
        ..EqualizerSettings::default()
    };
    session.play_queue(&[item(1, &first), item(2, &second).with_equalizer(Some(half))]);
    session.wait_state(TransportState::Stopped);
    drop(session);

    let written = media::read_output(&out).ints;
    assert_eq!(written.len(), 2 * signal.samples.len());
    // The first track is untouched, the second at half after the crossfade.
    assert_eq!(written[..signal.samples.len()], signal.samples[..]);
    let fade = media::fade_frames(20, RATE);
    let second_part = &written[signal.samples.len()..];
    for i in fade..FRAMES {
        let want = f64::from(signal.samples[2 * i]) * 0.5;
        let got = f64::from(second_part[2 * i]);
        assert!((got - want).abs() <= 2.0, "frame {i}: {got} vs {want}");
    }
}

#[test]
fn the_compressor_delays_but_loses_nothing() {
    let signal = media::sine(RATE, FRAMES);
    let path = media::write_wav("dsp-compressor-in.wav", &signal);
    let out = media::path("dsp-compressor-out.wav");
    let mut config = wav_config(&out);
    config.dsp.compressor.enabled = true;
    let mut session = Session::new(config);
    session.load(&item(1, &path), MediaTime::ZERO);
    session.wait_state(TransportState::Stopped);
    drop(session);

    let written = media::read_output(&out).ints;
    // The lookahead (2 ms) is played out at the end.
    let delay = (written.len() - signal.samples.len()) / 2;
    assert_eq!(delay, media::fade_frames(2, RATE));
    // Night mode: once the attack has acted, the sine at -6 dBFS comes out quieter
    // than it went in, but louder than the threshold.
    let settled = media::fade_frames(100, RATE) * 2;
    let peak = written[settled..]
        .iter()
        .map(|s| i32::from(*s).abs())
        .max()
        .unwrap();
    assert!(peak < 16_000 && peak > 1_000, "{peak}");
}
