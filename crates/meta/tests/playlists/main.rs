// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Playlist files through the public API.
//!
//! The files are written by the tests, shaped like those other players write: Windows
//! paths with backslashes and CRLF line ends, byte order marks, legacy charsets,
//! relative, absolute and `file://` entries, URLs, playlists that name each other, and
//! files too large to read.

#![forbid(unsafe_code)]

mod files;
mod round_trip;

use std::fs;
use std::path::{Path, PathBuf};

/// A fresh, empty folder named `name` for one test.
pub(crate) fn folder(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("meta-playlists")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the test folder");
    dir
}

/// Writes `bytes` to `dir/name` and returns the path.
pub(crate) fn write(dir: &Path, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).expect("write the playlist");
    path
}
