// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Crash reports, kept on this computer.
//!
//! [`install_panic_hook`] makes every panic write a report to
//! `$XDG_STATE_HOME/helikodu/crashes/crash-YYYYMMDD-HHMMSS-<pid>.txt` before
//! the usual panic message: the version, the thread, the message, where it
//! happened, a backtrace and the last warnings of the log. A report is written
//! even when the log is off, because it is only written when something went
//! wrong, it never leaves the computer, and the user decides whether to copy
//! it; it then says that no log lines are included. The newest
//! [`KEPT_REPORTS`] are kept.
//!
//! At the next start [`unseen_reports`] lists the reports not shown yet, so the
//! window can say what happened; [`mark_seen`] records that they were shown.

use std::backtrace::Backtrace;
use std::cell::Cell;
use std::fmt::Write as _;
use std::fs;
use std::panic::{self, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::files::write_atomic;
use crate::logging::{self, LogDetail};

/// The crash report folder inside the state folder.
pub const CRASH_FOLDER: &str = "crashes";
/// How many reports are kept.
pub const KEPT_REPORTS: usize = 10;
/// How many log lines a report holds at most.
const LOG_LINES: usize = 50;
/// The file that names the newest report already shown.
const SEEN_FILE: &str = "seen";

/// A crash report on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashReport {
    /// The file.
    pub path: PathBuf,
    /// The panic message, as the report's summary line holds it.
    pub summary: String,
}

impl CrashReport {
    /// The whole text of the report.
    pub fn text(&self) -> String {
        fs::read_to_string(&self.path).unwrap_or_default()
    }
}

/// The crash folder of the state folder `state`.
pub fn crash_folder(state: &Path) -> PathBuf {
    state.join(CRASH_FOLDER)
}

/// Makes every panic write a crash report into `<state>/crashes` first. The
/// panic then goes on as before.
pub fn install_panic_hook(state: &Path) {
    let folder = crash_folder(state);
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        thread_local! {
            static WRITING: Cell<bool> = const { Cell::new(false) };
        }
        // A panic while the report is written must not write another one.
        if !WRITING.with(|w| w.replace(true)) {
            let text = report_text(info, SystemTime::now());
            let _ = write_report(&folder, &text, SystemTime::now());
            WRITING.with(|w| w.set(false));
        }
        previous(info);
    }));
}

fn payload_text(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".to_owned())
}

fn report_text(info: &PanicHookInfo<'_>, at: SystemTime) -> String {
    let thread = std::thread::current();
    let message = payload_text(info);
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_default();
    let mut text = String::new();
    let _ = writeln!(text, "Summary: {}", first_line(&message));
    let _ = writeln!(
        text,
        "Helikodu {} ({}, {} build)",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    let _ = writeln!(text, "Time: {}", logging::utc_time(at));
    let _ = writeln!(text, "Thread: {}", thread.name().unwrap_or("unnamed"));
    let _ = writeln!(text, "Where: {location}");
    let _ = writeln!(text, "\nMessage:\n{message}");
    let _ = writeln!(text, "\nBacktrace:\n{}", Backtrace::force_capture());
    match logging::logger() {
        Some(logger) if logger.detail() != LogDetail::Off => {
            let _ = writeln!(text, "Last warnings of the log:");
            for line in logger.recent(LOG_LINES) {
                let _ = writeln!(text, "{line}");
            }
        }
        _ => {
            let _ = writeln!(
                text,
                "No log lines are included: the log is off (Settings > Advanced > Logs)."
            );
        }
    }
    let _ = writeln!(
        text,
        "\nThis report was written on this computer only. Helikodu sends nothing anywhere."
    );
    text
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

/// Writes `text` as a new report in `folder` and removes the oldest beyond
/// [`KEPT_REPORTS`].
pub fn write_report(folder: &Path, text: &str, at: SystemTime) -> std::io::Result<PathBuf> {
    let stamp: String = logging::utc_time(at)
        .chars()
        .filter(char::is_ascii_digit)
        .take(14)
        .collect();
    let name = format!(
        "crash-{}-{}-{}.txt",
        &stamp[..8.min(stamp.len())],
        &stamp[8.min(stamp.len())..],
        std::process::id()
    );
    let path = folder.join(name);
    write_atomic(&path, text.as_bytes())?;
    for stale in reports(folder).into_iter().skip(KEPT_REPORTS) {
        let _ = fs::remove_file(stale);
    }
    Ok(path)
}

/// The report files in `folder`, newest first.
fn reports(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("crash-") && n.ends_with(".txt"))
        })
        .collect();
    // The names start with the time, so the name order is the time order.
    found.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    found
}

/// The reports in `<state>/crashes` not shown yet, newest first.
pub fn unseen_reports(state: &Path) -> Vec<CrashReport> {
    let folder = crash_folder(state);
    let seen = fs::read_to_string(folder.join(SEEN_FILE)).unwrap_or_default();
    let seen = seen.trim();
    reports(&folder)
        .into_iter()
        .take_while(|p| p.file_name().and_then(|n| n.to_str()) != Some(seen))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| seen.is_empty() || n > seen)
        })
        .map(|path| {
            let text = fs::read_to_string(&path).unwrap_or_default();
            let summary = text
                .lines()
                .next()
                .and_then(|l| l.strip_prefix("Summary: "))
                .unwrap_or_default()
                .to_owned();
            CrashReport { path, summary }
        })
        .collect()
}

/// Records that every report in `<state>/crashes` was shown.
pub fn mark_seen(state: &Path) -> std::io::Result<()> {
    let folder = crash_folder(state);
    match reports(&folder).first().and_then(|p| p.file_name()) {
        Some(newest) => write_atomic(&folder.join(SEEN_FILE), newest.to_string_lossy().as_bytes()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("helikodu-crash-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn reports_are_listed_until_seen_and_rotated() {
        let state = scratch("list");
        let folder = crash_folder(&state);
        assert!(unseen_reports(&state).is_empty());
        let at = |s: u64| UNIX_EPOCH + Duration::from_secs(1_795_000_000 + s);
        for i in 0..12 {
            write_report(&folder, &format!("Summary: number {i}\nmore"), at(i)).unwrap();
        }
        let unseen = unseen_reports(&state);
        assert_eq!(unseen.len(), KEPT_REPORTS);
        assert_eq!(unseen[0].summary, "number 11");
        assert!(unseen[0].text().contains("more"));
        mark_seen(&state).unwrap();
        assert!(unseen_reports(&state).is_empty());
        write_report(&folder, "Summary: later", at(100)).unwrap();
        let unseen = unseen_reports(&state);
        assert_eq!(unseen.len(), 1);
        assert_eq!(unseen[0].summary, "later");
        fs::remove_dir_all(&state).unwrap();
    }
}
