// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The loudness scanner against signals in the manner of EBU Tech 3341 (loudness)
//! and Tech 3342 (loudness range), generated here: integrated loudness and range
//! within ±0.1 LU, true peak of an inter-sample peak, album values of a 3-track
//! album, and files and CUE-style sections measured through the decoder registry.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::f64::consts::PI;
use std::fs;
use std::path::{Path, PathBuf};

use helikodu_engine::analysis::{
    AnalysisError, LoudnessMeter, TrackMeasurement, album_loudness, measure_source,
};
use helikodu_engine::decode::DecoderRegistry;
use helikodu_model::audio::{ChannelLayout, MediaTime, SampleRate};
use helikodu_model::library::{MediaSource, SourceSection};

const RATE: u32 = 48_000;

/// One stretch of a test signal: a stereo sine of `hz` at `dbfs` (peak level of
/// each channel) for `secs` seconds.
#[derive(Clone, Copy)]
struct Part {
    dbfs: f64,
    secs: f64,
    hz: f64,
}

fn tone(dbfs: f64, secs: f64) -> Part {
    Part {
        dbfs,
        secs,
        hz: 1000.0,
    }
}

/// The interleaved stereo samples of `parts`, played one after the other with a
/// continuous phase.
fn stereo(parts: &[Part]) -> Vec<f32> {
    let mut out = Vec::new();
    let mut phase = 0.0_f64;
    for part in parts {
        let amplitude = 10f64.powf(part.dbfs / 20.0);
        let frames = (part.secs * f64::from(RATE)).round() as usize;
        let step = 2.0 * PI * part.hz / f64::from(RATE);
        for _ in 0..frames {
            let v = (amplitude * phase.sin()) as f32;
            out.extend([v, v]);
            phase = (phase + step) % (2.0 * PI);
        }
    }
    out
}

fn measure(samples: &[f32]) -> TrackMeasurement {
    let mut meter =
        LoudnessMeter::new(SampleRate::new(RATE).unwrap(), ChannelLayout::STEREO).unwrap();
    for chunk in samples.chunks(4096 * 2) {
        meter.add_f32(chunk).unwrap();
    }
    meter.finish().unwrap()
}

fn integrated(parts: &[Part]) -> f64 {
    measure(&stereo(parts)).loudness.integrated_lufs.unwrap()
}

fn range(parts: &[Part]) -> f64 {
    measure(&stereo(parts)).loudness.range_lu
}

fn assert_near(what: &str, measured: f64, expected: f64, tolerance: f64) {
    assert!(
        (measured - expected).abs() <= tolerance,
        "{what}: measured {measured:.3}, expected {expected:.3} ± {tolerance}"
    );
}

#[test]
fn tech_3341_integrated_loudness() {
    // Cases 1 to 5 of Tech 3341: stereo 1 kHz sines.
    assert_near("case 1", integrated(&[tone(-23.0, 20.0)]), -23.0, 0.1);
    assert_near("case 2", integrated(&[tone(-33.0, 20.0)]), -33.0, 0.1);
    assert_near(
        "case 3 (relative gate)",
        integrated(&[tone(-36.0, 10.0), tone(-23.0, 60.0), tone(-36.0, 10.0)]),
        -23.0,
        0.1,
    );
    assert_near(
        "case 4 (absolute and relative gate)",
        integrated(&[
            tone(-72.0, 10.0),
            tone(-36.0, 10.0),
            tone(-23.0, 60.0),
            tone(-36.0, 10.0),
            tone(-72.0, 10.0),
        ]),
        -23.0,
        0.1,
    );
    assert_near(
        "case 5",
        integrated(&[tone(-26.0, 20.0), tone(-20.0, 20.1), tone(-26.0, 20.0)]),
        -23.0,
        0.1,
    );
}

#[test]
fn track_gain_is_relative_to_minus_18_lufs() {
    let measured = measure(&stereo(&[tone(-23.0, 20.0)]));
    assert_near("gain", measured.loudness.gain_db().unwrap(), 5.0, 0.1);
    assert_eq!(measured.loudness.frames, u64::from(RATE) * 20);
    assert_eq!(measured.loudness.rate, RATE);
}

#[test]
fn tech_3342_loudness_range() {
    // Cases 1 to 4 of Tech 3342: stereo 1 kHz sines.
    assert_near(
        "case 1",
        range(&[tone(-20.0, 20.0), tone(-30.0, 20.0)]),
        10.0,
        0.1,
    );
    assert_near(
        "case 2",
        range(&[tone(-20.0, 20.0), tone(-15.0, 20.0)]),
        5.0,
        0.1,
    );
    assert_near(
        "case 3",
        range(&[tone(-40.0, 20.0), tone(-20.0, 20.0)]),
        20.0,
        0.1,
    );
    assert_near(
        "case 4",
        range(&[
            tone(-50.0, 20.0),
            tone(-35.0, 20.0),
            tone(-20.0, 20.0),
            tone(-35.0, 20.0),
            tone(-50.0, 20.0),
        ]),
        15.0,
        0.1,
    );
}

