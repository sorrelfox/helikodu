// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The track analysis on generated signals, decoded through the registry from WAV
//! files: leading and trailing silence (with a noise floor), silence inside, a
//! fade-out, a track without silence, a silent track, CUE-style sections, and the
//! loudness measurement taken in the same pass.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::f64::consts::PI;
use std::fs;
use std::path::{Path, PathBuf};

use helikodu_engine::analysis::{
    AnalysisError, MIN_INNER_SILENCE_MS, SourceAnalysis, analyze_source, measure_source,
};
use helikodu_engine::decode::DecoderRegistry;
use helikodu_model::audio::{MediaTime, SampleRate};
use helikodu_model::library::{MediaSource, SourceSection};

const RATE: u32 = 48_000;
/// The silence windows are 10 ms long: positions are found to within one.
const WINDOW: u64 = RATE as u64 / 100;

#[derive(Clone, Copy)]
enum Part {
    /// A 1 kHz sine at a peak level in dBFS, for seconds.
    Tone(f64, f64),
    /// Digital silence, for seconds.
    Silence(f64),
    /// White noise at an RMS level in dBFS, for seconds.
    Noise(f64, f64),
    /// A sine whose level falls linearly in dB from the first to the second
    /// level, for seconds.
    Fade(f64, f64, f64),
}

fn frames(secs: f64) -> usize {
    (secs * f64::from(RATE)).round() as usize
}

/// The interleaved stereo samples of `parts`, one after the other.
fn stereo(parts: &[Part]) -> Vec<f32> {
    let mut out = Vec::new();
    let mut phase = 0.0_f64;
    let step = 2.0 * PI * 1000.0 / f64::from(RATE);
    let mut seed: u32 = 0x1234_5678;
    for part in parts {
        match *part {
            Part::Tone(dbfs, secs) => {
                let amplitude = 10f64.powf(dbfs / 20.0);
                for _ in 0..frames(secs) {
                    let v = (amplitude * phase.sin()) as f32;
                    out.extend([v, v]);
                    phase = (phase + step) % (2.0 * PI);
                }
            }
            Part::Fade(from, to, secs) => {
                let n = frames(secs);
                for i in 0..n {
                    let db = from + (to - from) * i as f64 / n as f64;
                    let v = (10f64.powf(db / 20.0) * phase.sin()) as f32;
                    out.extend([v, v]);
                    phase = (phase + step) % (2.0 * PI);
                }
            }
            Part::Silence(secs) => out.extend(std::iter::repeat_n(0.0, frames(secs) * 2)),
            Part::Noise(dbfs, secs) => {
                // Uniform noise has an RMS of a third of its peak, squared.
                let peak = 10f64.powf(dbfs / 20.0) * 3f64.sqrt();
                for _ in 0..frames(secs) * 2 {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let unit = f64::from(seed >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;
                    out.push((peak * unit) as f32);
                }
            }
        }
    }
    out
}

fn dir() -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine-track-analysis");
    fs::create_dir_all(&d).unwrap();
    d
}

/// Writes interleaved stereo samples as a 16-bit WAV file.
fn write_wav(name: &str, samples: &[f32]) -> PathBuf {
    let data_len = samples.len() * 2;
    let mut b = Vec::with_capacity(44 + data_len);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data_len as u32).to_le_bytes());
    for v in samples {
        let s = (f64::from(*v) * 32_767.0).round() as i16;
        b.extend_from_slice(&s.to_le_bytes());
    }
    let path = dir().join(name);
    fs::write(&path, b).unwrap();
    path
}

fn analyze(source: &MediaSource, loudness: bool) -> SourceAnalysis {
    let registry = DecoderRegistry::with_default_backends();
    analyze_source(&registry, source, loudness, &mut || false).unwrap()
}

fn analyze_parts(name: &str, parts: &[Part]) -> SourceAnalysis {
    let path = write_wav(name, &stereo(parts));
    analyze(&MediaSource::file(&path), false)
}

fn at(secs: f64) -> u64 {
    frames(secs) as u64
}

#[track_caller]
fn assert_frames(what: &str, measured: u64, expected: u64, tolerance: u64) {
    assert!(
        measured.abs_diff(expected) <= tolerance,
        "{what}: {measured} frames ({:.3} s), expected {expected} ({:.3} s) ± {tolerance}",
        measured as f64 / f64::from(RATE),
        expected as f64 / f64::from(RATE),
    );
}

