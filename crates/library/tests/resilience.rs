// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What survives a hard stop and a hostile disk.
//!
//! - **`kill -9` during writes:** a child process (this test program run again
//!   with one ignored test selected) writes in a loop: tracks and playlists, a
//!   scan, tag edits with their undo journal, organizer runs. It is killed at
//!   varying moments. Afterwards the database must open, pass
//!   `PRAGMA integrity_check`, keep everything the child reported as done, show
//!   no half-done write, and the files must be whole.
//! - **A full disk:** a child runs in a private mount namespace on a small
//!   tmpfs (`unshare`, no root needed). Writes fail with a clear error, nothing
//!   panics, reads go on, and writes work again once space is free; a backup
//!   that does not fit leaves the older backups alone. Skipped where user
//!   namespaces are not allowed.
//! - **Read-only media:** a database in a read-only folder is refused for
//!   writing with an error that says so and opens read-only; tags of files in a
//!   read-only folder are not written and the files stay as they were.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[path = "support/scanning.rs"]
#[allow(dead_code)]
mod support;
#[path = "support/tracks.rs"]
mod tracks;

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use helikodu_library::backups::Backups;
use helikodu_library::tag_editing::{
    BatchItem, FieldKey, ReadOptions, TagChanges, TagField, load_for_editing,
};
use helikodu_library::{
    CancelToken, CharacterSet, DatabaseConfig, DbError, Destination, Library, MoveRequest,
    OrganizeOptions, Page, ScanOptions, TagWriteSettings,
};
use support::{flac, folder, have_ffmpeg, worker_program};

/// Set in a child process: the folder it works in.
const CHILD: &str = "HELIKODU_RESILIENCE_CHILD";

fn child_folder() -> Option<PathBuf> {
    env::var_os(CHILD).map(PathBuf::from)
}

/// Says to the parent that step `n` is done, durably from the child's side.
fn report(n: i64) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "done {n}").unwrap();
    out.flush().unwrap();
}

/// A child running the ignored test `test` in `dir`, and the last step it
/// reported.
struct Running {
    child: Child,
    last: Arc<AtomicI64>,
    lines: Arc<Mutex<Vec<String>>>,
}

fn spawn(test: &str, dir: &Path) -> Running {
    spawn_with(Command::new(env::current_exe().unwrap()), test, dir)
}

fn spawn_with(mut command: Command, test: &str, dir: &Path) -> Running {
    let mut child = command
        .args([
            "--exact",
            test,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let last = Arc::new(AtomicI64::new(-1));
    let lines = Arc::new(Mutex::new(Vec::new()));
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    {
        let last = Arc::clone(&last);
        let lines = Arc::clone(&lines);
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(n) = line
                    .rsplit_once("done ")
                    .and_then(|(_, n)| n.trim().parse().ok())
                {
                    last.store(n, Ordering::SeqCst);
                }
                lines.lock().unwrap().push(line);
            }
        });
    }
    {
        let lines = Arc::clone(&lines);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                lines.lock().unwrap().push(line);
            }
        });
    }
    Running { child, last, lines }
}

