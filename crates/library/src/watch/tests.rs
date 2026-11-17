// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The watcher's handling of events the kernel rarely sends: a queue overflow and the
//! watch limit, fed to a running watcher by hand.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::event::Flag;
use notify::{Event, EventKind};

use super::*;
use crate::DatabaseConfig;

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("hk-watch-{}", std::process::id()))
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    // Not empty, so it is not mistaken for a mount point left behind.
    fs::write(dir.join("readme.txt"), b"x").unwrap();
    dir
}

fn start(root: &PathBuf) -> Watcher {
    let library = Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap());
    let mut options = WatchOptions::new(ScanOptions::new([root]));
    options.initial_scan = false;
    let watcher = Watcher::spawn(library, options).unwrap();
    wait_for(&watcher, |e| {
        matches!(
            e,
            WatchEvent::Watching {
                mode: WatchMode::Inotify,
                ..
            }
        )
    });
    watcher
}

/// The events up to the first that `wanted` accepts, which is the last.
fn wait_for(watcher: &Watcher, wanted: impl Fn(&WatchEvent) -> bool) -> Vec<WatchEvent> {
    let deadline = Instant::now() + Duration::from_secs(10);
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

#[test]
fn an_overflow_rescans_the_whole_root() {
    let root = folder("overflow");
    let watcher = start(&root);
    watcher.inject(0, Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan)));
    let events = wait_for(&watcher, |e| matches!(e, WatchEvent::RescanFinished { .. }));
    assert!(
        events.contains(&WatchEvent::Overflow { root: root.clone() }),
        "{events:#?}"
    );
    assert!(
        events.contains(&WatchEvent::RescanStarted {
            reason: RescanReason::Overflow,
            roots: vec![root.clone()],
            full: true,
        }),
        "{events:#?}"
    );
    watcher.stop();
}

#[test]
fn the_watch_limit_turns_to_polling() {
    let root = folder("watch-limit");
    let watcher = start(&root);
    watcher.inject(0, Err(notify::Error::new(notify::ErrorKind::MaxFilesWatch)));
    let events = wait_for(&watcher, |e| matches!(e, WatchEvent::RescanFinished { .. }));
    let polling = events.iter().find_map(|e| match e {
        WatchEvent::Watching {
            mode: WatchMode::Polling { reason, .. },
            ..
        } => Some(reason.clone()),
        _ => None,
    });
    assert!(
        matches!(polling, Some(PollReason::WatchLimit { max_user_watches: Some(n) }) if n > 0),
        "{events:#?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            WatchEvent::RescanStarted {
                reason: RescanReason::WatchLimit,
                full: true,
                ..
            }
        )),
        "{events:#?}"
    );
    // Events of the dropped watch are ignored from now on.
    watcher.inject(0, Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan)));
    watcher.rescan(None);
    let events = wait_for(&watcher, |e| matches!(e, WatchEvent::RescanFinished { .. }));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, WatchEvent::Overflow { .. })),
        "{events:#?}"
    );
    watcher.stop();
}

#[test]
fn an_error_is_reported_and_the_watcher_goes_on() {
    let root = folder("error");
    let watcher = start(&root);
    watcher.inject(0, Err(notify::Error::generic("a test error")));
    let events = wait_for(&watcher, |e| matches!(e, WatchEvent::Error { .. }));
    assert!(
        matches!(events.last(), Some(WatchEvent::Error { root: Some(r), message }) if *r == root && message.contains("a test error")),
        "{events:#?}"
    );
    watcher.rescan(Some(root.clone()));
    wait_for(&watcher, |e| matches!(e, WatchEvent::RescanFinished { .. }));
    drop(watcher);
}
