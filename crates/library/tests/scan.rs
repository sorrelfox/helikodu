// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The scanner with real worker processes over a small library made with ffmpeg:
//! names that are not UTF-8, a CUE album, exclusions, a link loop, rescans after
//! changes, and files that crash or hang the worker.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[path = "support/scanning.rs"]
mod support;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use helikodu_library::{
    CancelToken, DatabaseConfig, ExcludeRule, FailureKind, Library, ScanError, ScanEvent,
    ScanHandle, ScanOptions, ScanSummary, SearchRequest, TrackKind,
};
use support::{byte_path, flac, folder, have_ffmpeg, worker_program, write};

fn options(root: &Path) -> ScanOptions {
    let mut options = ScanOptions::new([root]);
    options.workers.program = Some(worker_program());
    options.workers.count = 2;
    options
}

fn scan(library: &Library, options: &ScanOptions) -> (ScanSummary, Vec<ScanEvent>) {
    let (sender, receiver) = mpsc::channel();
    let summary = library
        .scan(options, Some(&sender), &CancelToken::new())
        .unwrap();
    drop(sender);
    (summary, receiver.into_iter().collect())
}

fn title_of(library: &Library, path: &Path, sub_index: u32) -> Option<String> {
    let track = library.track_at(path, sub_index).unwrap()?;
    let id = i64::try_from(track.get()).unwrap();
    library
        .database()
        .read(|c| c.query_row("SELECT title FROM tracks WHERE id = ?1", [id], |r| r.get(0)))
        .unwrap()
}

fn kind_of(library: &Library, path: &Path) -> i64 {
    let track = library.track_at(path, 0).unwrap().unwrap();
    let id = i64::try_from(track.get()).unwrap();
    library
        .database()
        .read(|c| c.query_row("SELECT kind FROM tracks WHERE id = ?1", [id], |r| r.get(0)))
        .unwrap()
}

const CUE_SHEET: &str = "PERFORMER \"Cue Artist\"\r\nTITLE \"Cue Album\"\r\n\
FILE \"album.flac\" WAVE\r\n\
  TRACK 01 AUDIO\r\n    TITLE \"First\"\r\n    INDEX 01 00:00:00\r\n\
  TRACK 02 AUDIO\r\n    TITLE \"Second\"\r\n    INDEX 01 00:03:00\r\n\
  TRACK 03 AUDIO\r\n    TITLE \"Third\"\r\n    INDEX 01 00:06:00\r\n";

#[test]
fn a_small_library_is_scanned_and_found() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("small");
    let one = root.join("Artist/Album/01 One.flac");
    flac(
        &one,
        440,
        1,
        &[("title", "One"), ("artist", "Artist"), ("album", "Album")],
    );
    // Latin-1 bytes in both the folder and the file name.
    let bjork = byte_path(&byte_path(&root, b"Bj\xf6rk"), b"J\xf3ga \xff.flac");
    flac(&bjork, 550, 1, &[("title", "Jóga"), ("artist", "Björk")]);
    let album = root.join("Cue/album.flac");
    flac(&album, 660, 10, &[("album", "Cue Album")]);
    write(root.join("Cue/album.cue"), CUE_SHEET);
    flac(
        root.join("Excluded/x.flac"),
        300,
        1,
        &[("title", "Excluded")],
    );
    flac(root.join(".hidden/h.flac"), 300, 1, &[("title", "Hidden")]);
    write(root.join("clip.mkv"), b"not really a video");
    write(root.join("Artist/Album/cover.jpg"), b"\xff\xd8\xff");
    symlink(&root, root.join("Artist/loop")).unwrap();

    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let mut options = options(&root);
    options.exclude = vec![ExcludeRule::Path(root.join("Excluded"))];
    let (summary, events) = scan(&library, &options);

    assert_eq!(summary.found, 4, "{summary:?}"); // 3 FLAC files and the sheet
    assert_eq!(summary.read, 3, "{summary:?}");
    assert_eq!(summary.added, 5, "{summary:?}"); // 1 + 1 + 3 sheet tracks
    assert_eq!(summary.failed, 0, "{events:?}");
    assert!(!summary.cancelled);
    assert!(matches!(events.last(), Some(ScanEvent::Finished(s)) if *s == summary));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ScanEvent::Progress { done: 3, total: 3 }))
    );

    assert_eq!(title_of(&library, &one, 0).as_deref(), Some("One"));
    assert_eq!(title_of(&library, &bjork, 0).as_deref(), Some("Jóga"));
    assert_eq!(
        title_of(&library, &album, 0),
        None,
        "the sheet replaces the file"
    );
    assert_eq!(title_of(&library, &album, 1).as_deref(), Some("First"));
    assert_eq!(title_of(&library, &album, 3).as_deref(), Some("Third"));
    assert_eq!(title_of(&library, &root.join("Excluded/x.flac"), 0), None);
    assert_eq!(title_of(&library, &root.join(".hidden/h.flac"), 0), None);
    let found = library.search(&SearchRequest::new("joga")).unwrap();
    assert_eq!(found.total, 1);

    // Nothing changed: nothing is read again.
    let (again, _) = scan(&library, &options);
    assert_eq!(again.read, 0, "{again:?}");
    assert_eq!(again.unchanged, 4, "{again:?}");
    assert_eq!(again.missing, 0);

    // Video files only when asked for; they are audio from video.
    let video = root.join("film.mkv");
    flac(&video, 440, 1, &[("title", "Film")]);
    options.include_video = true;
    let (with_video, _) = scan(&library, &options);
    assert_eq!(with_video.read, 2, "{with_video:?}"); // film.mkv and the fake clip.mkv
    assert_eq!(with_video.failed, 1, "{with_video:?}");
    assert_eq!(
        kind_of(&library, &video),
        3,
        "{:?}",
        TrackKind::AudioFromVideo
    );
}