impl Running {
    /// Waits until the child reported a step after `after`.
    fn wait_past(&mut self, after: i64) {
        let start = Instant::now();
        while self.last.load(Ordering::SeqCst) <= after {
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!(
                    "the child stopped ({status}):\n{}",
                    self.lines.lock().unwrap().join("\n")
                );
            }
            assert!(
                start.elapsed() < Duration::from_secs(120),
                "the child made no progress"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Kills the child with SIGKILL and returns the last step it reported.
    fn kill(mut self) -> i64 {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        self.last.load(Ordering::SeqCst)
    }

    /// Waits for the child to end and checks that it succeeded.
    fn finish(mut self) {
        let status = self.child.wait().unwrap();
        thread::sleep(Duration::from_millis(50));
        assert!(
            status.success(),
            "the child failed ({status}):\n{}",
            self.lines.lock().unwrap().join("\n")
        );
    }
}

/// The pause before the kill in round `round`: spread over the work.
fn pause(round: u64) -> Duration {
    Duration::from_millis(3 + (round * 37) % 160)
}

fn integrity(library: &Library) {
    // On the writing connection: a pooled reader that saw the search index
    // before its last merge can report a page of it as missing.
    let result: String = library
        .database()
        .writer()
        .unwrap()
        .write(|c| c.query_row("PRAGMA integrity_check", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(result, "ok");
}

fn open(db: &Path) -> Library {
    Library::open(&DatabaseConfig::file(db)).unwrap()
}

// --- Tracks and playlists ---------------------------------------------------------

#[test]
#[ignore = "run by kill_during_playlist_and_track_writes"]
fn child_playlist_and_track_writes() {
    let Some(dir) = child_folder() else { return };
    let library = open(&dir.join("library.db"));
    let root = library.add_root("/music").unwrap();
    // The built-in Favourites playlist is not one of this test's.
    let start = library
        .playlists(Page::first(1000))
        .unwrap()
        .items
        .iter()
        .filter(|p| !p.builtin)
        .count() as i64;
    for i in start..i64::MAX {
        let track = library
            .upsert_track(tracks::record(
                root,
                &format!("/music/{i}.flac"),
                &format!("content {i}"),
            ))
            .unwrap()
            .track;
        let playlist = library.create_playlist(&format!("P{i:05}")).unwrap();
        library
            .add_to_playlist(playlist, vec![track, track, track])
            .unwrap();
        report(i);
    }
}

#[test]
fn kill_during_playlist_and_track_writes() {
    let dir = folder("resilience-writes");
    let db = dir.join("library.db");
    let mut done = -1;
    for round in 0..6 {
        let mut child = spawn("child_playlist_and_track_writes", &dir);
        child.wait_past(done);
        thread::sleep(pause(round));
        done = done.max(child.kill());

        let library = open(&db);
        integrity(&library);
        let playlists = library.playlists(Page::first(1000)).unwrap().items;
        for playlist in playlists.iter().filter(|p| !p.builtin) {
            // Three tracks go in with one write: all or none.
            assert!(
                playlist.items == 0 || playlist.items == 3,
                "{} has {} items",
                playlist.name,
                playlist.items
            );
        }
        for i in 0..=done {
            let name = format!("P{i:05}");
            let found = playlists.iter().find(|p| p.name == name);
            assert_eq!(found.map(|p| p.items), Some(3), "{name} was reported done");
            assert!(
                library
                    .track_at(Path::new(&format!("/music/{i}.flac")), 0)
                    .unwrap()
                    .is_some()
            );
        }
        // A playlist the child was making when it died is finished by hand, so
        // that the next round's numbering stays in step.
        for playlist in playlists.iter().filter(|p| p.items == 0 && !p.builtin) {
            library.delete_playlist(playlist.id).unwrap();
        }
    }
    assert!(done > 0);
}

// --- A scan -----------------------------------------------------------------------

const SCAN_FILES: usize = 24;

fn scan_options(root: &Path) -> ScanOptions {
    let mut options = ScanOptions::new([root]);
    options.workers.program = Some(worker_program());
    options.workers.count = 2;
    options
}

#[test]
#[ignore = "run by kill_during_a_scan"]
fn child_scan() {
    let Some(dir) = child_folder() else { return };
    let library = open(&dir.join("library.db"));
    report(0);
    library
        .scan(&scan_options(&dir.join("music")), None, &CancelToken::new())
        .unwrap();
    report(1);
    thread::sleep(Duration::from_secs(60));
}

#[test]
fn kill_during_a_scan() {
    if !have_ffmpeg() {
        return;
    }
    let dir = folder("resilience-scan");
    for i in 0..SCAN_FILES {
        flac(
            dir.join(format!("music/Album {}/{i:02} Song.flac", i / 6)),
            300 + 20 * u32::try_from(i).unwrap(),
            1,
            &[("title", &format!("Song {i}")), ("album", "Album")],
        );
    }
    let db = dir.join("library.db");
    for round in 0..4 {
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", db.display()));
        }
        let mut child = spawn("child_scan", &dir);
        child.wait_past(-1);
        thread::sleep(pause(round) * 3);
        child.kill();

        let library = open(&db);
        integrity(&library);
        // A scan after the crash brings the library to the full count.
        library
            .scan(&scan_options(&dir.join("music")), None, &CancelToken::new())
            .unwrap();
        integrity(&library);
        let total = library
            .tracks(&helikodu_library::TrackQuery::default())
            .unwrap()
            .total;
        assert_eq!(total, SCAN_FILES);
    }
}

// --- Tag edits and their journal ----------------------------------------------------

const TAGGED_FILES: usize = 5;

fn tagged_files(dir: &Path) -> Vec<PathBuf> {
    (0..TAGGED_FILES)
        .map(|i| dir.join(format!("music/{i:02} song.flac")))
        .collect()
}

fn title_of(path: &Path) -> String {
    load_for_editing(path, &ReadOptions::default())
        .unwrap()
        .values(&FieldKey::Known(TagField::Title))
        .join("; ")
}

#[test]
#[ignore = "run by kill_during_tag_edits"]
fn child_tag_edits() {
    let Some(dir) = child_folder() else { return };
    let library = open(&dir.join("library.db"));
    let options = scan_options(&dir.join("music"));
    for k in 0..i64::MAX {
        let items: Vec<BatchItem> = tagged_files(&dir)
            .iter()
            .map(|path| {
                let file = load_for_editing(path, &ReadOptions::default()).unwrap();
                let mut changes = TagChanges::new();
                changes.set_one(TagField::Title, format!("Edit {k}"));
                BatchItem::for_file(&file, changes)
            })
            .collect();
        library
            .edit_tags(
                &items,
                &TagWriteSettings::default(),
                &options,
                &mut |_| {},
                &CancelToken::new(),
            )
            .unwrap();
        report(k);
    }
}

#[test]
fn kill_during_tag_edits() {
    if !have_ffmpeg() {
        return;
    }
    let dir = folder("resilience-tags");
    for (i, path) in tagged_files(&dir).iter().enumerate() {
        flac(
            path,
            440 + 10 * u32::try_from(i).unwrap(),
            1,
            &[("title", "Old"), ("album", "Album")],
        );
    }
    let db = dir.join("library.db");
    open(&db)
        .scan(&scan_options(&dir.join("music")), None, &CancelToken::new())
        .unwrap();
    let mut done = -1;
    for round in 0..5 {
        let mut child = spawn("child_tag_edits", &dir);
        child.wait_past(done);
        thread::sleep(pause(round));
        done = done.max(child.kill());

        // Every file is whole and holds a title one of the edits wrote.
        for path in tagged_files(&dir) {
            let title = title_of(&path);
            assert!(title.starts_with("Edit "), "{}: {title:?}", path.display());
        }
        let library = open(&db);
        integrity(&library);
        library.tag_history().unwrap();
        // Leftover temporary copies are cleared and the library catches up.
        library
            .scan(&scan_options(&dir.join("music")), None, &CancelToken::new())
            .unwrap();
        integrity(&library);
    }
    // The newest step can be undone after all this.
    let library = open(&db);
    library
        .undo_tags(
            &TagWriteSettings::default(),
            &scan_options(&dir.join("music")),
        )
        .unwrap();
    integrity(&library);
}

// --- Organizer runs -----------------------------------------------------------------

fn organize_options(pattern: &str) -> OrganizeOptions {
    OrganizeOptions {
        pattern: pattern.into(),
        into: Destination::OwnLibraryFolder,
        characters: CharacterSet::Windows,
        unknown: "Unknown".into(),
    }
}

fn organize_all(library: &Library, pattern: &str) {
    let planned = library
        .plan_organize(None, &organize_options(pattern))
        .unwrap();
    let requests: Vec<MoveRequest> = planned
        .iter()
        .filter(|p| p.change.moves())
        .map(|p| MoveRequest {
            file: p.file,
            from: p.from.clone(),
            to: p.to.clone(),
        })
        .collect();
    library
        .organize(
            &requests,
            Some(pattern),
            true,
            &|_, _| {},
            &CancelToken::new(),
        )
        .unwrap();
}

#[test]
#[ignore = "run by kill_during_organizer_runs"]
fn child_organizer_runs() {
    let Some(dir) = child_folder() else { return };
    let library = open(&dir.join("library.db"));
    library.recover_file_runs().unwrap();
    for k in 0..i64::MAX {
        let pattern = if k % 2 == 0 {
            "{album}/{track} {title}"
        } else {
            "Sorted/{artist} - {title}"
        };
        organize_all(&library, pattern);
        report(k);
    }
}

fn audio_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "flac") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn kill_during_organizer_runs() {
    if !have_ffmpeg() {
        return;
    }
    let dir = folder("resilience-organize");
    let root = dir.join("music");
    for i in 0..8u32 {
        flac(
            root.join(format!("in/{i}.flac")),
            300 + 15 * i,
            1,
            &[
                ("artist", "Okto"),
                ("album", &format!("Album {}", i % 2)),
                ("title", &format!("Song {i}")),
                ("track", &(i + 1).to_string()),
            ],
        );
    }
    let db = dir.join("library.db");
    open(&db)
        .scan(&scan_options(&root), None, &CancelToken::new())
        .unwrap();
    let mut done = -1;
    for round in 0..5 {
        let mut child = spawn("child_organizer_runs", &dir);
        child.wait_past(done);
        thread::sleep(pause(round) / 2);
        done = done.max(child.kill());

        let library = open(&db);
        library.recover_file_runs().unwrap();
        integrity(&library);
        // No file was lost or doubled, and the library knows where each one is.
        let on_disk = audio_files(&root);
        assert_eq!(on_disk.len(), 8, "{on_disk:?}");
        for path in &on_disk {
            assert!(
                library.track_at(path, 0).unwrap().is_some(),
                "the library lost {}",
                path.display()
            );
        }
    }
}

// --- A full disk ----------------------------------------------------------------------

/// Whether a private mount namespace can be made without root.
fn have_namespaces() -> bool {
    let works = Command::new("unshare")
        .args(["-rm", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !works {
        eprintln!("SKIPPED: user namespaces are not available (unshare -rm)");
    }
    works
}

/// Runs the ignored test `test` on a tmpfs of `size` mounted over `dir`.
fn on_small_disk(test: &str, dir: &Path, size: &str) -> Running {
    let mut command = Command::new("unshare");
    command
        .args(["-rm", "sh", "-c"])
        .arg(format!(
            "mount -t tmpfs -o size={size} none \"$1\" && cd \"$1\" && shift && exec \"$@\""
        ))
        .arg("sh")
        .arg(dir)
        .arg(env::current_exe().unwrap());
    spawn_with(command, test, dir)
}

#[test]
#[ignore = "run by a_full_disk_fails_writes_cleanly"]
fn child_full_disk() {
    let Some(dir) = child_folder() else { return };
    let db = dir.join("library.db");
    let library = open(&db);
    let root = library.add_root("/music").unwrap();
    let backups = Backups::beside(&db);
    let first = library.back_up(&backups, SystemTime::now()).unwrap();
    // Fill the disk with tracks until a write fails.
    let mut written = 0usize;
    let failure = loop {
        let batch: Vec<_> = (0..50)
            .map(|j| {
                let i = written * 50 + j;
                let mut record = tracks::record(
                    root,
                    &format!("/music/{i:06} {}.flac", "x".repeat(150)),
                    &format!("c{i}"),
                );
                record.tags.title = Some(format!("{i} {}", "title ".repeat(30)));
                record
            })
            .collect();
        match library.upsert_tracks(batch) {
            Ok(_) => written += 1,
            Err(e) => break e,
        }
        assert!(written < 100_000, "the disk never filled");
    };
    assert!(
        failure.is_disk_full() || matches!(failure, DbError::BatchFailed(_)),
        "{failure}"
    );
    assert!(written > 0);
    // Reads go on.
    let total = library
        .tracks(&helikodu_library::TrackQuery::default())
        .unwrap()
        .total;
    assert_eq!(total, written * 50);
    // A backup that does not fit fails and leaves the first one alone.
    let later = SystemTime::now() + Duration::from_secs(3600);
    assert!(library.back_up(&backups, later).is_err());
    assert_eq!(backups.list(), vec![first.clone()]);
    let leftovers: Vec<_> = fs::read_dir(backups.folder())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n.to_string_lossy().contains("partial"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // Once there is room again, writes work.
    fs::remove_file(&first.path).unwrap();
    library
        .upsert_track(tracks::record(root, "/music/after.flac", "after"))
        .unwrap();
    drop(library);
    let library = open(&db);
    integrity(&library);
    assert!(
        library
            .track_at(Path::new("/music/after.flac"), 0)
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_full_disk_fails_writes_cleanly() {
    if !have_namespaces() {
        return;
    }
    let dir = folder("resilience-full-disk");
    on_small_disk("child_full_disk", &dir, "3m").finish();
}

// --- Read-only media ------------------------------------------------------------------

/// Makes `path` read-only (or writable again) for its owner.
fn set_read_only(path: &Path, read_only: bool) {
    let mode = if read_only { 0o555 } else { 0o755 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// Whether permissions are enforced: not for root.
fn permissions_enforced(dir: &Path) -> bool {
    let probe = dir.join("probe");
    fs::create_dir_all(&probe).unwrap();
    set_read_only(&probe, true);
    let enforced = fs::write(probe.join("x"), b"x").is_err();
    set_read_only(&probe, false);
    fs::remove_dir_all(&probe).unwrap();
    if !enforced {
        eprintln!("SKIPPED: file permissions are not enforced for this user");
    }
    enforced
}

/// The database file and its log files, those that exist.
fn database_files(db: &Path) -> Vec<PathBuf> {
    ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| PathBuf::from(format!("{}{suffix}", db.display())))
        .filter(|p| p.exists())
        .collect()
}

#[test]
fn a_database_on_read_only_media_opens_read_only() {
    let dir = folder(&format!("resilience-read-only-db-{}", std::process::id()));
    if !permissions_enforced(&dir) {
        return;
    }
    let data = dir.join("data");
    fs::create_dir_all(&data).unwrap();
    let db = data.join("library.db");
    {
        let library = open(&db);
        library.create_playlist("Mine").unwrap();
    }
    // Once with the log beside the file, once with everything folded in.
    for settle in [false, true] {
        if settle {
            let c = helikodu_library::raw::rusqlite::Connection::open(&db).unwrap();
            let _: i64 = c
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
        }
        // As on a read-only mount: neither the folder nor the files can be written.
        let files = database_files(&db);
        for file in &files {
            fs::set_permissions(file, fs::Permissions::from_mode(0o444)).unwrap();
        }
        set_read_only(&data, true);
        let error = Library::open(&DatabaseConfig::file(&db)).unwrap_err();
        assert!(error.is_read_only_medium(), "{error}");
        let library = Library::open_read_only(&DatabaseConfig::file(&db)).unwrap();
        let names: Vec<String> = library
            .playlists(Page::first(10))
            .unwrap()
            .items
            .into_iter()
            .filter(|p| !p.builtin)
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["Mine"], "settled: {settle}");
        assert!(matches!(
            library.create_playlist("No").unwrap_err(),
            helikodu_library::PlaylistError::Db(DbError::ReadOnly)
        ));
        drop(library);
        set_read_only(&data, false);
        for file in &files {
            fs::set_permissions(file, fs::Permissions::from_mode(0o644)).unwrap();
        }
    }
    // A database that cannot be created says why.
    let empty = dir.join("empty");
    fs::create_dir_all(&empty).unwrap();
    set_read_only(&empty, true);
    let error = Library::open(&DatabaseConfig::file(empty.join("library.db"))).unwrap_err();
    set_read_only(&empty, false);
    assert!(error.is_read_only_medium(), "{error}");
}

#[test]
fn tags_in_a_read_only_folder_are_left_alone() {
    if !have_ffmpeg() {
        return;
    }
    let dir = folder(&format!("resilience-read-only-tags-{}", std::process::id()));
    if !permissions_enforced(&dir) {
        return;
    }
    let music = dir.join("music");
    let file = music.join("song.flac");
    flac(&file, 440, 1, &[("title", "Old")]);
    let before = fs::read(&file).unwrap();
    let library = open(&dir.join("library.db"));
    let options = scan_options(&music);
    library.scan(&options, None, &CancelToken::new()).unwrap();
    // The folder is read-only, and so is the file, as on a read-only mount.
    fs::set_permissions(&file, fs::Permissions::from_mode(0o444)).unwrap();
    set_read_only(&music, true);
    let loaded = load_for_editing(&file, &ReadOptions::default()).unwrap();
    let mut changes = TagChanges::new();
    changes.set_one(TagField::Title, "New");
    let report = library
        .edit_tags(
            &[BatchItem::for_file(&loaded, changes)],
            &TagWriteSettings::default(),
            &options,
            &mut |_| {},
            &CancelToken::new(),
        )
        .unwrap();
    set_read_only(&music, false);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    let failed: Vec<_> = report.write.batch.failed().collect();
    assert_eq!(failed.len(), 1, "the write was reported as failed");
    assert_eq!(fs::read(&file).unwrap(), before, "the file is unchanged");
    assert!(report.step.is_none(), "nothing to undo");
    assert_eq!(title_of(&file), "Old");
    integrity(&library);
}
