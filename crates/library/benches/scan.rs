// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The throughput of a full scan of a synthetic library.
//!
//! ```sh
//! cargo bench -p helikodu-library --bench scan [-- <tracks> [--workers N] [--keep]]
//! ```
//!
//! Generates a library of one-second FLAC files with `cargo xtask synth-library`
//! (100 000 tracks by default) under `target/scan-bench/`, builds the worker, scans
//! the library into a new database file and prints the time and the files per
//! second; then scans it again unchanged. Afterwards it deletes the library and the
//! database, unless `--keep` is given.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use helikodu_library::{CancelToken, DatabaseConfig, Library, ScanEvent, ScanOptions, ScanSummary};

struct Args {
    tracks: usize,
    workers: usize,
    keep: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        tracks: 100_000,
        workers: 0,
        keep: false,
    };
    let mut rest = std::env::args().skip(1);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--bench" => {}
            "--keep" => args.keep = true,
            "--workers" => {
                args.workers = rest
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or_else(|| fail("--workers needs a number"));
            }
            count => args.tracks = parse_count(count),
        }
    }
    args
}

fn parse_count(text: &str) -> usize {
    let (digits, factor) = match text.strip_suffix('k') {
        Some(d) => (d, 1000),
        None => (text, 1),
    };
    digits
        .parse::<usize>()
        .map(|n| n * factor)
        .unwrap_or_else(|_| fail(&format!("not a number of tracks: {text}")))
}

fn fail(message: &str) -> ! {
    eprintln!("scan bench: {message}");
    std::process::exit(2);
}

fn run(command: &mut Command, what: &str) {
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => fail(&format!("{what} failed: {status}")),
        Err(e) => fail(&format!("{what} could not run: {e}")),
    }
}

fn scan(library: &Library, options: &ScanOptions) -> ScanSummary {
    let (sender, events) = mpsc::channel();
    let printer = thread::spawn(move || {
        let mut last = Instant::now();
        for event in events {
            match event {
                ScanEvent::Compared { found, to_read, .. } => {
                    eprintln!("  walked: {found} files, {to_read} to read");
                }
                ScanEvent::Progress { done, total } if last.elapsed() > Duration::from_secs(5) => {
                    last = Instant::now();
                    eprintln!("  {done} / {total}");
                }
                ScanEvent::FileFailed { path, failure } => {
                    eprintln!("  failed: {}: {failure}", path.display());
                }
                _ => {}
            }
        }
    });
    let summary = library
        .scan(options, Some(&sender), &CancelToken::new())
        .unwrap_or_else(|e| fail(&format!("the scan failed: {e}")));
    drop(sender);
    let _ = printer.join();
    summary
}

fn main() {
    let args = parse_args();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .map_or_else(|| workspace.join("target"), Path::to_path_buf);
    let base = target.join("scan-bench");
    let music = base.join(format!("flac-{}", args.tracks));
    let database = base.join("library.db");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());

    eprintln!("generating {} tracks in {}", args.tracks, music.display());
    let started = Instant::now();
    run(
        Command::new(&cargo)
            .args(["xtask", "synth-library", "--tracks"])
            .arg(args.tracks.to_string())
            .arg("--out")
            .arg(&music)
            .current_dir(&workspace),
        "cargo xtask synth-library",
    );
    eprintln!("generated in {:.1} s", started.elapsed().as_secs_f64());
    run(
        Command::new(&cargo)
            .args([
                "build",
                "--quiet",
                "--release",
                "-p",
                "helikodu-scan-worker",
            ])
            .arg("--target-dir")
            .arg(&target)
            .current_dir(&workspace),
        "building the scan worker",
    );
    let program: PathBuf = target.join("release/helikodu-scan-worker");

    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", database.display()));
    }
    let library = Library::open(&DatabaseConfig::file(&database))
        .unwrap_or_else(|e| fail(&format!("cannot open the database: {e}")));
    let mut options = ScanOptions::new([&music]);
    options.workers.program = Some(program);
    options.workers.count = args.workers;
    let workers = options.workers.count;

    eprintln!("first scan");
    let first = scan(&library, &options);
    let seconds = first.elapsed.as_secs_f64();
    println!(
        "first scan: {} files, {} tracks, {} failed, workers {}: {:.1} s, {:.0} files/s",
        first.read,
        first.added,
        first.failed,
        if workers == 0 {
            "auto".to_owned()
        } else {
            workers.to_string()
        },
        seconds,
        first.read as f64 / seconds
    );
    let second = scan(&library, &options);
    println!(
        "unchanged rescan: {} files, {} read: {:.2} s",
        second.found,
        second.read,
        second.elapsed.as_secs_f64()
    );
    drop(library);

    if args.keep {
        eprintln!("kept {} and {}", music.display(), database.display());
    } else {
        let _ = fs::remove_dir_all(&base);
    }
}
