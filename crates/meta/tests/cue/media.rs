// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Folders for test sheets, and audio generated with the `ffmpeg` tool.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// A fresh, empty folder named `name` for one test.
pub(crate) fn folder(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("meta-cue")
        .join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).expect("create the test folder");
    d
}

/// Whether `tool` runs.
pub(crate) fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg(if tool == "ffmpeg" {
            "-version"
        } else {
            "--version"
        })
        .output()
        .is_ok_and(|o| o.status.success())
}

fn have_ffmpeg() -> bool {
    static HAVE: OnceLock<bool> = OnceLock::new();
    *HAVE.get_or_init(|| {
        helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
        have("ffmpeg")
    })
}

/// Ten seconds of a 440 Hz tone at 44.1 kHz, stereo, encoded by ffmpeg into `out`
/// with the extra `args` (codec, metadata, format). `None`, after printing why,
/// when ffmpeg or what the test needs is missing.
pub(crate) fn encode<S: AsRef<OsStr>>(out: &Path, args: &[S]) -> Option<PathBuf> {
    if !have_ffmpeg() {
        eprintln!(
            "SKIPPED: ffmpeg is not installed; {} not tested",
            out.display()
        );
        return None;
    }
    let result = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg("sine=frequency=440:duration=10:sample_rate=44100")
        .args(["-ac", "2"])
        .args(args)
        .arg(out)
        .output()
        .expect("run ffmpeg");
    if !result.status.success() {
        eprintln!(
            "SKIPPED: ffmpeg could not write {}: {}",
            out.display(),
            String::from_utf8_lossy(&result.stderr)
        );
        return None;
    }
    Some(out.to_path_buf())
}
