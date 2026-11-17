// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli scan`.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use helikodu_service::library::{FailureKind, ScanEvent, ScanRequest, ScanSummary};
use serde_json::{Map, Value, json};

use super::database::{DatabaseArgs, insert_path};
use super::format;
use crate::Outcome;

#[derive(Debug, Args)]
pub(crate) struct ScanArgs {
    /// The folders to scan; each is added to the library [default: every folder
    /// already in the library]
    #[arg(value_name = "FOLDER")]
    folders: Vec<PathBuf>,
    /// Leave out every file and folder whose name matches PATTERN (`*` and `?` are
    /// wildcards), or, when PATTERN contains a `/`, the folder or file at that path.
    /// Can be given more than once.
    #[arg(long, value_name = "PATTERN")]
    exclude: Vec<String>,
    /// The number of worker processes that read the files [default: one per
    /// processor, at most 8]
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u16).range(1..=64))]
    workers: Option<u16>,
    /// The worker program [default: helikodu-scan-worker beside this program]
    #[arg(long, value_name = "PATH")]
    worker: Option<PathBuf>,
    /// Print the summary as JSON, for scripts.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

pub(crate) fn run(args: ScanArgs) -> Result<Outcome> {
    let library = args.database.open()?;
    let scan = library.scan(ScanRequest {
        folders: args.folders,
        exclude: args.exclude,
        workers: args.workers.map_or(0, usize::from),
        worker_program: args.worker,
    })?;
    let mut progress = Progress::new(io::stderr().is_terminal());
    let mut failures = Vec::new();
    for event in scan.events() {
        progress.show(&event);
        if let ScanEvent::FileFailed { path, failure } = event {
            failures.push((path, failure.kind, failure.message));
        }
    }
    progress.clear();
    let summary = scan.join().context("the scan failed")?;

    let mut out = io::stdout().lock();
    if args.json {
        let failures: Vec<Value> = failures
            .iter()
            .map(|(path, kind, message)| {
                let mut object = Map::new();
                insert_path(&mut object, path);
                object.insert("kind".into(), json!(failure_kind(*kind)));
                object.insert("message".into(), json!(message));
                Value::Object(object)
            })
            .collect();
        let mut object = summary_json(&summary);
        object.insert("failures".into(), Value::Array(failures));
        serde_json::to_writer_pretty(&mut out, &object)?;
        writeln!(out)?;
    } else {
        print_summary(&mut out, &summary)?;
    }
    out.flush()?;
    Ok(if summary.unavailable_roots > 0 {
        Outcome::Failure
    } else {
        Outcome::Success
    })
}

/// The progress of a scan on standard error: a line that updates itself on a
/// terminal, otherwise only the steps and the failures.
struct Progress {
    terminal: bool,
    /// A progress line is showing.
    line: bool,
}

impl Progress {
    fn new(terminal: bool) -> Self {
        Self {
            terminal,
            line: false,
        }
    }

    fn show(&mut self, event: &ScanEvent) {
        match event {
            ScanEvent::Walking { root } => self.message(&format!("Scanning {}", root.display())),
            ScanEvent::RootUnavailable { root, error } => {
                self.message(&format!("Cannot read {}: {error}", root.display()));
            }
            ScanEvent::Compared {
                found,
                to_read,
                missing,
            } => self.message(&format!(
                "{found} files found: {to_read} to read, {missing} gone"
            )),
            ScanEvent::Progress { done, total } if self.terminal => {
                let mut err = io::stderr().lock();
                let _ = write!(err, "\r\x1b[2KRead {done} of {total} files");
                let _ = err.flush();
                self.line = true;
            }
            ScanEvent::FileFailed { path, failure } => {
                self.message(&format!("Cannot read {}: {failure}", path.display()));
            }
            _ => {}
        }
    }

    /// A line of its own, above the progress line.
    fn message(&mut self, text: &str) {
        self.clear();
        eprintln!("{text}");
    }

    fn clear(&mut self) {
        if self.line {
            eprint!("\r\x1b[2K");
            self.line = false;
        }
    }
}

fn failure_kind(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::Io => "io",
        FailureKind::Unreadable => "unreadable",
        FailureKind::ReaderFailed => "reader_failed",
        FailureKind::WorkerCrashed => "worker_crashed",
        FailureKind::TimedOut => "timed_out",
        FailureKind::WriteFailed => "write_failed",
        _ => "other",
    }
}

fn summary_json(summary: &ScanSummary) -> Map<String, Value> {
    let value = json!({
        "roots": summary.roots,
        "unavailable_roots": summary.unavailable_roots,
        "found": summary.found,
        "unchanged": summary.unchanged,
        "read": summary.read,
        "added": summary.added,
        "returned": summary.returned,
        "updated": summary.updated,
        "moved": summary.moved,
        "missing": summary.missing,
        "back": summary.back,
        "failed": summary.failed,
        "skipped_unreadable": summary.skipped_unreadable,
        "worker_restarts": summary.worker_restarts,
        "cancelled": summary.cancelled,
        "elapsed_ms": format::millis(summary.elapsed),
    });
    match value {
        Value::Object(object) => object,
        _ => Map::new(),
    }
}

fn print_summary(out: &mut impl Write, s: &ScanSummary) -> io::Result<()> {
    writeln!(
        out,
        "Scanned {} folders in {:.1} s: {} files found, {} read, {} unchanged.",
        s.roots,
        s.elapsed.as_secs_f64(),
        s.found,
        s.read,
        s.unchanged
    )?;
    writeln!(
        out,
        "Tracks: {} added, {} updated, {} moved, {} returned.",
        s.added, s.updated, s.moved, s.returned
    )?;
    writeln!(
        out,
        "Files: {} gone, {} back, {} could not be read, {} skipped as unreadable before.",
        s.missing, s.back, s.failed, s.skipped_unreadable
    )?;
    if s.unavailable_roots > 0 {
        writeln!(out, "{} folders could not be read.", s.unavailable_roots)?;
    }
    if s.cancelled {
        writeln!(out, "The scan was cancelled.")?;
    }
    Ok(())
}