#[track_caller]
fn assert_near(what: &str, measured: f64, expected: f64, tolerance: f64) {
    assert!(
        (measured - expected).abs() <= tolerance,
        "{what}: {measured:.3}, expected {expected:.3} ± {tolerance}"
    );
}

#[test]
fn leading_and_trailing_silence_with_a_noise_floor() {
    let result = analyze_parts(
        "edges.wav",
        &[
            Part::Silence(1.0),
            Part::Noise(-75.0, 1.0),
            Part::Tone(-20.0, 5.0),
            Part::Noise(-70.0, 1.5),
            Part::Silence(1.5),
        ],
    );
    let a = result.analysis;
    assert_eq!(a.rate, RATE);
    assert_eq!(a.frames, at(10.0));
    let audio = a.audio.clone().unwrap();
    assert_frames("sound starts", audio.start, at(2.0), WINDOW);
    assert_frames("sound ends", audio.end, at(7.0), WINDOW);
    assert_frames("leading silence", a.leading_silence(), at(2.0), WINDOW);
    assert_frames("trailing silence", a.trailing_silence(), at(3.0), WINDOW);
    assert!(a.silences.is_empty(), "{:?}", a.silences);
    // A sine at −20 dBFS peak reads about −20.7 LUFS at 1 kHz in stereo.
    let head = a.head_lufs.unwrap();
    let tail = a.tail_lufs.unwrap();
    assert_near("head", head, -20.0, 1.0);
    assert_near("tail", tail, head, 0.2);
    assert_frames("fade-in end", a.fade_in_end.unwrap(), at(2.5), WINDOW * 20);
    assert_frames(
        "fade-out start",
        a.fade_out_start.unwrap(),
        at(6.5),
        WINDOW * 20,
    );
    // One value per second: silence, noise below the gate of the meter, the tone.
    assert_eq!(a.envelope.step_secs(), 1);
    assert_eq!(a.envelope.len(), 10);
    assert_eq!(a.envelope.lufs(0), None);
    for second in 2..7 {
        assert_near("envelope", a.envelope.lufs(second).unwrap(), head, 0.5);
    }
    assert_eq!(a.envelope.lufs(9), None);
    assert!(result.loudness.is_none());
}

#[test]
fn silence_inside_counts_from_two_seconds() {
    assert_eq!(MIN_INNER_SILENCE_MS, 2_000);
    let a = analyze_parts(
        "inside.wav",
        &[
            Part::Tone(-20.0, 3.0),
            Part::Silence(4.0),
            Part::Tone(-20.0, 3.0),
            // Too short to be recorded.
            Part::Silence(1.0),
            Part::Tone(-20.0, 2.0),
            Part::Noise(-80.0, 2.5),
            Part::Tone(-20.0, 1.0),
        ],
    )
    .analysis;
    let audio = a.audio.clone().unwrap();
    assert_eq!(audio.start, 0);
    assert_frames("sound ends", audio.end, at(16.5), WINDOW);
    assert_eq!(a.silences.len(), 2, "{:?}", a.silences);
    assert_frames("first silence starts", a.silences[0].start, at(3.0), WINDOW);
    assert_frames("first silence ends", a.silences[0].end, at(7.0), WINDOW);
    assert_frames(
        "second silence starts",
        a.silences[1].start,
        at(13.0),
        WINDOW,
    );
    assert_frames("second silence ends", a.silences[1].end, at(15.5), WINDOW);
}

#[test]
fn a_fade_out_is_found_where_it_starts() {
    let a = analyze_parts(
        "fade.wav",
        &[
            Part::Tone(-10.0, 20.0),
            Part::Fade(-10.0, -80.0, 8.0),
            Part::Silence(2.0),
        ],
    )
    .analysis;
    let audio = a.audio.clone().unwrap();
    assert_eq!(audio.start, 0);
    // The sine's RMS is 3 dB below its peak: it falls below −60 dBFS RMS at a
    // peak of −57 dBFS, 47/70 of the way through the fade.
    assert_frames(
        "sound ends",
        audio.end,
        at(20.0 + 8.0 * 47.0 / 70.0),
        WINDOW * 2,
    );
    assert_frames(
        "fade-out start",
        a.fade_out_start.unwrap(),
        at(20.0),
        at(1.0),
    );
    assert_frames("fade-in end", a.fade_in_end.unwrap(), at(0.5), WINDOW * 2);
    let (head, tail) = (a.head_lufs.unwrap(), a.tail_lufs.unwrap());
    // The last five seconds of sound are the fade, mostly its louder start.
    assert!(tail < head - 10.0, "head {head:.1}, tail {tail:.1}");
    assert!(a.silences.is_empty());
    // The envelope falls through the fade.
    let during: Vec<f64> = (20..25).map(|s| a.envelope.lufs(s).unwrap()).collect();
    assert!(during.windows(2).all(|w| w[1] < w[0]), "{during:?}");
}

