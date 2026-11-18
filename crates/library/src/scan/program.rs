// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Finding the worker program.

use std::path::{Path, PathBuf};

use super::progress::ScanError;

/// The file name of the worker program.
pub const WORKER_PROGRAM: &str = "helikodu-scan-worker";

/// The folder an installation puts the worker in, set at build time (Meson's
/// `helperdir` option sets `HELIKODU_HELPERDIR`); `None` for a plain Cargo build.
const HELPER_DIR: Option<&str> = option_env!("HELIKODU_HELPERDIR");

/// The worker program: `explicit` if given, else `helikodu-scan-worker` beside the
/// running program, in `../libexec/helikodu/` from it, or in the folder chosen
/// at build time.
pub fn find_worker_program(explicit: Option<&Path>) -> Result<PathBuf, ScanError> {
    let candidates: Vec<PathBuf> = match explicit {
        Some(path) => vec![path.to_path_buf()],
        None => {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf));
            worker_candidates(exe_dir.as_deref(), HELPER_DIR.map(Path::new))
        }
    };
    candidates
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .ok_or(ScanError::WorkerNotFound(candidates))
}

/// Where the worker is looked for, in order. The places relative to the
/// running program come first, so that a copy of the programs moved elsewhere
/// (an unpacked archive, an AppImage) uses its own worker.
fn worker_candidates(exe_dir: Option<&Path>, helper_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = exe_dir {
        candidates.push(dir.join(WORKER_PROGRAM));
        candidates.push(dir.join("../libexec/helikodu").join(WORKER_PROGRAM));
    }
    if let Some(dir) = helper_dir.filter(|dir| !dir.as_os_str().is_empty()) {
        let path = dir.join(WORKER_PROGRAM);
        if !candidates.contains(&path) {
            candidates.push(path);
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_beside_the_program_then_in_libexec_then_in_the_helper_dir() {
        let candidates = worker_candidates(
            Some(Path::new("/usr/bin")),
            Some(Path::new("/usr/lib/helikodu")),
        );
        assert_eq!(
            candidates,
            [
                PathBuf::from("/usr/bin/helikodu-scan-worker"),
                PathBuf::from("/usr/bin/../libexec/helikodu/helikodu-scan-worker"),
                PathBuf::from("/usr/lib/helikodu/helikodu-scan-worker"),
            ]
        );
    }

    #[test]
    fn without_a_helper_dir_only_the_relative_places_are_tried() {
        let expected = [
            PathBuf::from("/opt/x/bin/helikodu-scan-worker"),
            PathBuf::from("/opt/x/bin/../libexec/helikodu/helikodu-scan-worker"),
        ];
        assert_eq!(
            worker_candidates(Some(Path::new("/opt/x/bin")), None),
            expected
        );
        assert_eq!(
            worker_candidates(Some(Path::new("/opt/x/bin")), Some(Path::new(""))),
            expected
        );
    }

    #[test]
    fn the_helper_dir_is_used_when_the_program_has_no_folder() {
        assert_eq!(
            worker_candidates(None, Some(Path::new("/usr/lib/helikodu"))),
            [PathBuf::from("/usr/lib/helikodu/helikodu-scan-worker")]
        );
    }

    #[test]
    fn the_helper_dir_is_not_listed_twice() {
        let candidates = worker_candidates(
            Some(Path::new("/usr/bin")),
            Some(Path::new("/usr/bin/../libexec/helikodu")),
        );
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn an_explicit_program_is_the_only_candidate() {
        let missing = Path::new("/nonexistent/helikodu-scan-worker");
        match find_worker_program(Some(missing)) {
            Err(ScanError::WorkerNotFound(candidates)) => {
                assert_eq!(candidates, [missing.to_path_buf()]);
            }
            other => panic!("expected WorkerNotFound, got {other:?}"),
        }
    }
}