#[test]
fn rescans_see_additions_changes_removals_and_moves() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("rescan");
    flac(root.join("a.flac"), 440, 1, &[("title", "A")]);
    flac(root.join("b.flac"), 550, 1, &[("title", "B")]);
    flac(root.join("c.flac"), 660, 1, &[("title", "C")]);
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let options = options(&root);
    let (first, _) = scan(&library, &options);
    assert_eq!(first.added, 3, "{first:?}");
    let a_before = library.track_at(&root.join("a.flac"), 0).unwrap().unwrap();
    let a_identity = library.identity_of(a_before).unwrap();

    // Changed tags; a removal; a move into a new folder; an addition.
    std::thread::sleep(Duration::from_millis(20));
    flac(root.join("b.flac"), 550, 1, &[("title", "B2")]);
    fs::remove_file(root.join("c.flac")).unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::rename(root.join("a.flac"), root.join("sub/a2.flac")).unwrap();
    flac(root.join("d.flac"), 770, 10, &[("title", "D")]);

    let (second, _) = scan(&library, &options);
    assert_eq!(second.read, 3, "{second:?}"); // b, d, sub/a2
    assert_eq!(second.updated, 1, "{second:?}");
    assert_eq!(second.moved, 1, "{second:?}");
    assert_eq!(second.added, 1, "{second:?}");
    assert_eq!(second.missing, 2, "{second:?}"); // a (until it is found) and c
    assert_eq!(
        title_of(&library, &root.join("b.flac"), 0).as_deref(),
        Some("B2")
    );
    let a_after = library
        .track_at(&root.join("sub/a2.flac"), 0)
        .unwrap()
        .unwrap();
    assert_eq!(a_after, a_before, "the moved file keeps its track");
    assert_eq!(library.identity_of(a_after).unwrap(), a_identity);
    assert_eq!(library.track_at(&root.join("a.flac"), 0).unwrap(), None);
    // The removed file is missing, not gone: its track waits for the grace period.
    assert!(library.track_at(&root.join("c.flac"), 0).unwrap().is_some());

    // It comes back in place.
    flac(root.join("c.flac"), 660, 1, &[("title", "C")]);
    let (third, _) = scan(&library, &options);
    assert_eq!(third.read, 1, "{third:?}");
    assert_eq!(third.updated, 1, "{third:?}");

    // A CUE sheet that appears turns the file into its tracks.
    write(
        root.join("d.cue"),
        CUE_SHEET.replace("album.flac", "d.flac"),
    );
    let (fourth, _) = scan(&library, &options);
    assert_eq!(fourth.added, 3, "{fourth:?}");
    assert_eq!(title_of(&library, &root.join("d.flac"), 0), None);
    assert_eq!(
        title_of(&library, &root.join("d.flac"), 2).as_deref(),
        Some("Second")
    );
}