#[test]
fn a_track_without_silence() {
    let a = analyze_parts("full.wav", &[Part::Tone(-14.0, 10.0)]).analysis;
    assert_eq!(a.audio, Some(0..at(10.0)));
    assert_eq!((a.leading_silence(), a.trailing_silence()), (0, 0));
    assert!(a.silences.is_empty());
    let integrated = a.integrated_lufs.unwrap();
    assert_near("head", a.head_lufs.unwrap(), integrated, 0.2);
    assert_near("tail", a.tail_lufs.unwrap(), integrated, 0.2);
    // Without fades, the points are half a window from the edges.
    assert_frames("fade-in end", a.fade_in_end.unwrap(), at(0.5), WINDOW);
    assert_frames("fade-out start", a.fade_out_start.unwrap(), at(9.5), WINDOW);
    assert_eq!(a.envelope.len(), 10);
    for second in 0..10 {
        assert_near(
            "envelope",
            a.envelope.lufs(second).unwrap(),
            integrated,
            0.5,
        );
    }
}

#[test]
fn a_silent_track() {
    let a = analyze_parts("silent.wav", &[Part::Silence(5.0)]).analysis;
    assert!(a.is_silent());
    assert_eq!(a.frames, at(5.0));
    assert_eq!(a.leading_silence(), at(5.0));
    assert_eq!(a.integrated_lufs, None);
    assert_eq!((a.head_lufs, a.tail_lufs), (None, None));
    assert_eq!((a.fade_in_end, a.fade_out_start), (None, None));
    assert_eq!(a.envelope.len(), 5);
    assert!((0..5).all(|s| a.envelope.lufs(s).is_none()));
}

#[test]
fn sections_of_a_file_are_analysed_on_their_own() {
    let path = write_wav(
        "sections.wav",
        &stereo(&[
            Part::Tone(-20.0, 5.0),
            Part::Silence(2.0),
            Part::Tone(-30.0, 5.0),
        ]),
    );
    let rate = SampleRate::new(RATE).unwrap();
    let time = |secs: u64| MediaTime::from_frames(secs * u64::from(RATE), rate);
    let first = MediaSource::file(&path).with_section(SourceSection::Span {
        start: time(0),
        end: Some(time(6)),
    });
    let second = MediaSource::file(&path).with_section(SourceSection::Span {
        start: time(6),
        end: None,
    });
    let first = analyze(&first, true);
    let second = analyze(&second, true);

    let a = &first.analysis;
    assert_eq!(a.frames, at(6.0));
    assert_eq!(a.audio.as_ref().unwrap().start, 0);
    assert_frames("first: trailing", a.trailing_silence(), at(1.0), WINDOW);
    let b = &second.analysis;
    assert_eq!(b.frames, at(6.0));
    assert_frames("second: leading", b.leading_silence(), at(1.0), WINDOW);
    assert_eq!(b.trailing_silence(), 0);
    assert_near(
        "second: loudness",
        b.integrated_lufs.unwrap(),
        a.integrated_lufs.unwrap() - 10.0,
        0.2,
    );

    // The loudness measurement taken in the same pass is that of the scanner.
    let registry = DecoderRegistry::with_default_backends();
    let source = MediaSource::file(&path).with_section(SourceSection::Span {
        start: time(6),
        end: None,
    });
    let alone = measure_source(&registry, &source, &|| false).unwrap();
    let with = second.loudness.unwrap();
    assert_eq!(with.loudness, alone.loudness);
    assert_eq!(with.codec, "pcm_s16le");
    assert_eq!(second.codec, "pcm_s16le");
}

#[test]
fn cancelling_and_failing() {
    let path = write_wav("cancel.wav", &stereo(&[Part::Tone(-20.0, 2.0)]));
    let registry = DecoderRegistry::with_default_backends();
    let result = analyze_source(&registry, &MediaSource::file(&path), false, &mut || true);
    assert!(
        matches!(result, Err(AnalysisError::Cancelled)),
        "{result:?}"
    );
    let missing = dir().join("missing.wav");
    let result = analyze_source(&registry, &MediaSource::file(&missing), false, &mut || {
        false
    });
    assert!(
        matches!(result, Err(AnalysisError::Decode(_))),
        "{result:?}"
    );
}
