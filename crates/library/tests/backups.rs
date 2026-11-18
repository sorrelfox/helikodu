// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Backups of the database: made while writes go on, rotated, checked; a backup
//! put back at the next start, with the replaced database kept; a damaged
//! database replaced by the newest good backup; and files that are not a
//! backup refused.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod tracks;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use helikodu_library::backups::{
    BackupError, Backups, apply_pending_restore, pending_restore, recover_damaged, stage_restore,
};
use helikodu_library::{DatabaseConfig, Library, TrackQuery};

fn scratch(test: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "library-backups-{test}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_790_000_000 + seconds)
}

fn open(path: &Path) -> Library {
    Library::open(&DatabaseConfig::file(path)).unwrap()
}

fn playlist_names(library: &Library) -> Vec<String> {
    let mut names: Vec<String> = library
        .playlists(helikodu_library::Page::default())
        .unwrap()
        .items
        .into_iter()
        .filter(|p| !p.builtin)
        .map(|p| p.name)
        .collect();
    names.sort();
    names
}

#[test]
fn a_backup_is_made_while_writes_go_on_and_old_ones_are_rotated() {
    let dir = scratch("rotate");
    let db = dir.join("library.db");
    let library = Arc::new(open(&db));
    let root = library.add_root("/music").unwrap();
    let backups = Backups::beside(&db).keeping(3);
    assert!(backups.is_due(at(0)));

    // A writer keeps adding tracks while the backups are made.
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let library = Arc::clone(&library);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut i = 0;
            while !stop.load(Ordering::Relaxed) {
                library
                    .upsert_track(tracks::record(
                        root,
                        &format!("/music/{i}.flac"),
                        &i.to_string(),
                    ))
                    .unwrap();
                i += 1;
            }
            i
        })
    };
    for day in 0..5 {
        let made = library.back_up(&backups, at(day * 86_400)).unwrap();
        assert!(made.path.is_file());
        assert!(made.size > 0);
    }
    stop.store(true, Ordering::Relaxed);
    assert!(writer.join().unwrap() > 0);

    let list = backups.list();
    assert_eq!(list.len(), 3, "{list:?}");
    assert_eq!(list[0].made, at(4 * 86_400));
    assert_eq!(list[2].made, at(2 * 86_400));
    assert!(!backups.is_due(at(4 * 86_400 + 3600)));
    assert!(backups.is_due(at(5 * 86_400)));
    let check = backups.last_check().unwrap();
    assert_eq!(check.at, at(4 * 86_400));
    assert_eq!(check.problem, None);
    // No temporary file is left.
    let names: Vec<String> = fs::read_dir(backups.folder())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.contains("partial")), "{names:?}");

    // Each backup opens as a library of its own.
    let copy = Library::open_read_only(&DatabaseConfig::file(&list[0].path)).unwrap();
    assert_eq!(copy.roots().unwrap().len(), 1);
}

