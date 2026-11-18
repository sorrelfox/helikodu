// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the service keeps through a hard stop, a full disk and read-only media,
//! and what it says about it at the next start.
//!
//! - `kill -9` while the settings and the window state are saved over and over:
//!   each file is always the old one or the new one, never a mixture.
//! - A full disk (a small tmpfs in a private mount namespace; skipped where user
//!   namespaces are not allowed): saving the settings fails with an error and
//!   the file on disk stays as it was.
//! - A library on read-only media opens read-only and says so; one that cannot
//!   be opened leaves the service running without a library; a backup staged to
//!   be put back is put back; a damaged database is replaced with a backup.
//! - A panic writes a crash report, even with the log off, and the log off
//!   writes nothing at all.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use helikodu_model::settings::{LogDetail, Settings, UiState};
use helikodu_service::bus::EventBus;
use helikodu_service::files::AppDirs;
use helikodu_service::library::backups::StartNotice;
use helikodu_service::library::{DATABASE_FILE, MusicLibrary};
use helikodu_service::settings::{SettingsStore, SettingsStoreError, UiStateStore};
use helikodu_service::{LibraryChoice, Service, ServiceConfig, crash, logging};

const CHILD: &str = "HELIKODU_SERVICE_RESILIENCE_CHILD";

fn child_folder() -> Option<PathBuf> {
    env::var_os(CHILD).map(PathBuf::from)
}

fn folder(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("service-resilience")
        .join(format!("{test}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn report(n: i64) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "done {n}").unwrap();
    out.flush().unwrap();
}

fn spawn_with(mut command: Command, test: &str, dir: &Path) -> (Child, Arc<AtomicI64>) {
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
        .spawn()
        .unwrap();
    let last = Arc::new(AtomicI64::new(-1));
    let stdout = child.stdout.take().unwrap();
    let seen = Arc::clone(&last);
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(n) = line
                .rsplit_once("done ")
                .and_then(|(_, n)| n.trim().parse().ok())
            {
                seen.store(n, Ordering::SeqCst);
            }
        }
    });
    (child, last)
}

fn spawn(test: &str, dir: &Path) -> (Child, Arc<AtomicI64>) {
    spawn_with(Command::new(env::current_exe().unwrap()), test, dir)
}

fn wait_past(last: &AtomicI64, after: i64) {
    let start = Instant::now();
    while last.load(Ordering::SeqCst) <= after {
        assert!(start.elapsed() < Duration::from_secs(60), "no progress");
        thread::sleep(Duration::from_millis(1));
    }
}

// --- kill -9 during saves -------------------------------------------------------------

#[test]
#[ignore = "run by kill_during_settings_and_window_state_saves"]
fn child_saves() {
    let Some(dir) = child_folder() else { return };
    let settings = SettingsStore::open(dir.join("settings.toml"), EventBus::new());
    let window = UiStateStore::new(dir.join("ui-state.toml"));
    for i in 0..i64::MAX {
        settings
            .update(|s| {
                s.library.storage.covers_mib = if i % 2 == 0 { 512 } else { 1024 };
                s.library.folders = (0..i % 7)
                    .map(|n| PathBuf::from(format!("/m/{n}")))
                    .collect();
            })
            .unwrap();
        let mut state = UiState {
            last_view: format!("view {i}"),
            ..UiState::default()
        };
        state.panes.sidebar_width = if i % 2 == 0 { 220 } else { 300 };
        window.save(&state).unwrap();
        report(i);
    }
}

#[test]
fn kill_during_settings_and_window_state_saves() {
    let dir = folder("saves");
    let mut done = -1;
    for round in 0..8u64 {
        let (mut child, last) = spawn("child_saves", &dir);
        wait_past(&last, done);
        thread::sleep(Duration::from_millis(1 + round * 7));
        child.kill().unwrap();
        child.wait().unwrap();
        done = done.max(last.load(Ordering::SeqCst));

        let text = fs::read_to_string(dir.join("settings.toml")).unwrap();
        let parsed: Settings = toml::from_str(&text).unwrap();
        assert!([512, 1024].contains(&parsed.library.storage.covers_mib));
        let store = SettingsStore::open(dir.join("settings.toml"), EventBus::new());
        assert_eq!(store.load_notice(), None);
        let text = fs::read_to_string(dir.join("ui-state.toml")).unwrap();
        let state: UiState = toml::from_str(&text).unwrap();
        assert!(state.last_view.starts_with("view "), "{text}");
    }
    assert!(done > 0);
}

