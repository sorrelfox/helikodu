// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pause, seek, stop, device switches and start positions: where the audio goes on,
//! and that it never clicks.

use helikodu_model::audio::{MediaTime, SampleRate};
use helikodu_model::player::{
    EngineCommand, EngineError, EngineEvent, ItemId, TrackEndReason, TransportState, Volume,
};

use crate::media::{self, Signal};
use crate::session::{Session, item, wav_config, wav_output};

const RATE: u32 = 44_100;
const FRAMES: usize = 3 * 44_100;

fn rate() -> SampleRate {
    SampleRate::new(RATE).unwrap()
}

/// The left channel of frame `i`.
fn left(samples: &[i16], i: usize) -> f64 {
    f64::from(samples[2 * i])
}

/// No step between frames larger than the signal's own, allowing for the fades.
fn assert_click_free(out: &[i16]) {
    let limit = 1.5 * media::sine_max_step(RATE);
    let step = media::max_step(out);
    assert!(step <= limit, "a step of {step} (limit {limit})");
}

/// Checks that the output from frame `at` is the signal from frame `from` on,
/// faded in over `fade` frames, to the end, and that the output is silent from
/// `silent_from` to there.
fn assert_continues(
    out: &[i16],
    at: usize,
    signal: &Signal,
    from: usize,
    fade: usize,
    silent_from: usize,
) {
    let frames = out.len() / 2;
    assert_eq!(
        frames - at,
        signal.frames() - from,
        "the tail has the wrong length"
    );
    for i in silent_from..at {
        assert_eq!(out[2 * i], 0, "frame {i} before the fade in is not silent");
    }
    for k in 0..fade {
        let expected = left(&signal.samples, from + k) * media::fade_in_level(k, fade);
        let got = left(out, at + k);
        assert!(
            (got - expected).abs() <= 3.0,
            "fade in frame {k}: {got} instead of {expected}"
        );
    }
    assert!(
        out[2 * (at + fade)..] == signal.samples[2 * (from + fade)..],
        "the audio after the fade in differs"
    );
}

/// Checks that the output is the signal untouched up to a fade out that ends in
/// silence at frame `end`.
fn assert_fades_out_at(out: &[i16], signal: &Signal, end: usize, fade: usize) {
    let start = end - fade;
    assert!(
        out[..2 * start] == signal.samples[..2 * start],
        "the audio before the fade out differs"
    );
    for i in start..end {
        assert!(left(out, i).abs() <= left(&signal.samples, i).abs() + 2.0);
    }
    // The last frames of the fade are below one step, give or take the dither.
    assert!(
        left(out, end - 1).abs() <= 2.0,
        "the fade out does not end at frame {end}"
    );
}

fn sine_file(name: &str) -> (Signal, std::path::PathBuf) {
    let signal = media::sine(RATE, FRAMES);
    let path = media::write_wav(name, &signal);
    (signal, path)
}

#[test]
fn pause_and_resume_continue_exactly_where_the_sound_stopped() {
    let (signal, path) = sine_file("pause-in.wav");
    let out = media::path("pause-out.wav");
    let mut session = Session::new(wav_config(&out));
    session.load(&item(1, &path), MediaTime::ZERO);
    session.send(EngineCommand::Pause);
    session.wait_state(TransportState::Paused);
    let paused_at = session.wait_position(RATE) as usize;
    session.send(EngineCommand::Play);
    session.wait_state(TransportState::Playing);
    let resumed_at = session.wait_position(RATE) as usize;
    assert_eq!(resumed_at, paused_at);
    session.wait_state(TransportState::Stopped);
    drop(session);

    let written = media::read_output(&out).ints;
    let fade = media::fade_frames(10, RATE);
    assert_fades_out_at(&written, &signal, paused_at, fade);
    let at = written.len() / 2 - (FRAMES - paused_at);
    assert!(at >= paused_at);
    assert_continues(&written, at, &signal, paused_at, fade, paused_at);
    assert_click_free(&written);
}

