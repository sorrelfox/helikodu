// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The watcher over real folders, with real worker processes: changes appear within
//! two seconds, bursts coalesce, moves keep identities, polled and masked library
//! folders, and the grace period for missing files.

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
    DatabaseConfig, Library, PollReason, RescanReason, ScanOptions, TrackQuery, WatchEvent,
    WatchMode, WatchOptions, Watcher, path_to_blob,
};
use support::{flac, folder, have_ffmpeg, worker_program};

/// The time a watched change may take to appear.
const TARGET: Duration = Duration::from_secs(2);

fn options(root: &Path) -> WatchOptions {
    let mut scan = ScanOptions::new([root]);
    scan.workers.program = Some(worker_program());
    scan.workers.count = 2;
    let mut options = WatchOptions::new(scan);
    options.check_interval = Duration::from_millis(200);
    options
}

fn library() -> Arc<Library> {
    Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap())
}

/// Starts watching and waits for the first scan to end.
fn start(library: &Arc<Library>, options: WatchOptions) -> Watcher {
    let watcher = Watcher::spawn(Arc::clone(library), options).unwrap();
    wait_for(&watcher, |e| {
        matches!(
            e,
            WatchEvent::RescanFinished {
                reason: RescanReason::Started,
                ..
            }
        )
    });
    watcher
}

/// The events up to the first that `wanted` accepts, which is the last.
fn wait_for(watcher: &Watcher, wanted: impl Fn(&WatchEvent) -> bool) -> Vec<WatchEvent> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match watcher.events().recv_timeout(left) {
            Ok(event) => {
                let done = wanted(&event);
                seen.push(event);
                if done {
                    return seen;
                }
            }
            Err(e) => panic!("{e}; events so far: {seen:#?}"),
        }
    }
}

/// The events that have come so far.
fn drain(watcher: &Watcher) -> Vec<WatchEvent> {
    watcher.events().try_iter().collect()
}

/// How long it took until `done` held, checked every 5 ms; panics after 10 s.
fn time_until(start: Instant, what: &str, done: impl Fn() -> bool) -> Duration {
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{what}: not seen after 10 s"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let latency = start.elapsed();
    println!("{what}: {} ms", latency.as_millis());
    latency
}

fn title_of(library: &Library, path: &Path) -> Option<String> {
    let track = library.track_at(path, 0).unwrap()?;
    let id = i64::try_from(track.get()).unwrap();
    library
        .database()
        .read(|c| c.query_row("SELECT title FROM tracks WHERE id = ?1", [id], |r| r.get(0)))
        .unwrap()
}

/// The file row at `path`: whether there is one, and whether it is marked missing.
fn file_state(library: &Library, path: &Path) -> Option<bool> {
    let dir = path_to_blob(path.parent().unwrap()).to_vec();
    let name = path_to_blob(Path::new(path.file_name().unwrap())).to_vec();
    library
        .database()
        .read(move |c| {
            let mut statement = c.prepare(
                "SELECT f.missing_since IS NOT NULL FROM files f JOIN dirs d ON d.id = f.dir_id
                 WHERE d.path = ?1 AND f.name = ?2",
            )?;
            let mut rows = statement.query_map((dir, name), |r| r.get(0))?;
            rows.next().transpose()
        })
        .unwrap()
}

fn track_count(library: &Library) -> i64 {
    library
        .database()
        .read(|c| c.query_row("SELECT count(*) FROM tracks", [], |r| r.get(0)))
        .unwrap()
}

fn rescans(events: &[WatchEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, WatchEvent::RescanStarted { .. }))
        .count()
}

