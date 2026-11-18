// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Closing the window quits: over the running service, playing a generated
//! track to the null sink, closing the main window ends the application, and
//! the program can end within two seconds, the service and its threads
//! with it. (Keeping the window in the tray instead is tested in
//! `desktop.rs`.)
//!
//! Like `dialogs.rs`, this program runs only under the headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test closing
//! ```
//!
//! Without `ffmpeg` it prints `SKIPPED:` and passes.

#![forbid(unsafe_code)]
// A panic is how a test fails; this is a test without the test harness.
#![allow(clippy::unwrap_used)]

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_service::files::AppDirs;
use helikodu_service::library::{MusicLibrary, ScanRequest};
use helikodu_service::{LibraryChoice, Service, ServiceConfig};
use helikodu_ui_gtk::backend::{AlbumQuery, Backend, PlayerRequest, service};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display};

/// How long quitting may take, from the close to the end of the program.
const QUIT_WITHIN: Duration = Duration::from_secs(2);

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The `helikodu-scan-worker` program of this build, built here: it belongs
/// to another package, so Cargo does not build it for these tests.
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

/// A library of one minute of tone under `dir`, scanned, and the backend over
/// a service that plays to the null sink.
fn backend(dir: &Path) -> Backend {
    let _ = std::fs::remove_dir_all(dir);
    let file = dir.join("music").join("Album").join("01 Tone.flac");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=60:sample_rate=44100",
            "-metadata",
            "title=Tone",
            "-metadata",
            "album=Album",
            "-c:a",
            "flac",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert!(output.status.success(), "ffmpeg failed");
    let worker = worker_program();
    let database = dir.join("library.db");
    let scanned = MusicLibrary::open(&database).unwrap();
    scanned
        .scan(ScanRequest {
            folders: vec![dir.join("music")],
            worker_program: Some(worker.clone()),
            ..ScanRequest::default()
        })
        .unwrap()
        .join()
        .unwrap();
    drop(scanned);
    let dirs = AppDirs::under(&dir.join("service"));
    std::fs::create_dir_all(&dirs.config).unwrap();
    std::fs::write(
        dirs.config.join("settings.toml"),
        format!(
            "version = 2\n[library]\nfolders = [{:?}]\nwatch = false\n[output]\nbackend = \"null\"\n",
            dir.join("music")
        ),
    )
    .unwrap();
    let mut config = ServiceConfig::new(dirs);
    config.library = LibraryChoice::Open(MusicLibrary::open_existing(&database).unwrap());
    config.thumbnails = false;
    config.worker_program = Some(worker);
    let running = Service::start(config).unwrap();
    service::connect(
        running,
        service::ServiceOptions {
            mpris: false,
            inhibit_suspend: false,
        },
    )
    .unwrap()
}

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("closing: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if !have_ffmpeg() {
        println!("SKIPPED: ffmpeg is not installed");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("closing: skipped, no display");
        return ExitCode::SUCCESS;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ui-closing");
    let backend = backend(&dir);
    // A program that does not end is a failure, not a hang of the test run.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(60));
        println!("test close_quits_while_playing ... FAILED (the program did not end)");
        std::process::exit(1);
    });

    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.ClosingTests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let closed_at: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    let played = Rc::new(Cell::new(false));
    let shared = Rc::new(RefCell::new(Some(backend)));
    let (c, p, b) = (closed_at.clone(), played.clone(), shared.clone());
    application.connect_activate(move |application| {
        let Some(backend) = b.borrow().clone() else {
            return;
        };
        install_actions(application, &backend);
        let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
        window.present();
        let album = backend
            .library()
            .album_keys(&AlbumQuery::default())
            .unwrap()[0];
        backend.send(PlayerRequest::PlayAlbum {
            album,
            start: 0,
            shuffle: false,
        });
        let (c, p) = (c.clone(), p.clone());
        let started = Instant::now();
        glib::timeout_add_local(Duration::from_millis(20), move || {
            if !window.is_playing() && started.elapsed() < Duration::from_secs(20) {
                return glib::ControlFlow::Continue;
            }
            p.set(window.is_playing());
            // Some time playing, then the close button.
            let (c, window) = (c.clone(), window.clone());
            glib::timeout_add_local_once(Duration::from_millis(500), move || {
                c.set(Some(Instant::now()));
                window.close();
            });
            glib::ControlFlow::Break
        });
    });
    application.run_with_args(&["closing"]);
    let returned = closed_at.get().map(|t| t.elapsed());
    // What the program drops on its way out: the window's backend and service.
    drop(application);
    shared.borrow_mut().take();
    let ended = closed_at.get().map(|t| t.elapsed());
    let ok = played.get()
        && returned.is_some_and(|t| t < QUIT_WITHIN)
        && ended.is_some_and(|t| t < QUIT_WITHIN);
    if ok {
        println!("test close_quits_while_playing ... ok");
        println!("closing: passed");
        ExitCode::SUCCESS
    } else {
        println!(
            "test close_quits_while_playing ... FAILED (played {}, run returned after {returned:?}, ended after {ended:?})",
            played.get()
        );
        ExitCode::FAILURE
    }
}