#[test]
fn seeking_lands_on_the_exact_frame_without_clicks() {
    let (signal, path) = sine_file("seek-in.wav");
    let out = media::path("seek-out.wav");
    let mut session = Session::new(wav_config(&out));
    session.load(&item(1, &path), MediaTime::ZERO);
    let target = 77_777;
    session.send(EngineCommand::Seek(MediaTime::from_frames(target, rate())));
    session.wait_state(TransportState::Seeking);
    session.wait_state(TransportState::Playing);
    assert_eq!(session.wait_position(RATE), target);
    session.wait_state(TransportState::Stopped);
    drop(session);

    let written = media::read_output(&out).ints;
    let at = written.len() / 2 - (FRAMES - target as usize);
    // Where the fade out ended is not reported; the shape of the fade in shows a
    // landing that is off by even one frame.
    assert_continues(
        &written,
        at,
        &signal,
        target as usize,
        media::fade_frames(5, RATE),
        at,
    );
    assert_click_free(&written);
}

#[test]
fn seeking_while_paused_moves_the_position_only() {
    let (signal, path) = sine_file("paused-seek-in.wav");
    let out = media::path("paused-seek-out.wav");
    let mut session = Session::new(wav_config(&out));
    session.load(&item(1, &path), MediaTime::ZERO);
    session.send(EngineCommand::Pause);
    session.wait_state(TransportState::Paused);
    let paused_at = session.wait_position(RATE) as usize;
    let target = 100_001;
    session.send(EngineCommand::Seek(MediaTime::from_frames(target, rate())));
    session.wait_state(TransportState::Seeking);
    session.wait_state(TransportState::Paused);
    assert_eq!(session.wait_position(RATE), target);
    session.send(EngineCommand::Play);
    session.wait_state(TransportState::Stopped);
    drop(session);

    let written = media::read_output(&out).ints;
    let at = written.len() / 2 - (FRAMES - target as usize);
    let fade = media::fade_frames(10, RATE);
    assert_continues(&written, at, &signal, target as usize, fade, paused_at);
    assert_click_free(&written);
}

#[test]
fn stop_fades_out_and_unloads() {
    let (_, path) = sine_file("stop-in.wav");
    let out = media::path("stop-out.wav");
    let mut session = Session::new(wav_config(&out));
    session.load(&item(1, &path), MediaTime::ZERO);
    session.send(EngineCommand::Stop);
    session.wait_state(TransportState::Stopped);
    let ended = session.log.iter().any(|e| {
        *e == EngineEvent::TrackEnded {
            item: ItemId::new(1),
            reason: TrackEndReason::Interrupted,
        }
    });
    assert!(ended, "{:#?}", session.log);
    session.send(EngineCommand::Play);
    let error = session.wait_for(|e| matches!(e, EngineEvent::Error { .. }));
    assert_eq!(
        error,
        EngineEvent::Error {
            item: None,
            error: EngineError::NothingLoaded
        }
    );
    drop(session);

    let written = media::read_output(&out).ints;
    assert!(written.len() / 2 < FRAMES);
    assert_click_free(&written);
    // The file ends in silence (give or take the dither), not in the middle of a wave.
    assert!(written[written.len() - 2..].iter().all(|&s| s.abs() <= 2));
}

#[test]
fn switching_the_output_continues_at_the_same_position() {
    let signal = media::chirp(RATE, FRAMES);
    let path = media::write_wav("switch-in.wav", &signal);
    let first = media::path("switch-a.wav");
    let second = media::path("switch-b.wav");
    let mut session = Session::new(wav_config(&first));
    session.load(&item(1, &path), MediaTime::ZERO);
    session.send(EngineCommand::SetOutput(wav_output(&second)));
    session.wait_state(TransportState::Reconfiguring);
    session.wait_state(TransportState::Playing);
    let at = session.wait_position(RATE) as usize;
    session.wait_state(TransportState::Stopped);
    drop(session);

    let fade = media::fade_frames(10, RATE);
    let a = media::read_output(&first).ints;
    assert!(a.len() / 2 >= at);
    assert_eq!(a[..2 * (at - fade)], signal.samples[..2 * (at - fade)]);
    assert!(
        a[2 * at..].iter().all(|&s| s == 0),
        "sound after the switch point"
    );
    let b = media::read_output(&second).ints;
    assert_eq!(
        b.len() / 2,
        FRAMES - at,
        "the second device starts elsewhere"
    );
    assert_eq!(b[2 * fade..], signal.samples[2 * (at + fade)..]);
}