#[test]
fn create_modify_delete_and_move_appear_within_two_seconds() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("watch-changes");
    let staging = folder("watch-changes-staging");
    flac(root.join("a/old.flac"), 440, 1, &[("title", "Old")]);
    fs::create_dir_all(root.join("b")).unwrap();
    flac(staging.join("new.flac"), 550, 1, &[("title", "New")]);
    flac(
        staging.join("changed.flac"),
        660,
        2,
        &[("title", "Changed")],
    );
    let library = library();
    let watcher = start(&library, options(&root));
    assert!(
        library
            .track_at(&root.join("a/old.flac"), 0)
            .unwrap()
            .is_some()
    );

    // Created: copied in.
    let created = root.join("a/new.flac");
    let start = Instant::now();
    fs::copy(staging.join("new.flac"), &created).unwrap();
    let create = time_until(start, "create", || {
        title_of(&library, &created).as_deref() == Some("New")
    });

    // Modified: written over.
    let start = Instant::now();
    fs::copy(staging.join("changed.flac"), &created).unwrap();
    let modify = time_until(start, "modify", || {
        title_of(&library, &created).as_deref() == Some("Changed")
    });

    // Moved to another folder: the same track, without reading it again.
    let old = root.join("a/old.flac");
    let moved = root.join("b/moved.flac");
    let track = library.track_at(&old, 0).unwrap().unwrap();
    drain(&watcher);
    let start = Instant::now();
    fs::rename(&old, &moved).unwrap();
    let mv = time_until(start, "move", || {
        library.track_at(&moved, 0).unwrap() == Some(track)
    });
    let events = wait_for(&watcher, |e| matches!(e, WatchEvent::Moved { .. }));
    assert_eq!(
        events.last(),
        Some(&WatchEvent::Moved {
            from: old.clone(),
            to: moved.clone(),
            files: 1
        }),
        "{events:#?}"
    );
    let events = wait_for(&watcher, |e| matches!(e, WatchEvent::RescanFinished { .. }));
    let summary = events.iter().find_map(|e| match e {
        WatchEvent::RescanFinished { summary, .. } => Some(summary.clone()),
        _ => None,
    });
    assert_eq!(summary.unwrap().read, 0, "a watched move reads nothing");
    assert_eq!(file_state(&library, &old), None);

    // Deleted: marked missing, kept for the grace period.
    let start = Instant::now();
    fs::remove_file(&created).unwrap();
    let delete = time_until(start, "delete", || {
        file_state(&library, &created) == Some(true)
    });
    assert!(title_of(&library, &created).is_some());

    for (what, latency) in [
        ("create", create),
        ("modify", modify),
        ("move", mv),
        ("delete", delete),
    ] {
        assert!(latency < TARGET, "{what} took {latency:?}");
    }
    watcher.stop();
}

#[test]
fn a_burst_of_a_thousand_files_coalesces() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("watch-burst");
    let staging = folder("watch-burst-staging");
    flac(staging.join("one.flac"), 440, 1, &[("title", "One")]);
    let library = library();
    let watcher = start(&library, options(&root));
    drain(&watcher);

    let start = Instant::now();
    fs::create_dir_all(root.join("burst")).unwrap();
    for i in 0..1000 {
        fs::copy(
            staging.join("one.flac"),
            root.join(format!("burst/{i:04}.flac")),
        )
        .unwrap();
    }
    let written = start.elapsed();
    let all = time_until(start, "1000 files", || track_count(&library) == 1000);
    println!(
        "1000 files: written in {} ms, all in the library after {} ms",
        written.as_millis(),
        all.as_millis()
    );
    // Wait for the rescans to settle, then count them.
    std::thread::sleep(Duration::from_millis(1500));
    let events = drain(&watcher);
    let count = rescans(&events);
    println!("1000 files: {count} rescans");
    assert!(
        (1..=4).contains(&count),
        "{count} rescans for 1000 files: {:#?}",
        events
            .iter()
            .filter(|e| !matches!(e, WatchEvent::Scan(_)))
            .collect::<Vec<_>>()
    );
    watcher.stop();
}

#[test]
fn folder_moves_keep_identities() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("watch-folders");
    let staging = folder("watch-folders-staging");
    flac(
        root.join("Artist/Album/1.flac"),
        440,
        1,
        &[("title", "One")],
    );
    flac(
        root.join("Artist/Album/CD2/2.flac"),
        550,
        1,
        &[("title", "Two")],
    );
    flac(
        staging.join("Incoming/3.flac"),
        660,
        1,
        &[("title", "Three")],
    );
    let library = library();
    let watcher = start(&library, options(&root));
    let one = library
        .track_at(&root.join("Artist/Album/1.flac"), 0)
        .unwrap()
        .unwrap();
    let two = library
        .track_at(&root.join("Artist/Album/CD2/2.flac"), 0)
        .unwrap()
        .unwrap();

    // Renamed inside the library.
    let start = Instant::now();
    fs::rename(root.join("Artist/Album"), root.join("Artist/Renamed")).unwrap();
    time_until(start, "folder rename", || {
        library
            .track_at(&root.join("Artist/Renamed/CD2/2.flac"), 0)
            .unwrap()
            == Some(two)
    });
    assert_eq!(
        library
            .track_at(&root.join("Artist/Renamed/1.flac"), 0)
            .unwrap(),
        Some(one)
    );
    let events = wait_for(&watcher, |e| matches!(e, WatchEvent::Moved { .. }));
    assert_eq!(
        events.last(),
        Some(&WatchEvent::Moved {
            from: root.join("Artist/Album"),
            to: root.join("Artist/Renamed"),
            files: 2
        }),
        "{events:#?}"
    );
    assert_eq!(
        file_state(&library, &root.join("Artist/Album/1.flac")),
        None
    );

    // Moved in from outside: read.
    let start = Instant::now();
    fs::rename(staging.join("Incoming"), root.join("Incoming")).unwrap();
    time_until(start, "folder moved in", || {
        title_of(&library, &root.join("Incoming/3.flac")).as_deref() == Some("Three")
    });

    // Moved out: missing.
    let start = Instant::now();
    fs::rename(root.join("Artist"), staging.join("Artist")).unwrap();
    time_until(start, "folder moved out", || {
        file_state(&library, &root.join("Artist/Renamed/1.flac")) == Some(true)
            && file_state(&library, &root.join("Artist/Renamed/CD2/2.flac")) == Some(true)
    });

    // Moved back in, unseen from the other side: found again by the quick hash.
    let start = Instant::now();
    fs::rename(staging.join("Artist/Renamed"), root.join("Back")).unwrap();
    time_until(start, "folder moved back", || {
        library.track_at(&root.join("Back/1.flac"), 0).unwrap() == Some(one)
            && library.track_at(&root.join("Back/CD2/2.flac"), 0).unwrap() == Some(two)
    });
    watcher.stop();
}

