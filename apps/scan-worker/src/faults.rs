// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deliberate failures for testing the scanner, in debug builds only.
//!
//! When `HELIKODU_SCAN_WORKER_FAULTS=1` is set, a debug build of the worker misbehaves
//! on files whose name contains a marker:
//!
//! | Marker | What the worker does |
//! |---|---|
//! | `crash-worker` | aborts, as a decoder that crashes would |
//! | `hang-worker` | never answers, as a decoder stuck in a loop would |
//! | `hog-worker` | allocates 4 GiB, which the address-space limit refuses |
//!
//! A release build ignores the variable.

use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// The environment variable that turns the faults on.
const VARIABLE: &str = "HELIKODU_SCAN_WORKER_FAULTS";

/// Whether the faults are on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Faults {
    enabled: bool,
}

impl Faults {
    pub(crate) fn from_env() -> Self {
        Self {
            enabled: cfg!(debug_assertions)
                && std::env::var_os(VARIABLE).is_some_and(|v| v.as_bytes() == b"1"),
        }
    }

    /// Misbehaves if the faults are on and the file name of `path` has a marker.
    pub(crate) fn inject(self, path: &Path) {
        if !self.enabled {
            return;
        }
        let Some(name) = path.file_name().map(OsStrExt::as_bytes) else {
            return;
        };
        let has = |marker: &[u8]| name.windows(marker.len()).any(|w| w == marker);
        if has(b"crash-worker") {
            std::process::abort();
        }
        if has(b"hang-worker") {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        if has(b"hog-worker") {
            let hog = vec![1_u8; 4 << 30];
            std::hint::black_box(&hog);
        }
    }
}
