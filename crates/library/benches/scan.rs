// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The throughput of a full scan of a synthetic library.
//!
//! ```sh
//! cargo bench -p helikodu-library --bench scan [-- <tracks> [--workers N] [--keep] [--reuse]]
//! ```
//!
//! Generates a library of one-second FLAC files with `cargo xtask synth-library`
//! (100 000 tracks by default) under `target/scan-bench/`, builds the worker, scans
//! the library into a new database file and prints the time and the files per
//! second; then scans it again unchanged. Then it measures the browsing queries
//! (album lists, album details, artists, genres) on the scanned library. Afterwards
//! it deletes the library and the database, unless `--keep` is given. `--reuse`
//! measures the queries on a database kept by an earlier run, without scanning.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use helikodu_library::{
    AlbumFilter, AlbumQuery, AlbumSort, ArtistQuery, ArtistRole, CancelToken, DatabaseConfig,
    GenreQuery, Library, Page, ScanEvent, ScanOptions, ScanSummary, TrackFilter, TrackQuery,
};

struct Args {
    tracks: usize,
    workers: usize,
    keep: bool,
    reuse: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        tracks: 100_000,
        workers: 0,
        keep: false,
        reuse: false,
    };
    let mut rest = std::env::args().skip(1);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--bench" => {}
            "--keep" => args.keep = true,
            "--reuse" => {
                args.reuse = true;
                args.keep = true;
            }
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

    if args.reuse {
        let library = Library::open(&DatabaseConfig::file(&database))
            .unwrap_or_else(|e| fail(&format!("cannot open the database: {e}")));
        browse(&library);
        return;
    }

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
    browse(&library);
    drop(library);

    if args.keep {
        eprintln!("kept {} and {}", music.display(), database.display());
    } else {
        let _ = fs::remove_dir_all(&base);
    }
}

/// The median and the slowest of `runs` runs of `query`, in milliseconds.
fn time<T>(runs: usize, mut query: impl FnMut(usize) -> T) -> (f64, f64) {
    let mut times: Vec<f64> = (0..runs)
        .map(|run| {
            let started = Instant::now();
            std::hint::black_box(query(run));
            started.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(f64::total_cmp);
    (times[times.len() / 2], times[times.len() - 1])
}

fn report(what: &str, (median, max): (f64, f64)) {
    println!("{what:<48} median {median:7.2} ms   max {max:7.2} ms");
}

/// Measures the browsing queries.
fn browse(library: &Library) {
    let page = |sort, offset, filter| AlbumQuery {
        filter,
        sort,
        page: Page::new(offset, 50),
        include_masked: false,
    };
    let all = library
        .albums(&page(AlbumSort::Title, 0, AlbumFilter::All))
        .unwrap_or_else(|e| fail(&e.to_string()));
    let artists = library
        .artists(&ArtistQuery {
            page: Page::first(1000),
            ..ArtistQuery::new(ArtistRole::AlbumArtist)
        })
        .unwrap_or_else(|e| fail(&e.to_string()));
    let compilations = library
        .albums(&page(AlbumSort::Title, 0, AlbumFilter::Compilations))
        .unwrap_or_else(|e| fail(&e.to_string()));
    println!(
        "browsing: {} albums, {} compilations, {} album artists",
        all.total, compilations.total, artists.total
    );
    let middle = all.total / 2;
    for (name, sort, offset) in [
        ("albums by title, first page", AlbumSort::Title, 0),
        ("albums by title, middle page", AlbumSort::Title, middle),
        ("albums by artist, first page", AlbumSort::Artist, 0),
        ("albums by artist, middle page", AlbumSort::Artist, middle),
        ("albums by year, middle page", AlbumSort::Year, middle),
        ("albums newest first, first page", AlbumSort::Added, 0),
    ] {
        report(
            name,
            time(15, |_| {
                library.albums(&page(sort, offset, AlbumFilter::All))
            }),
        );
    }
    report(
        "compilations, first page",
        time(15, |_| {
            library.albums(&page(AlbumSort::Title, 0, AlbumFilter::Compilations))
        }),
    );
    let people: Vec<_> = artists.items.iter().map(|a| a.id).collect();
    if !people.is_empty() {
        report(
            "albums of an album artist",
            time(50, |run| {
                let person = people[run * 7919 % people.len()];
                library.albums(&page(AlbumSort::Year, 0, AlbumFilter::ByArtist(person)))
            }),
        );
        report(
            "albums an artist appears on",
            time(50, |run| {
                let person = people[run * 7919 % people.len()];
                library.albums(&page(AlbumSort::Year, 0, AlbumFilter::AppearsOn(person)))
            }),
        );
    }
    // Album details of albums spread over the list.
    let ids: Vec<_> = (0..20)
        .flat_map(|i| {
            library
                .albums(&page(
                    AlbumSort::Title,
                    all.total * i / 20,
                    AlbumFilter::All,
                ))
                .map(|p| p.items.into_iter().map(|a| a.id).collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect();
    if !ids.is_empty() {
        report(
            "album detail with discs and tracks",
            time(200, |run| library.album(ids[run * 7919 % ids.len()])),
        );
    }
    for (name, role) in [
        ("artists, first page", ArtistRole::Artist),
        ("album artists, middle page", ArtistRole::AlbumArtist),
        ("composers, first page", ArtistRole::Composer),
    ] {
        let offset = if role == ArtistRole::AlbumArtist {
            artists.total / 2
        } else {
            0
        };
        report(
            name,
            time(15, |_| {
                library.artists(&ArtistQuery {
                    page: Page::new(offset, 50),
                    ..ArtistQuery::new(role)
                })
            }),
        );
    }
    report(
        "genres",
        time(15, |_| library.genres(&GenreQuery::default())),
    );
    let genres = library
        .genres(&GenreQuery::default())
        .unwrap_or_else(|e| fail(&e.to_string()));
    if let Some(genre) = genres.items.first() {
        report(
            "tracks of a genre, first page",
            time(15, |_| {
                library.tracks(&TrackQuery {
                    filter: TrackFilter::Genre(genre.id),
                    ..TrackQuery::default()
                })
            }),
        );
    }
    if !people.is_empty() {
        report(
            "tracks of an album artist",
            time(50, |run| {
                let person = people[run * 7919 % people.len()];
                library.tracks(&TrackQuery {
                    filter: TrackFilter::Artist(person, Some(ArtistRole::AlbumArtist)),
                    ..TrackQuery::default()
                })
            }),
        );
    }

    // With a masked library folder, every query checks each track's folder.
    let gone = library
        .add_root("/nonexistent/helikodu-bench")
        .unwrap_or_else(|e| fail(&e.to_string()));
    let _ = library.set_root_masked(gone, true);
    report(
        "masking on: albums by title, first page",
        time(15, |_| {
            library.albums(&page(AlbumSort::Title, 0, AlbumFilter::All))
        }),
    );
    report(
        "masking on: albums by title, middle page",
        time(15, |_| {
            library.albums(&page(AlbumSort::Title, middle, AlbumFilter::All))
        }),
    );
    if !ids.is_empty() {
        report(
            "masking on: album detail",
            time(200, |run| library.album(ids[run * 7919 % ids.len()])),
        );
    }
    report(
        "masking on: album artists, first page",
        time(15, |_| {
            library.artists(&ArtistQuery::new(ArtistRole::AlbumArtist))
        }),
    );
    let _ = library.set_root_masked(gone, false);
}
