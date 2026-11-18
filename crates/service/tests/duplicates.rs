// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Copies through the service: the `[library.duplicates]` settings applied to the
//! library as they change, the group of a track, the main copy, and the duplicate
//! finder, each announced on the bus.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use helikodu_model::settings::DuplicateScan;
use helikodu_service::bus::Subscription;
use helikodu_service::files::AppDirs;
use helikodu_service::library::{
    KeepRule, LibraryEvent, MatchBy, MusicLibrary, ScanRequest, TrackFilter,
};
use helikodu_service::{Event, LibraryChoice, Service, ServiceConfig, SubscribeOptions};

fn folder(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("service-duplicates")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn flac(path: &Path, title: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg("sine=frequency=440:duration=1:sample_rate=44100")
        .args(["-metadata", &format!("title={title}"), "-c:a", "flac"])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success());
}

fn until(events: &Subscription<Event>, mut done: impl FnMut(&Event) -> bool) -> Event {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = events.recv_timeout(left).expect("timed out");
        if done(&event) {
            return event;
        }
    }
}

fn tracks(library: &MusicLibrary) -> usize {
    library.all_tracks(TrackFilter::All).unwrap().len()
}

#[test]
fn settings_changes_reach_the_library_at_once() {
    if !Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("SKIPPED: ffmpeg is not installed");
        return;
    }
    let dir = folder("live");
    let a = dir.join("music/a");
    let b = dir.join("music/b");
    flac(&a.join("x.flac"), "X");
    fs::create_dir_all(&b).unwrap();
    fs::copy(a.join("x.flac"), b.join("x.flac")).unwrap();
    let worker = worker_program();
    let music = MusicLibrary::in_memory().unwrap();
    music
        .scan(ScanRequest {
            folders: vec![a.clone(), b.clone()],
            worker_program: Some(worker.clone()),
            ..ScanRequest::default()
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(tracks(&music), 2);

    let dirs = AppDirs::under(&dir);
    fs::create_dir_all(&dirs.config).unwrap();
    fs::write(
        dirs.config.join("settings.toml"),
        format!(
            "version = 1\n[library]\nfolders = [\"{}\", \"{}\"]\n\
             [library.duplicates]\non_scan = \"skip\"\n[output]\nbackend = \"null\"\n",
            a.display(),
            b.display()
        ),
    )
    .unwrap();
    let mut config = ServiceConfig::new(dirs);
    config.library = LibraryChoice::Open(music.clone());
    config.thumbnails = false;
    config.worker_program = Some(worker);
    let service = Service::start(config).unwrap();
    let events = service.subscribe(SubscribeOptions {
        player: false,
        ..SubscribeOptions::all()
    });
    // The settings file says to skip copies: applied at start.
    let deadline = Instant::now() + Duration::from_secs(60);
    while tracks(&music) != 1 {
        assert!(Instant::now() < deadline, "the copy kept its track");
        std::thread::sleep(Duration::from_millis(20));
    }

    // Back to adding copies: they are read again at once.
    service
        .settings()
        .update(|s| s.library.duplicates.on_scan = DuplicateScan::Add)
        .unwrap();
    let applied = until(&events, |e| {
        matches!(e, Event::Library(LibraryEvent::DuplicatesApplied(_)))
    });
    let Event::Library(LibraryEvent::DuplicatesApplied(Ok(summary))) = applied else {
        panic!("{applied:?}");
    };
    assert_eq!(summary.read, 1, "{summary:?}");
    until(&events, |e| {
        matches!(e, Event::Library(LibraryEvent::Changed))
    });
    assert_eq!(tracks(&music), 2);

    // Showing one copy is a query setting, also at once.
    service
        .settings()
        .update(|s| s.library.duplicates.show_one = true)
        .unwrap();
    until(&events, |e| {
        matches!(e, Event::Library(LibraryEvent::DuplicatesApplied(_)))
    });
    assert_eq!(tracks(&music), 1);

    // The group, the main copy and the finder through the library service.
    let library = service.library().unwrap();
    let main_track = music.track_at(&a.join("x.flac")).unwrap().unwrap();
    let (reply, answer) = mpsc::channel();
    library
        .copies_of(main_track, move |r| reply.send(r).unwrap())
        .unwrap();
    let group = answer.recv().unwrap().unwrap().unwrap();
    assert_eq!(group.files.len(), 2);
    assert_eq!(group.files[0].path, a.join("x.flac"));
    let copy = group.files[1].file;
    let (reply, answer) = mpsc::channel();
    library
        .set_main_copy(copy, move |r| reply.send(r).unwrap())
        .unwrap();
    answer.recv().unwrap().unwrap();
    until(&events, |e| {
        matches!(e, Event::Library(LibraryEvent::Changed))
    });
    assert_eq!(music.copies_of(main_track).unwrap().unwrap().main, copy);
    assert_eq!(
        music
            .track_source(main_track)
            .unwrap()
            .unwrap()
            .source
            .path(),
        b.join("x.flac").as_path()
    );
    let (reply, answer) = mpsc::channel();
    library
        .find_duplicates(MatchBy::IdenticalFiles, KeepRule::Oldest, move |r| {
            reply.send(r).unwrap();
        })
        .unwrap();
    let groups = answer.recv().unwrap().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].entries.len(), 2);
    let (reply, answer) = mpsc::channel();
    library
        .duplicate_counts(move |r| reply.send(r).unwrap())
        .unwrap();
    assert_eq!(answer.recv().unwrap().unwrap().identical_files, 1);
}

/// The `helikodu-scan-worker` program of this build, built on first use.
fn worker_program() -> PathBuf {
    let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let release = !cfg!(debug_assertions);
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .args([
            "build",
            "--quiet",
            "-p",
            "helikodu-scan-worker",
            "--target-dir",
        ])
        .arg(&target)
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    if release {
        command.arg("--release");
    }
    assert!(
        command.status().unwrap().success(),
        "building the scan worker"
    );
    target
        .join(if release { "release" } else { "debug" })
        .join("helikodu-scan-worker")
}