#[test]
fn a_start_position_is_exact_and_faded_in() {
    let (signal, path) = sine_file("start-in.wav");
    let out = media::path("start-out.wav");
    let mut session = Session::new(wav_config(&out));
    let start = 44_100;
    session.load(&item(1, &path), MediaTime::from_millis(1000));
    assert_eq!(session.wait_position(RATE), start);
    session.wait_state(TransportState::Stopped);
    drop(session);

    let written = media::read_output(&out).ints;
    assert_eq!(written.len() / 2, FRAMES - start as usize);
    let fade = media::fade_frames(10, RATE);
    assert_eq!(
        written[2 * fade..],
        signal.samples[2 * (start as usize + fade)..]
    );
    assert_click_free(&written);
}

#[test]
fn events_come_in_order_and_positions_rise() {
    let a = media::write_wav("order-a.wav", &media::sine(RATE, 30_000));
    let b = media::write_wav("order-b.wav", &media::sine(RATE, 20_000));
    let out = media::path("order-out.wav");
    let mut session = Session::new(wav_config(&out));
    session.play_queue(&[item(1, &a), item(2, &b)]);
    session.wait_state(TransportState::Stopped);

    let one = ItemId::new(1);
    let two = ItemId::new(2);
    let state = EngineEvent::StateChanged;
    let started = |item: ItemId, frames: u64| EngineEvent::TrackStarted {
        item,
        spec: helikodu_model::audio::AudioSpec::new(
            helikodu_model::audio::SampleFormat::S16(helikodu_model::audio::Endianness::NATIVE),
            rate(),
            helikodu_model::audio::ChannelLayout::STEREO,
        ),
        length: Some(MediaTime::from_frames(frames, rate())),
    };
    let ended = |item| EngineEvent::TrackEnded {
        item,
        reason: TrackEndReason::Finished,
    };
    assert_eq!(
        session.milestones(),
        [
            state(TransportState::Loading),
            started(one, 30_000),
            state(TransportState::Playing),
            ended(one),
            started(two, 20_000),
            state(TransportState::Draining),
            ended(two),
            state(TransportState::Stopped),
        ]
    );

    // NearEnd comes before each end; positions rise within an item and stay inside it.
    let near_end = |item| {
        session
            .log
            .iter()
            .position(|e| *e == EngineEvent::NearEnd { item })
            .unwrap()
    };
    let end = |item| session.log.iter().position(|e| *e == ended(item)).unwrap();
    assert!(near_end(one) < end(one));
    assert!(near_end(two) < end(two));
    let mut last: Option<(ItemId, MediaTime)> = None;
    for event in &session.log {
        if let EngineEvent::Position { item, position } = event {
            let length = if *item == one { 30_000 } else { 20_000 };
            assert!(position.frames_at(rate()) <= length);
            if let Some((last_item, last_position)) = last
                && last_item == *item
            {
                assert!(*position >= last_position, "{:#?}", session.log);
            }
            last = Some((*item, *position));
        }
    }
}

#[test]
fn software_volume_scales_the_samples() {
    let (signal, path) = sine_file("volume-in.wav");
    let out = media::path("volume-out.wav");
    let mut config = wav_config(&out);
    config.volume = Volume::new(0.5).unwrap();
    let mut session = Session::new(config);
    session.load(&item(1, &path), MediaTime::ZERO);
    session.wait_state(TransportState::Stopped);
    drop(session);

    let written = media::read_output(&out).ints;
    assert_eq!(written.len(), signal.samples.len());
    for (got, original) in written.iter().zip(&signal.samples) {
        let expected = f64::from(*original) * 0.125;
        assert!(
            (f64::from(*got) - expected).abs() <= 2.0,
            "{got} for {expected}"
        );
    }
}
