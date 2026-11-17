// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Helpers for scanner tests: the worker program, test folders and audio made by the
//! `ffmpeg` tool.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The `helikodu-scan-worker` program of this build, built on first use.
///
/// It is a program of another package, so Cargo does not build it for this crate's
/// tests; this runs `cargo build -p helikodu-scan-worker` into the same target
/// folder and profile.
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

/// A fresh, empty folder named `name` for one test.
pub(crate) fn folder(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("library-scan")
        .join(name);
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

/// A FLAC file at `path` with a tone of `hz` for `seconds`, tagged with `tags`
/// (`key`, `value`). Creates the folders it needs.
pub(crate) fn flac(path: impl AsRef<Path>, hz: u32, seconds: u32, tags: &[(&str, &str)]) {
    let path = path.as_ref();
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
            "sine=frequency={hz}:duration={seconds}:sample_rate=44100"
        ))
        .args(["-ac", "2"]);
    for (key, value) in tags {
        command.arg("-metadata").arg(format!("{key}={value}"));
    }
    let output = command
        .args(["-c:a", "flac", "-f", "flac"])
        .arg(path)
        .output()
        .expect("run ffmpeg");
    assert!(
        output.status.success(),
        "ffmpeg could not write {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Writes `text` to `path`, creating the folders it needs.
pub(crate) fn write(path: impl AsRef<Path>, text: impl AsRef<[u8]>) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().expect("a folder")).expect("create the folder");
    fs::write(path, text).expect("write the file");
}

/// The path `dir` joined with a name given as bytes, which need not be UTF-8.
pub(crate) fn byte_path(dir: &Path, name: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    dir.join(OsStr::from_bytes(name))
}
