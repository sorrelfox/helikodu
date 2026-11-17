// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Finding the worker program.

use std::path::{Path, PathBuf};

use super::progress::ScanError;

/// The file name of the worker program.
pub const WORKER_PROGRAM: &str = "helikodu-scan-worker";

/// The worker program: `explicit` if given, else `helikodu-scan-worker` beside the
/// running program, or in `../libexec/helikodu/` from it.
pub fn find_worker_program(explicit: Option<&Path>) -> Result<PathBuf, ScanError> {
    let candidates: Vec<PathBuf> = match explicit {
        Some(path) => vec![path.to_path_buf()],
        None => std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .map(|dir| {
                vec![
                    dir.join(WORKER_PROGRAM),
                    dir.join("../libexec/helikodu").join(WORKER_PROGRAM),
                ]
            })
            .unwrap_or_default(),
    };
    candidates
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .ok_or(ScanError::WorkerNotFound(candidates))
}
