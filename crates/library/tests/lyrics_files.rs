// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `has:lyrics` finds lyrics in the file's tags and lyrics files beside it, also
//! files added or removed after the first scan, by a rescan or by the watcher.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[allow(dead_code)]
#[path = "support/scanning.rs"]
mod support;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use helikodu_library::{
    CancelToken, DatabaseConfig, Library, QueryRequest, RescanReason, ScanOptions, WatchEvent,
    WatchOptions, Watcher, parse_query,
};
use support::{flac, folder, have_ffmpeg, worker_program, write};

fn options(root: &Path) -> ScanOptions {
    let mut options = ScanOptions::new([root]);
    options.workers.program = Some(worker_program());
    options.workers.count = 2;
    options
}

fn library() -> Arc<Library> {
    Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap())
}

fn scan(library: &Library, options: &ScanOptions) {
    library.scan(options, None, &CancelToken::new()).unwrap();
}

/// The titles `text` finds, in order.
fn found(library: &Library, text: &str) -> Vec<String> {
    let request = QueryRequest::new(parse_query(&format!("{text} sort:title")).unwrap());
    let mut titles: Vec<String> = library
        .query_tracks(&request)
        .unwrap()
        .items
        .into_iter()
        .filter_map(|t| t.title)
        .collect();
    titles.sort();
    titles
}

fn details_say_lyrics(library: &Library, path: &Path) -> bool {
    let track = library.track_at(path, 0).unwrap().unwrap();
    library.track_details(track).unwrap().unwrap().has_lyrics
}

#[test]
fn lyrics_in_tags_and_beside_the_track_are_found_after_scans() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("lyrics-files-scan");
    let album = root.join("Album");
    flac(
        album.join("1 Tagged.flac"),
        440,
        1,
        &[("title", "Tagged"), ("lyrics", "Words")],
    );
    flac(album.join("2 Synced.flac"), 440, 1, &[("title", "Synced")]);
    write(album.join("2 Synced.lrc"), "[00:01.00]Words\n");
    flac(album.join("3 Plain.flac"), 440, 1, &[("title", "Plain")]);
    write(album.join("3 Plain.TXT"), "Words\n");
    flac(album.join("4 None.flac"), 440, 1, &[("title", "None")]);
    let library = library();
    let options = options(&root);
    scan(&library, &options);

    assert_eq!(found(&library, "has:lyrics"), ["Plain", "Synced", "Tagged"]);
    assert_eq!(found(&library, "-has:lyrics"), ["None"]);
    assert!(details_say_lyrics(&library, &album.join("2 Synced.flac")));
    assert!(!details_say_lyrics(&library, &album.join("4 None.flac")));

    // Added beside an unchanged track: found by the next scan, without reading it.
    let late = album.join("4 None.lrc");
    write(&late, "[00:01.00]Late\n");
    let summary = library.scan(&options, None, &CancelToken::new()).unwrap();
    assert_eq!(summary.read, 0, "{summary:?}");
    assert_eq!(
        found(&library, "has:lyrics"),
        ["None", "Plain", "Synced", "Tagged"]
    );

    // Removed again: no longer found.
    fs::remove_file(&late).unwrap();
    fs::remove_file(album.join("3 Plain.TXT")).unwrap();
    scan(&library, &options);
    assert_eq!(found(&library, "has:lyrics"), ["Synced", "Tagged"]);
}

/// The events up to the first that `wanted` accepts.
fn wait_for(watcher: &Watcher, wanted: impl Fn(&WatchEvent) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = watcher.events().recv_timeout(left).unwrap();
        if wanted(&event) {
            return;
        }
    }
}

fn until(what: &str, done: impl Fn() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{what}: not seen after 10 s"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_watcher_sees_lyrics_files_come_and_go() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("lyrics-files-watch");
    let track = root.join("Album/Song.flac");
    flac(&track, 440, 1, &[("title", "Song")]);
    let library = library();
    let mut watch = WatchOptions::new(options(&root));
    watch.check_interval = Duration::from_millis(200);
    let watcher = Watcher::spawn(Arc::clone(&library), watch).unwrap();
    wait_for(&watcher, |e| {
        matches!(
            e,
            WatchEvent::RescanFinished {
                reason: RescanReason::Started,
                ..
            }
        )
    });
    assert!(found(&library, "has:lyrics").is_empty());

    let lyrics = root.join("Album/Song.lrc");
    write(&lyrics, "[00:01.00]Words\n");
    until("lyrics file added", || {
        found(&library, "has:lyrics") == ["Song"]
    });

    let renamed = root.join("Album/Song.txt");
    fs::rename(&lyrics, &renamed).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(found(&library, "has:lyrics"), ["Song"]);

    fs::remove_file(&renamed).unwrap();
    until("lyrics file removed", || {
        found(&library, "has:lyrics").is_empty()
    });
    watcher.stop();
}