#[test]
fn a_missing_root_marks_nothing_missing() {
    if !have_ffmpeg() {
        return;
    }
    let base = folder("unmounted");
    let root = base.join("music");
    flac(root.join("a.flac"), 440, 1, &[("title", "A")]);
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let options = options(&root);
    scan(&library, &options);
    fs::rename(&root, base.join("elsewhere")).unwrap();
    let (summary, events) = scan(&library, &options);
    assert_eq!(summary.unavailable_roots, 1);
    assert_eq!(summary.missing, 0);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ScanEvent::RootUnavailable { .. }))
    );
}

#[test]
fn a_worker_that_cannot_be_found_is_an_error() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("no-worker");
    flac(root.join("a.flac"), 440, 1, &[]);
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let mut options = options(&root);
    options.workers.program = Some(root.join("no-such-program"));
    let error = library
        .scan(&options, None, &CancelToken::new())
        .unwrap_err();
    assert!(matches!(error, ScanError::WorkerNotFound(_)), "{error}");
}

/// The fault markers the worker honours in debug builds.
#[cfg(debug_assertions)]
mod faults {
    use super::*;

    fn faulty(root: &Path, timeout: Duration) -> ScanOptions {
        let mut options = options(root);
        options.workers.env = vec![("HELIKODU_SCAN_WORKER_FAULTS".into(), "1".into())];
        options.workers.file_timeout = timeout;
        options
    }

    #[test]
    fn crashing_and_hanging_files_do_not_stop_the_scan() {
        if !have_ffmpeg() {
            return;
        }
        let root = folder("poison");
        for name in [
            "a.flac",
            "crash-worker.flac",
            "m.flac",
            "hang-worker.flac",
            "z.flac",
        ] {
            flac(root.join(name), 440, 1, &[("title", name)]);
        }
        write(root.join("hog-worker.flac"), b"fLaC");
        let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
        let options = faulty(&root, Duration::from_secs(2));
        let (summary, events) = scan(&library, &options);
        assert_eq!(summary.read, 6, "{summary:?}");
        assert_eq!(summary.added, 3, "{summary:?}");
        assert_eq!(summary.failed, 3, "{summary:?}");
        assert!(summary.worker_restarts >= 3, "{summary:?}");

        let failures: Vec<(PathBuf, FailureKind)> = events
            .iter()
            .filter_map(|e| match e {
                ScanEvent::FileFailed { path, failure } => Some((path.clone(), failure.kind)),
                _ => None,
            })
            .collect();
        assert!(failures.contains(&(root.join("crash-worker.flac"), FailureKind::WorkerCrashed)));
        assert!(failures.contains(&(root.join("hog-worker.flac"), FailureKind::WorkerCrashed)));
        assert!(failures.contains(&(root.join("hang-worker.flac"), FailureKind::TimedOut)));

        let unreadable = library.unreadable_files().unwrap();
        let paths: Vec<&PathBuf> = unreadable.iter().map(|(p, _)| p).collect();
        assert_eq!(
            paths,
            [
                &root.join("crash-worker.flac"),
                &root.join("hang-worker.flac"),
                &root.join("hog-worker.flac")
            ]
        );
        assert!(unreadable[1].1.contains("longer than"), "{unreadable:?}");

        // They are not tried again until they change.
        let (again, _) = scan(&library, &options);
        assert_eq!(again.read, 0, "{again:?}");
        assert_eq!(again.skipped_unreadable, 3, "{again:?}");
        let mut retry = options.clone();
        retry.retry_unreadable = true;
        retry.workers.env.clear();
        let (retried, _) = scan(&library, &retry);
        assert_eq!(retried.read, 3, "{retried:?}");
        assert_eq!(retried.added, 2, "{retried:?}"); // the hog file is no FLAC at all
        assert_eq!(library.unreadable_files().unwrap().len(), 1);
    }

    #[test]
    fn a_scan_can_be_cancelled() {
        if !have_ffmpeg() {
            return;
        }
        let root = folder("cancel");
        for n in 0..8 {
            write(root.join(format!("{n}-hang-worker.flac")), b"fLaC");
        }
        let library = Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap());
        let options = faulty(&root, Duration::from_secs(60));
        let scan = ScanHandle::spawn(Arc::clone(&library), options).unwrap();
        for event in scan.events() {
            if matches!(event, ScanEvent::Compared { .. }) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
        let started = std::time::Instant::now();
        scan.cancel();
        let summary = scan.join().unwrap();
        assert!(summary.cancelled);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(summary.read, 0);
    }
}
