// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `play --replaygain`: the level the WAV sink writes for files with ReplayGain tags
//! (FLAC Vorbis comments) and Opus R128 tags, in every mode, with the preamp and with
//! clipping prevention. The files are made by the `ffmpeg` tool; without it the tests
//! print `SKIPPED:` and pass.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/media.rs"]
mod support;

use std::path::{Path, PathBuf};

use support::{RATE, cli, folder, have_encoder, have_ffmpeg, read_wav, rms, text, tone};

/// Plays `files` to a WAV file with `options` and returns the RMS level of each file's
/// part of the output, leaving out the first and last 50 ms of each (fades, the
/// codec's edges). Every file is one second long.
fn levels(dir: &Path, name: &str, options: &[&str], files: &[&PathBuf]) -> Vec<f64> {
    let out = dir.join(format!("{name}.wav"));
    let mut args: Vec<String> = vec![
        "play".into(),
        "--backend".into(),
        "wav".into(),
        "--wav-out".into(),
        out.to_str().unwrap().into(),
    ];
    args.extend(options.iter().map(|s| (*s).to_owned()));
    args.extend(files.iter().map(|f| f.to_str().unwrap().to_owned()));
    let output = cli(&args);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let (samples, channels) = read_wav(&out);
    let per_file = RATE as usize * channels;
    assert_eq!(samples.len(), per_file * files.len(), "{name}");
    let edge = per_file / 20;
    samples
        .chunks(per_file)
        .map(|part| rms(&part[edge..part.len() - edge]))
        .collect()
}

/// Whether `measured / reference` is the gain `db`, within `tolerance_db`.
fn assert_gain(measured: f64, reference: f64, db: f64, tolerance_db: f64, what: &str) {
    let actual = 20.0 * (measured / reference).log10();
    assert!(
        (actual - db).abs() <= tolerance_db,
        "{what}: expected {db:+.2} dB, got {actual:+.3} dB"
    );
}

#[test]
fn flac_track_and_album_gain() {
    if !have_ffmpeg() {
        return;
    }
    let dir = folder("replaygain", "flac");
    let file = dir.join("a.flac");
    tone(
        &file,
        "flac",
        440,
        1,
        &[
            ("album", "First"),
            ("REPLAYGAIN_TRACK_GAIN", "-6.00 dB"),
            ("REPLAYGAIN_TRACK_PEAK", "0.125"),
            ("REPLAYGAIN_ALBUM_GAIN", "-9.50 dB"),
            ("REPLAYGAIN_ALBUM_PEAK", "0.130"),
        ],
    );
    let off = levels(&dir, "off", &[], &[&file])[0];
    assert!(off > 0.01, "the tone is missing: {off}");
    let explicit_off = levels(&dir, "off2", &["--replaygain", "off"], &[&file])[0];
    assert!((explicit_off - off).abs() < 1e-9);
    let tolerance = 0.02;
    let track = levels(&dir, "track", &["--replaygain", "track"], &[&file])[0];
    assert_gain(track, off, -6.0, tolerance, "track");
    let album = levels(&dir, "album", &["--replaygain", "album"], &[&file])[0];
    assert_gain(album, off, -9.5, tolerance, "album");
    let preamp = levels(
        &dir,
        "preamp",
        &["--replaygain", "track", "--preamp", "-2.5"],
        &[&file],
    )[0];
    assert_gain(preamp, off, -8.5, tolerance, "track with preamp");
    // A single file has no neighbour from its album: the track gain.
    let auto = levels(&dir, "auto", &["--replaygain", "auto"], &[&file])[0];
    assert_gain(auto, off, -6.0, tolerance, "auto, alone");
}

