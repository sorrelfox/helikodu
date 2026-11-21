// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The built program: arguments, exit codes and output, playing generated WAV files
//! to the WAV and null sinks.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const RATE: u32 = 44_100;

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_helikodu-cli"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn folder(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cli")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `frames` frames of a quiet 16-bit stereo ramp as a WAV file.
fn write_wav(path: &Path, frames: u32) {
    let data_len = frames * 4;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    for value in [16u32.to_le_bytes(), [1, 0, 2, 0], RATE.to_le_bytes()] {
        bytes.extend_from_slice(&value);
    }
    bytes.extend_from_slice(&(RATE * 4).to_le_bytes());
    bytes.extend_from_slice(&[4, 0, 16, 0]);
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..frames {
        let sample = ((frame % 200) as i16 - 100) * 10;
        bytes.extend_from_slice(&sample.to_le_bytes());
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn version() {
    let output = cli(&["--version"]);
    assert!(output.status.success());
    assert!(text(&output.stdout).starts_with("helikodu-cli "));
}

#[test]
fn plays_files_to_a_wav_file() {
    let dir = folder("wav");
    let (a, b, out) = (dir.join("a.wav"), dir.join("b.wav"), dir.join("out.wav"));
    write_wav(&a, 10_000);
    write_wav(&b, 5_000);
    let output = cli(&[
        "play",
        "--backend",
        "wav",
        "--wav-out",
        out.to_str().unwrap(),
        a.to_str().unwrap(),
        b.to_str().unwrap(),
    ]);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("Playing 1/2"), "{stderr}");
    assert!(stderr.contains("Playing 2/2"), "{stderr}");
    assert!(output.stdout.is_empty());
    // 15 000 frames of 16-bit stereo, after the header.
    let written = fs::read(&out).unwrap();
    let data = written.windows(4).position(|w| w == b"data").unwrap();
    let size = u32::from_le_bytes(written[data + 4..data + 8].try_into().unwrap());
    assert_eq!(size, 15_000 * 4);
    assert_eq!(written.len(), data + 8 + 15_000 * 4);
}

#[test]
fn skips_unplayable_files() {
    let dir = folder("skip");
    let good = dir.join("good.wav");
    write_wav(&good, 4_410);
    let output = cli(&[
        "play",
        "--backend",
        "null",
        "--volume",
        "50",
        dir.join("missing.flac").to_str().unwrap(),
        good.to_str().unwrap(),
    ]);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("missing.flac"), "{stderr}");
    assert!(stderr.contains("1 could not be played"), "{stderr}");
}

#[test]
fn nothing_playable_has_its_own_exit_code() {
    let dir = folder("nothing");
    let broken = dir.join("broken.flac");
    fs::write(&broken, b"not audio").unwrap();
    let output = cli(&[
        "play",
        "--backend",
        "null",
        broken.to_str().unwrap(),
        dir.join("missing.flac").to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output.stderr));
}

#[test]
fn usage_errors_exit_with_2() {
    for args in [
        &["play"][..],
        &["play", "--backend", "wav", "a.wav"],
        &["play", "--wav-out", "out.wav", "a.wav"],
        &["play", "--volume", "101", "a.wav"],
        &["play", "--backend", "coreaudio", "a.wav"],
        &["play", "--device", "", "a.wav"],
        &[
            "play",
            "--backend",
            "wav",
            "--wav-out",
            "o.wav",
            "--device",
            "x",
            "a.wav",
        ],
        &["list-outputs", "--backend", "jack"],
        &["no-such-command"],
    ] {
        let output = cli(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(!output.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn list_outputs_as_json() {
    let output = cli(&["list-outputs", "--backend", "null", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let devices: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let devices = devices.as_array().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["backend"], "null");
    assert_eq!(devices[0]["id"], "null");
    assert_eq!(devices[0]["is_default"], true);
}

#[test]
fn list_outputs_for_people() {
    let output = cli(&["list-outputs", "--backend", "null"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = text(&output.stdout);
    assert!(stdout.starts_with("null:\n* null  "), "{stdout}");
}

#[test]
fn list_outputs_failure_exits_with_1() {
    let output = cli(&["list-outputs", "--backend", "wav"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("wav"));
}

#[test]
fn dsp_speed_writes_the_settings_file() {
    let dir = folder("dsp-speed");
    let settings = dir.join("settings.toml");
    let path = settings.to_str().unwrap();
    let output = cli(&["dsp", "--settings", path, "speed", "1.25", "--tape"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "Speed: 1.25×, the pitch following it.\n"
    );
    let written = fs::read_to_string(&settings).unwrap();
    assert!(written.contains("[dsp.speed]"), "{written}");
    assert!(written.contains("rate = 1.25"), "{written}");
    assert!(written.contains("keep_pitch = false"), "{written}");
    // Without a choice the pitch does as before; 1 switches the speed off.
    let output = cli(&["dsp", "--settings", path, "speed", "0.8"]);
    assert_eq!(
        text(&output.stdout),
        "Speed: 0.8×, the pitch following it.\n"
    );
    let output = cli(&["dsp", "--settings", path, "speed", "1", "--keep-pitch"]);
    assert_eq!(text(&output.stdout), "The speed is normal.\n");
    let written = fs::read_to_string(&settings).unwrap();
    assert!(written.contains("enabled = false") && written.contains("keep_pitch = true"));
    let output = cli(&["dsp", "--settings", path, "speed", "2.5"]);
    assert_eq!(output.status.code(), Some(1));
    let output = cli(&[
        "dsp",
        "--settings",
        path,
        "speed",
        "1.5",
        "--tape",
        "--keep-pitch",
    ]);
    assert_eq!(output.status.code(), Some(2));
}