// --- A full disk ------------------------------------------------------------------------

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

#[test]
#[ignore = "run by a_full_disk_keeps_the_saved_settings"]
fn child_full_disk() {
    let Some(dir) = child_folder() else { return };
    let path = dir.join("settings.toml");
    let store = SettingsStore::open(&path, EventBus::new());
    store
        .update(|s| s.library.storage.covers_mib = 512)
        .unwrap();
    let before = fs::read(&path).unwrap();
    // Fill the disk.
    let mut filler = fs::File::create(dir.join("filler")).unwrap();
    let block = vec![0u8; 64 << 10];
    while filler.write_all(&block).is_ok() {}
    drop(filler);
    let error = store
        .update(|s| {
            s.library.storage.covers_mib = 1024;
            s.library.folders = (0..2000)
                .map(|n| PathBuf::from(format!("/m/{n}")))
                .collect();
        })
        .unwrap_err();
    assert!(matches!(error, SettingsStoreError::Write { .. }), "{error}");
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "the saved file is unchanged"
    );
    assert_eq!(store.get().library.storage.covers_mib, 512);
    let temporaries: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n.to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(temporaries.is_empty(), "{temporaries:?}");
    let window = UiStateStore::new(dir.join("ui-state.toml"));
    assert!(window.save(&UiState::default()).is_err());
    // With room again, saving works.
    fs::remove_file(dir.join("filler")).unwrap();
    store
        .update(|s| s.library.storage.covers_mib = 1024)
        .unwrap();
    report(0);
}

#[test]
fn a_full_disk_keeps_the_saved_settings() {
    if !have_namespaces() {
        return;
    }
    let dir = folder("full-disk");
    let mut command = Command::new("unshare");
    command
        .args(["-rm", "sh", "-c"])
        .arg("mount -t tmpfs -o size=1m none \"$1\" && cd \"$1\" && shift && exec \"$@\"")
        .arg("sh")
        .arg(&dir)
        .arg(env::current_exe().unwrap());
    let (mut child, last) = spawn_with(command, "child_full_disk", &dir);
    let status = child.wait().unwrap();
    thread::sleep(Duration::from_millis(50));
    assert!(status.success(), "{status}");
    assert_eq!(last.load(Ordering::SeqCst), 0);
}

// --- The library at start ---------------------------------------------------------------

fn permissions_enforced(dir: &Path) -> bool {
    let probe = dir.join("probe");
    fs::create_dir_all(&probe).unwrap();
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o555)).unwrap();
    let enforced = fs::write(probe.join("x"), b"x").is_err();
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(&probe).unwrap();
    if !enforced {
        eprintln!("SKIPPED: file permissions are not enforced for this user");
    }
    enforced
}