/// A mount table with the root file system and `extra` lines.
fn mount_table(path: &Path, extra: &[String]) {
    let mut text = String::from("1 0 259:2 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n");
    for line in extra {
        text.push_str(line);
        text.push('\n');
    }
    fs::write(path, text).unwrap();
}

fn mount_line(point: &Path, fs_type: &str) -> String {
    let point = fs::canonicalize(point).unwrap();
    format!(
        "70 1 0:55 / {} rw,relatime shared:40 - {fs_type} server:/music rw",
        point.display().to_string().replace(' ', "\\040")
    )
}

#[test]
fn a_network_folder_is_polled() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("watch-network");
    let table = folder("watch-network-table").join("mountinfo");
    mount_table(&table, &[mount_line(&root, "nfs4")]);
    flac(root.join("first.flac"), 440, 1, &[("title", "First")]);
    let library = library();
    let mut options = options(&root);
    options.mountinfo = table;
    options.poll_interval = Duration::from_secs(1);
    let watcher = Watcher::spawn(Arc::clone(&library), options).unwrap();
    let events = wait_for(&watcher, |e| {
        matches!(
            e,
            WatchEvent::RescanFinished {
                reason: RescanReason::Started,
                ..
            }
        )
    });
    assert!(
        events.contains(&WatchEvent::Watching {
            root: root.clone(),
            mode: WatchMode::Polling {
                reason: PollReason::FileSystem {
                    fs_type: "nfs4".to_owned()
                },
                interval: Duration::from_secs(1),
            },
        }),
        "{events:#?}"
    );
    assert!(library.root_states().unwrap()[0].polled);

    let second = root.join("second.flac");
    flac(&second, 550, 1, &[("title", "Second")]);
    let start = Instant::now();
    time_until(start, "polled change", || {
        title_of(&library, &second).is_some()
    });
    let events = wait_for(&watcher, |e| matches!(e, WatchEvent::RescanFinished { .. }));
    assert!(
        events.iter().any(|e| matches!(
            e,
            WatchEvent::RescanStarted {
                reason: RescanReason::Periodic,
                full: true,
                ..
            }
        )),
        "{events:#?}"
    );
    watcher.stop();
}