#[test]
fn auto_uses_the_album_gain_for_neighbours_of_one_album() {
    if !have_ffmpeg() {
        return;
    }
    let dir = folder("replaygain", "auto");
    let tags = |album: &'static str, track: &'static str, album_gain: &'static str| {
        vec![
            ("album", album),
            ("album_artist", "Someone"),
            ("REPLAYGAIN_TRACK_GAIN", track),
            ("REPLAYGAIN_ALBUM_GAIN", album_gain),
        ]
    };
    let single = dir.join("single.flac");
    let first = dir.join("first.flac");
    let second = dir.join("second.flac");
    tone(
        &single,
        "flac",
        440,
        1,
        &tags("Single", "-3.00 dB", "-4.00 dB"),
    );
    tone(
        &first,
        "flac",
        440,
        1,
        &tags("Album", "-2.00 dB", "-8.00 dB"),
    );
    tone(
        &second,
        "flac",
        440,
        1,
        &tags("Album", "-5.00 dB", "-8.00 dB"),
    );
    let files = [&single, &first, &second];
    let off = levels(&dir, "off", &[], &files);
    let auto = levels(&dir, "auto", &["--replaygain", "auto"], &files);
    let tolerance = 0.02;
    assert_gain(auto[0], off[0], -3.0, tolerance, "single: track gain");
    assert_gain(auto[1], off[1], -8.0, tolerance, "first of the album");
    assert_gain(auto[2], off[2], -8.0, tolerance, "second of the album");
    let track = levels(&dir, "track", &["--replaygain", "track"], &files);
    assert_gain(track[1], off[1], -2.0, tolerance, "first, track mode");
    assert_gain(track[2], off[2], -5.0, tolerance, "second, track mode");
}

#[test]
fn clipping_prevention_limits_a_positive_gain_by_the_peak() {
    if !have_ffmpeg() {
        return;
    }
    let dir = folder("replaygain", "clipping");
    let file = dir.join("quiet.flac");
    // +15 dB would take the peak of 0.25 above full scale: the gain stops at
    // 1 / 0.25, +12.04 dB.
    tone(
        &file,
        "flac",
        440,
        1,
        &[
            ("REPLAYGAIN_TRACK_GAIN", "+15.00 dB"),
            ("REPLAYGAIN_TRACK_PEAK", "0.25"),
        ],
    );
    let off = levels(&dir, "off", &[], &[&file])[0];
    let track = levels(&dir, "track", &["--replaygain", "track"], &[&file])[0];
    assert_gain(track, off, -20.0 * 0.25f64.log10(), 0.02, "peak-limited");
}

#[test]
fn files_without_tags_play_unchanged() {
    if !have_ffmpeg() {
        return;
    }
    let dir = folder("replaygain", "untagged");
    let file = dir.join("plain.flac");
    tone(&file, "flac", 440, 1, &[]);
    let off = levels(&dir, "off", &[], &[&file])[0];
    let track = levels(
        &dir,
        "track",
        &["--replaygain", "track", "--preamp", "6"],
        &[&file],
    )[0];
    assert!((track - off).abs() < 1e-9, "{off} {track}");
}

#[test]
fn opus_r128_gain_is_converted_to_the_replaygain_reference() {
    if !have_ffmpeg() || !have_encoder("libopus") {
        return;
    }
    let dir = folder("replaygain", "opus");
    let file = dir.join("a.opus");
    // -2560 / 256 = -10 dB relative to -23 LUFS, so -5 dB relative to -18 LUFS.
    tone(
        &file,
        "libopus",
        440,
        1,
        &[("R128_TRACK_GAIN", "-2560"), ("R128_ALBUM_GAIN", "-1280")],
    );
    let off = levels(&dir, "off", &[], &[&file])[0];
    let track = levels(&dir, "track", &["--replaygain", "track"], &[&file])[0];
    assert_gain(track, off, -5.0, 0.02, "R128 track gain");
    let album = levels(&dir, "album", &["--replaygain", "album"], &[&file])[0];
    assert_gain(album, off, 0.0, 0.02, "R128 album gain");
}

#[test]
fn replaygain_usage_errors_exit_with_2() {
    for args in [
        &["play", "--replaygain", "loud", "a.flac"][..],
        &["play", "--preamp", "30", "a.flac"],
        &["play", "--preamp", "-24.5", "a.flac"],
        &["play", "--preamp", "much", "a.flac"],
    ] {
        let output = cli(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}