#[test]
fn true_peak_finds_the_peak_between_samples() {
    // A sine at a quarter of the sample rate, 45° out of phase with the samples:
    // every sample is at ±0.707 of the amplitude, the waveform peaks between them.
    let frames = RATE as usize * 2;
    let mut samples = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let v = (2.0 * PI * (n as f64) / 4.0 + PI / 4.0).sin() as f32;
        samples.extend([v, v]);
    }
    let measured = measure(&samples).loudness;
    let db = |linear: f64| 20.0 * linear.log10();
    assert_near("sample peak", db(measured.sample_peak), -3.01, 0.01);
    // Tech 3341 allows −0.4 to +0.2 dB for true-peak meters.
    let true_peak = db(measured.true_peak);
    assert!(
        (-0.4..=0.2).contains(&true_peak),
        "true peak {true_peak:.3} dBTP, expected 0 dBTP"
    );
}

#[test]
fn album_is_one_programme_not_the_mean_of_its_tracks() {
    let parts = [tone(-20.0, 10.0), tone(-26.0, 10.0), tone(-23.0, 10.0)];
    let tracks: Vec<TrackMeasurement> = parts
        .iter()
        .map(|part| measure(&stereo(&[*part])))
        .collect();
    for (track, part) in tracks.iter().zip(&parts) {
        assert_near(
            "track",
            track.loudness.integrated_lufs.unwrap(),
            part.dbfs,
            0.1,
        );
    }
    let album = album_loudness(&tracks).unwrap();
    // Equal lengths, all above the relative gate: the mean of the energies.
    let energy: f64 = parts.iter().map(|p| 10f64.powf(p.dbfs / 10.0)).sum::<f64>() / 3.0;
    let expected = 10.0 * energy.log10();
    assert_near("album", album.integrated_lufs.unwrap(), expected, 0.1);
    // The same signal measured in one go.
    assert_near(
        "album",
        album.integrated_lufs.unwrap(),
        integrated(&parts),
        0.1,
    );
    let mean_gain = tracks
        .iter()
        .map(|t| t.loudness.gain_db().unwrap())
        .sum::<f64>()
        / 3.0;
    assert!((album.gain_db().unwrap() - mean_gain).abs() > 0.5);
    assert_near(
        "album gain",
        album.gain_db().unwrap(),
        -18.0 - expected,
        0.1,
    );
    let loudest = 10f64.powf(-20.0 / 20.0);
    assert_near("album peak", album.sample_peak, loudest, 1e-3);
    assert!(album.true_peak >= album.sample_peak);
    assert!(album.range_lu > 3.0, "{}", album.range_lu);
}

fn dir() -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine-loudness");
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

#[test]
fn files_and_sections_through_the_registry() {
    let path = write_wav(
        "two-parts.wav",
        &stereo(&[tone(-30.0, 10.0), tone(-20.0, 10.0)]),
    );
    let registry = DecoderRegistry::with_default_backends();
    let never = || false;

    let whole = measure_source(&registry, &MediaSource::file(&path), &never).unwrap();
    assert_eq!(whole.codec, "pcm_s16le");
    assert_eq!(whole.loudness.frames, u64::from(RATE) * 20);

    let rate = SampleRate::new(RATE).unwrap();
    let at = |secs: u64| MediaTime::from_frames(secs * u64::from(RATE), rate);
    let first = MediaSource::file(&path).with_section(SourceSection::Span {
        start: at(0),
        end: Some(at(10)),
    });
    let second = MediaSource::file(&path).with_section(SourceSection::Span {
        start: at(10),
        end: None,
    });
    let first = measure_source(&registry, &first, &never).unwrap();
    let second = measure_source(&registry, &second, &never).unwrap();
    assert_eq!(first.loudness.frames, u64::from(RATE) * 10);
    assert_eq!(second.loudness.frames, u64::from(RATE) * 10);
    assert_near("first", first.loudness.integrated_lufs.unwrap(), -30.0, 0.1);
    assert_near(
        "second",
        second.loudness.integrated_lufs.unwrap(),
        -20.0,
        0.1,
    );
    // The sections as an album are the whole file.
    let album = album_loudness([&first, &second]).unwrap();
    assert_near(
        "album of sections",
        album.integrated_lufs.unwrap(),
        whole.loudness.integrated_lufs.unwrap(),
        0.1,
    );
}

#[test]
fn cancelling_and_failing() {
    let path = write_wav("cancel.wav", &stereo(&[tone(-20.0, 2.0)]));
    let registry = DecoderRegistry::with_default_backends();
    let result = measure_source(&registry, &MediaSource::file(&path), &|| true);
    assert!(
        matches!(result, Err(AnalysisError::Cancelled)),
        "{result:?}"
    );

    let missing = dir().join("missing.wav");
    let result = measure_source(&registry, &MediaSource::file(&missing), &|| false);
    assert!(
        matches!(result, Err(AnalysisError::Decode(_))),
        "{result:?}"
    );
}