#[test]
fn a_restore_waits_for_the_next_start_and_keeps_the_replaced_database() {
    let dir = scratch("restore");
    let db = dir.join("library.db");
    let backups = Backups::beside(&db);
    let library = open(&db);
    library.create_playlist("Kept").unwrap();
    let backup = library.back_up(&backups, at(0)).unwrap();
    library.create_playlist("Made after the backup").unwrap();

    stage_restore(&db, &backup.path).unwrap();
    assert!(pending_restore(&db).is_some());
    // The running library is untouched until the next start.
    assert_eq!(playlist_names(&library).len(), 2);
    drop(library);

    let outcome = apply_pending_restore(&db, &backups, at(60))
        .unwrap()
        .unwrap();
    assert!(pending_restore(&db).is_none());
    let library = open(&db);
    assert_eq!(playlist_names(&library), ["Kept"]);
    // The replaced database was saved, with what was made after the backup.
    let saved = outcome.saved_copy.unwrap();
    let before = Library::open_read_only(&DatabaseConfig::file(&saved)).unwrap();
    assert_eq!(playlist_names(&before).len(), 2);
    // Nothing waits any more.
    assert!(
        apply_pending_restore(&db, &backups, at(120))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_damaged_database_is_replaced_by_the_newest_good_backup() {
    let dir = scratch("damaged");
    let db = dir.join("library.db");
    let backups = Backups::beside(&db);
    {
        let library = open(&db);
        library.create_playlist("Old").unwrap();
        library.back_up(&backups, at(0)).unwrap();
        library.create_playlist("Newer").unwrap();
        library.back_up(&backups, at(86_400)).unwrap();
        library.create_playlist("Lost").unwrap();
    }
    settle(&db);
    // The newest backup is damaged too: it is skipped.
    let newest = backups.latest().unwrap().path;
    scribble(&newest);
    scribble(&db);
    let error = Library::open(&DatabaseConfig::file(&db)).unwrap_err();
    assert!(error.is_damaged_database(), "{error}");

    let (used, outcome) = recover_damaged(&db, &backups, at(2 * 86_400))
        .unwrap()
        .unwrap();
    assert_eq!(used.made, at(0));
    assert!(outcome.damaged_copy.is_some() || outcome.saved_copy.is_some());
    let library = open(&db);
    assert_eq!(playlist_names(&library), ["Old"]);
}

/// Folds the write-ahead log into the database file: the last reading
/// connection to close cannot.
fn settle(path: &Path) {
    let c = helikodu_library::raw::rusqlite::Connection::open(path).unwrap();
    // A connection that read nothing never opened the log.
    let _: i64 = c
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    drop(c);
    assert!(!path.with_extension("db-wal").exists());
}

/// Overwrites the pages after the first with garbage.
fn scribble(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    let len = bytes.len();
    for (i, b) in bytes[4096.min(len)..].iter_mut().enumerate() {
        *b = (i * 7 % 251) as u8;
    }
    // The header claims more pages than there are, which SQLite notices at once.
    bytes[28..32].copy_from_slice(&u32::MAX.to_be_bytes());
    fs::write(path, bytes).unwrap();
}

#[test]
fn files_that_are_not_backups_are_refused() {
    let dir = scratch("refuse");
    let db = dir.join("library.db");
    drop(open(&db));
    settle(&db);
    let text = dir.join("notes.db");
    fs::write(&text, "not a database").unwrap();
    assert!(matches!(
        stage_restore(&db, &text),
        Err(BackupError::NotALibrary(_))
    ));
    let empty = dir.join("empty.db");
    drop(helikodu_library::raw::rusqlite::Connection::open(&empty).unwrap());
    assert!(matches!(
        stage_restore(&db, &empty),
        Err(BackupError::NotALibrary(_))
    ));
    // A backup from a newer version.
    let newer = dir.join("newer.db");
    fs::copy(&db, &newer).unwrap();
    let c = helikodu_library::raw::rusqlite::Connection::open(&newer).unwrap();
    c.execute(
        "INSERT INTO schema_migrations (version, name, applied_at) VALUES (9999, 'future', 0)",
        [],
    )
    .unwrap();
    drop(c);
    assert!(matches!(
        stage_restore(&db, &newer),
        Err(BackupError::Newer { found: 9999, .. })
    ));
    assert!(pending_restore(&db).is_none());
}

#[test]
fn a_backup_of_a_library_holds_its_tracks() {
    let dir = scratch("tracks");
    let db = dir.join("library.db");
    let library = open(&db);
    let root = library.add_root("/music").unwrap();
    for i in 0..50 {
        library
            .upsert_track(tracks::record(
                root,
                &format!("/music/{i}.flac"),
                &format!("c{i}"),
            ))
            .unwrap();
    }
    let backups = Backups::beside(&db);
    let made = library.back_up(&backups, at(0)).unwrap();
    let copy = Library::open_read_only(&DatabaseConfig::file(&made.path)).unwrap();
    let all = copy.tracks(&TrackQuery::default()).unwrap();
    assert_eq!(all.total, 50);
}