fn start(dirs: &AppDirs) -> Service {
    fs::create_dir_all(&dirs.config).unwrap();
    let settings = dirs.config.join("settings.toml");
    if !settings.exists() {
        fs::write(&settings, "version = 1\n[output]\nbackend = \"null\"\n").unwrap();
    }
    let mut config = ServiceConfig::new(dirs.clone());
    config.library = LibraryChoice::Default;
    config.thumbnails = false;
    config.backups_after = None;
    config.session_debounce = Duration::ZERO;
    Service::start(config).unwrap()
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn a_library_on_read_only_media_opens_read_only_and_says_so() {
    let root = folder("read-only");
    if !permissions_enforced(&root) {
        return;
    }
    let dirs = AppDirs::under(&root);
    let db = dirs.data.join(DATABASE_FILE);
    {
        let library = MusicLibrary::open(&db).unwrap();
        assert!(!library.is_read_only());
    }
    let files: Vec<PathBuf> = ["", "-wal", "-shm"]
        .iter()
        .map(|s| PathBuf::from(format!("{}{s}", db.display())))
        .filter(|p| p.exists())
        .collect();
    for file in &files {
        set_mode(file, 0o444);
    }
    set_mode(&dirs.data, 0o555);
    let service = start(&dirs);
    let notices = service.start_notices().to_vec();
    let read_only = service.library().map(|l| l.library().is_read_only());
    drop(service);
    set_mode(&dirs.data, 0o755);
    for file in &files {
        set_mode(file, 0o644);
    }
    assert!(
        matches!(notices.as_slice(), [StartNotice::ReadOnly(_)]),
        "{notices:?}"
    );
    assert_eq!(read_only, Some(true));
}

#[test]
fn a_library_that_cannot_be_opened_leaves_the_service_without_one() {
    let root = folder("unavailable");
    let dirs = AppDirs::under(&root);
    fs::create_dir_all(&dirs.data).unwrap();
    // Not a database at all, and no backup to fall back on.
    fs::write(dirs.data.join(DATABASE_FILE), vec![7u8; 8192]).unwrap();
    let service = start(&dirs);
    assert!(service.library().is_none());
    assert!(
        matches!(service.start_notices(), [StartNotice::Unavailable(_)]),
        "{:?}",
        service.start_notices()
    );
    // The file is left as it was, for the user to look at.
    assert_eq!(
        fs::read(dirs.data.join(DATABASE_FILE)).unwrap(),
        vec![7u8; 8192]
    );
}

#[test]
fn a_staged_backup_is_put_back_at_the_next_start() {
    let root = folder("restore");
    let db = root.join(DATABASE_FILE);
    let (library, notices) = MusicLibrary::open_at_start(&db);
    let library = library.unwrap();
    assert!(notices.is_empty(), "{notices:?}");
    library.create_playlist("Before").unwrap();
    let backup = library.back_up_now().unwrap();
    library.create_playlist("After").unwrap();
    library.stage_restore(&backup.path).unwrap();
    assert!(library.pending_restore().is_some());
    drop(library);

    let (library, notices) = MusicLibrary::open_at_start(&db);
    let library = library.unwrap();
    assert!(
        matches!(
            notices.as_slice(),
            [StartNotice::Restored {
                saved_copy: Some(_)
            }]
        ),
        "{notices:?}"
    );
    let names: Vec<String> = library
        .playlists()
        .unwrap()
        .into_iter()
        .filter(|p| !p.builtin)
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["Before"]);
    assert!(library.pending_restore().is_none());
}

#[test]
fn a_damaged_database_is_replaced_with_a_backup_at_start() {
    let root = folder("damaged");
    let db = root.join(DATABASE_FILE);
    {
        let library = MusicLibrary::open(&db).unwrap();
        library.create_playlist("Saved").unwrap();
        library.back_up_now().unwrap();
    }
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", db.display()));
    }
    fs::write(&db, b"SQLite format 3\0 but nothing after that is right").unwrap();
    let (library, notices) = MusicLibrary::open_at_start(&db);
    let library = library.unwrap();
    assert!(
        matches!(notices.as_slice(), [StartNotice::Recovered { .. }]),
        "{notices:?}"
    );
    let names: Vec<String> = library
        .playlists()
        .unwrap()
        .into_iter()
        .filter(|p| !p.builtin)
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["Saved"]);
}

// --- Crash reports and the log off --------------------------------------------------------

#[test]
#[ignore = "run by a_panic_writes_a_crash_report_even_with_the_log_off"]
fn child_panics() {
    let Some(dir) = child_folder() else { return };
    logging::install(logging::LogConfig {
        folder: dir.join(logging::LOG_FOLDER),
        detail: LogDetail::Off,
        limit: 50 << 20,
        terminal: false,
    });
    crash::install_panic_hook(&dir);
    log::warn!("a warning that stays in memory");
    log::error!("an error that stays in memory");
    let worker = thread::Builder::new()
        .name("helikodu-test-worker".into())
        .spawn(|| panic!("the test asked for a panic"))
        .unwrap();
    assert!(worker.join().is_err());
    report(0);
}

#[test]
fn a_panic_writes_a_crash_report_even_with_the_log_off() {
    let dir = folder("crash");
    let (mut child, last) = spawn("child_panics", &dir);
    assert!(child.wait().unwrap().success());
    thread::sleep(Duration::from_millis(50));
    assert_eq!(last.load(Ordering::SeqCst), 0);
    assert!(
        !dir.join(logging::LOG_FOLDER).exists(),
        "the log off wrote a log folder"
    );
    let reports = crash::unseen_reports(&dir);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].summary, "the test asked for a panic");
    let text = reports[0].text();
    assert!(text.contains("Thread: helikodu-test-worker"), "{text}");
    assert!(text.contains("the log is off"), "{text}");
    assert!(!text.contains("stays in memory"), "{text}");
    crash::mark_seen(&dir).unwrap();
    assert!(crash::unseen_reports(&dir).is_empty());
}