#[test]
fn a_folder_that_disappears_is_masked_until_it_returns() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("watch-mask");
    let aside = folder("watch-mask-aside").join("moved");
    let song = root.join("Album/song.flac");
    flac(&song, 440, 1, &[("title", "Song")]);
    let library = library();
    let watcher = start(&library, options(&root));
    let track = library.track_at(&song, 0).unwrap().unwrap();

    // The folder goes away (as when its device is unplugged).
    fs::rename(&root, &aside).unwrap();
    let events = wait_for(&watcher, |e| matches!(e, WatchEvent::RootMasked { .. }));
    assert!(
        matches!(events.last(), Some(WatchEvent::RootMasked { root: r, .. }) if *r == root),
        "{events:#?}"
    );
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(
        file_state(&library, &song),
        Some(false),
        "not marked missing"
    );
    // The mask is in the database, and the browsing queries leave the track out.
    let state = library.root_states().unwrap().remove(0);
    assert!(state.masked && !state.polled, "{state:?}");
    let shown = |library: &Library| library.tracks(&TrackQuery::default()).unwrap().total;
    assert_eq!(shown(&library), 0);

    // It comes back: unmasked and rescanned, nothing lost.
    fs::rename(&aside, &root).unwrap();
    let events = wait_for(&watcher, |e| {
        matches!(
            e,
            WatchEvent::RescanFinished {
                reason: RescanReason::Remounted,
                ..
            }
        )
    });
    assert!(
        events.contains(&WatchEvent::RootUnmasked { root: root.clone() }),
        "{events:#?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            WatchEvent::Watching {
                mode: WatchMode::Inotify,
                ..
            }
        )),
        "{events:#?}"
    );
    assert_eq!(library.track_at(&song, 0).unwrap(), Some(track));
    assert_eq!(file_state(&library, &song), Some(false));
    assert!(!library.root_states().unwrap()[0].masked);
    assert_eq!(shown(&library), 1);

    // Watched again: a new file appears.
    let staging = folder("watch-mask-staging");
    flac(staging.join("new.flac"), 550, 1, &[("title", "New")]);
    let start = Instant::now();
    fs::copy(staging.join("new.flac"), root.join("Album/new.flac")).unwrap();
    let latency = time_until(start, "create after unmask", || {
        title_of(&library, &root.join("Album/new.flac")).is_some()
    });
    assert!(latency < TARGET);
    watcher.stop();
}

#[test]
fn an_empty_mount_point_is_masked() {
    if !have_ffmpeg() {
        return;
    }
    // The library folder is a mount point (by the mount table); unmounting leaves it
    // empty.
    let root = folder("watch-mount");
    let aside = folder("watch-mount-aside");
    let table = folder("watch-mount-table").join("mountinfo");
    let mounted = [mount_line(&root, "ext4")];
    mount_table(&table, &mounted);
    let song = root.join("song.flac");
    flac(&song, 440, 1, &[("title", "Song")]);
    let library = library();
    let mut options = options(&root);
    options.mountinfo = table.clone();
    let watcher = start(&library, options);

    mount_table(&table, &[]);
    fs::rename(&song, aside.join("song.flac")).unwrap();
    let events = wait_for(&watcher, |e| matches!(e, WatchEvent::RootMasked { .. }));
    assert!(
        matches!(events.last(), Some(WatchEvent::RootMasked { reason, .. }) if reason.contains("mounted")),
        "{events:#?}"
    );
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(
        file_state(&library, &song),
        Some(false),
        "not marked missing"
    );
    // The mask is in the database, and the browsing queries leave the track out.
    let state = library.root_states().unwrap().remove(0);
    assert!(state.masked && !state.polled, "{state:?}");
    let shown = |library: &Library| library.tracks(&TrackQuery::default()).unwrap().total;
    assert_eq!(shown(&library), 0);

    fs::rename(aside.join("song.flac"), &song).unwrap();
    mount_table(&table, &mounted);
    wait_for(&watcher, |e| {
        matches!(
            e,
            WatchEvent::RescanFinished {
                reason: RescanReason::Remounted,
                ..
            }
        )
    });
    assert_eq!(file_state(&library, &song), Some(false));
    watcher.stop();
}

#[test]
fn missing_files_are_removed_after_the_grace_period() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("watch-grace");
    let song = root.join("song.flac");
    let kept = root.join("kept.flac");
    flac(&song, 440, 1, &[("title", "Song")]);
    flac(&kept, 550, 1, &[("title", "Kept")]);
    let library = library();
    let mut options = options(&root);
    options.grace_period = Duration::from_secs(2);
    let watcher = start(&library, options);

    let start = Instant::now();
    fs::remove_file(&song).unwrap();
    time_until(start, "marked missing", || {
        file_state(&library, &song) == Some(true)
    });
    assert!(
        title_of(&library, &song).is_some(),
        "kept during the grace period"
    );
    let events = wait_for(&watcher, |e| matches!(e, WatchEvent::MissingRemoved { .. }));
    let removed_after = start.elapsed();
    println!("removed after {} ms", removed_after.as_millis());
    assert!(removed_after >= Duration::from_secs(1), "{removed_after:?}");
    assert!(
        events.contains(&WatchEvent::MissingRemoved {
            root: root.clone(),
            files: 1
        }),
        "{events:#?}"
    );
    assert_eq!(file_state(&library, &song), None);
    assert!(title_of(&library, &song).is_none());
    assert!(title_of(&library, &kept).is_some());
    watcher.stop();
}
