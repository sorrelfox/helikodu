// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Helpers for the tests that run the built program on media made by the `ffmpeg`
//! tool: running the program, test folders, tagged audio files and reading the WAV
//! files the `wav` backend writes.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

/// Runs the program with `args`, without input.
pub(crate) fn cli<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_helikodu-cli"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run helikodu-cli")
}

/// Standard output or error as text.
pub(crate) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A fresh, empty folder for one test.
pub(crate) fn folder(group: &str, test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(group)
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the test folder");
    dir
}

/// Whether the `ffmpeg` tool runs; prints why a test is skipped when it does not.
pub(crate) fn have_ffmpeg() -> bool {
    static HAVE: OnceLock<bool> = OnceLock::new();
    let have = *HAVE.get_or_init(|| {
        Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    });
    if !have {
        eprintln!("SKIPPED: ffmpeg is not installed");
    }
    have
}

/// Whether this `ffmpeg` has `encoder`; prints why a test is skipped when not.
pub(crate) fn have_encoder(encoder: &str) -> bool {
    let have = Command::new("ffmpeg")
        .args(["-hide_banner", "-h", &format!("encoder={encoder}")])
        .output()
        .is_ok_and(|o| o.status.success() && !text(&o.stdout).contains("is not recognized"));
    if !have {
        eprintln!("SKIPPED: ffmpeg has no {encoder} encoder");
    }
    have
}

/// The sample rate of the generated files.
pub(crate) const RATE: u32 = 48_000;

/// A stereo sine of `hz` (peaks near 0.09) for `seconds`, encoded by `codec`
/// into `path` with the tags `tags`. Creates the folders it needs.
pub(crate) fn tone(path: &Path, codec: &str, hz: u32, seconds: u32, tags: &[(&str, &str)]) {
    fs::create_dir_all(path.parent().expect("a folder")).expect("create the folder");
    let mut command = Command::new("ffmpeg");
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg(format!(
            "sine=frequency={hz}:duration={seconds}:sample_rate={RATE}"
        ))
        .args(["-ac", "2", "-c:a", codec]);
    for (key, value) in tags {
        command.arg("-metadata").arg(format!("{key}={value}"));
    }
    let output = command.arg(path).output().expect("run ffmpeg");
    assert!(
        output.status.success(),
        "ffmpeg could not write {}: {}",
        path.display(),
        text(&output.stderr)
    );
}

/// The samples of a WAV file (16, 24 or 32-bit integers, or 32-bit floats) as
/// interleaved values from -1.0 to 1.0, and the number of channels.
pub(crate) fn read_wav(path: &Path) -> (Vec<f64>, usize) {
    let bytes = fs::read(path).expect("read the WAV file");
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let mut at = 12;
    let mut format = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32_at(at + 4) as usize;
        let body = at + 8;
        if id == b"fmt " {
            let mut tag = u16_at(body);
            if tag == 0xFFFE {
                // WAVE_FORMAT_EXTENSIBLE: the format is the start of the sub-format GUID.
                tag = u16_at(body + 24);
            }
            format = Some((tag, usize::from(u16_at(body + 2)), u16_at(body + 14)));
        } else if id == b"data" {
            let (tag, channels, bits) = format.expect("fmt before data");
            let data = &bytes[body..(body + len).min(bytes.len())];
            let samples = match (tag, bits) {
                (3, 32) => data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f64::from(f32::from_le_bytes(*c)))
                    .collect(),
                (1, 16) => data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| f64::from(i16::from_le_bytes(*c)) / 32_768.0)
                    .collect(),
                (1, 24) => data
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|[a, b, c]| {
                        f64::from(i32::from_le_bytes([0, *a, *b, *c]) >> 8) / 8_388_608.0
                    })
                    .collect(),
                (1, 32) => data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f64::from(i32::from_le_bytes(*c)) / 2_147_483_648.0)
                    .collect(),
                other => panic!("unexpected WAV format {other:?}"),
            };
            return (samples, channels);
        }
        at = body + len + (len & 1);
    }
    panic!("no data chunk in {}", path.display());
}

/// The RMS level of `samples`.
pub(crate) fn rms(samples: &[f64]) -> f64 {
    (samples.iter().map(|s| s * s).sum::<f64>() / samples.len() as f64).sqrt()
}

/// The `helikodu-scan-worker` program, built on first use into this build's target
/// folder, beside `helikodu-cli`.
///
/// It is a program of another package, so Cargo does not build it for these tests.
pub(crate) fn worker_program() -> PathBuf {
    static PROGRAM: OnceLock<PathBuf> = OnceLock::new();
    PROGRAM
        .get_or_init(|| {
            let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
                .parent()
                .expect("the target folder")
                .to_path_buf();
            let release = !cfg!(debug_assertions);
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let mut command = Command::new(cargo);
            command
                .args([
                    "build",
                    "--quiet",
                    "-p",
                    "helikodu-scan-worker",
                    "--target-dir",
                ])
                .arg(&target)
                .current_dir(env!("CARGO_MANIFEST_DIR"));
            if release {
                command.arg("--release");
            }
            let status = command.status().expect("run cargo build");
            assert!(status.success(), "building the scan worker failed");
            let program = target
                .join(if release { "release" } else { "debug" })
                .join("helikodu-scan-worker");
            assert!(program.is_file(), "{} was not built", program.display());
            program
        })
        .clone()
}
