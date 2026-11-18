// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tag editor over the running service: a library generated with the
//! `ffmpeg` tool (two FLAC files and a Matroska file, whose tags can only be
//! read) is scanned, edited through the dialog and read back from the files
//! and from the library. Nothing is played; the output is the null sink.
//!
//! Like `dialogs.rs`, this program runs only under the headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test tag_editing
//! ```
//!
//! Without `ffmpeg` it prints `SKIPPED:` and passes.

#![forbid(unsafe_code)]
// A panic is how a test fails; these are tests without the test harness.
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_service::files::AppDirs;
use helikodu_service::library::{MusicLibrary, ScanRequest, read_file_info};
use helikodu_service::{LibraryChoice, Service, ServiceConfig};
use helikodu_ui_gtk::backend::{Backend, TrackKey, TrackQuery, service};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display};

fn spin(ms: u64) {
    let context = glib::MainContext::default();
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for(what: &str, done: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        spin(20);
    }
}

fn toplevel(name: &str) -> Option<gtk::Window> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .find(|w| w.widget_name() == name && w.is_visible())
}

fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut all = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(widget) = pending.pop() {
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c.clone());
            all.push(c);
        }
    }
    all
}

fn button(root: &impl IsA<gtk::Widget>, text: &str) -> gtk::Button {
    descendants(root.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| {
            b.label().as_deref() == Some(text)
                || descendants(b.upcast_ref()).iter().any(|w| {
                    w.downcast_ref::<gtk::Label>()
                        .is_some_and(|l| l.text() == text)
                })
        })
        .unwrap_or_else(|| panic!("no button {text}"))
}

fn labels(root: &impl IsA<gtk::Widget>) -> Vec<String> {
    descendants(root.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Label>().ok())
        .map(|l| l.text().to_string())
        .collect()
}

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

/// A second of tone at `path`, encoded with `codec` into the container the
/// extension names, tagged with `tags`.
fn encode(path: &Path, codec: &str, tags: &[(&str, &str)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut command = Command::new("ffmpeg");
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=1:sample_rate=44100",
    ]);
    for (key, value) in tags {
        command.arg("-metadata").arg(format!("{key}={value}"));
    }
    let output = command.args(["-c:a", codec]).arg(path).output().unwrap();
    assert!(
        output.status.success(),
        "ffmpeg: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A library of three tracks in `dir`, scanned, and the backend over a service
/// that plays to the null sink.
fn library(dir: &Path) -> (Backend, Vec<PathBuf>) {
    let music = dir.join("music").join("Album");
    let files = vec![
        music.join("01 One.flac"),
        music.join("02 Two.flac"),
        music.join("03 Three.mka"),
    ];
    for (i, file) in files.iter().enumerate() {
        let number = (i + 1).to_string();
        let title = ["One", "Two", "Three"][i];
        let codec = if i < 2 { "flac" } else { "pcm_s16le" };
        encode(
            file,
            codec,
            &[
                ("title", title),
                ("artist", "Ann"),
                ("album", "Old Name"),
                ("track", &number),
            ],
        );
    }
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
            "version = 1\n[library]\nfolders = [{:?}]\nwatch = false\n[output]\nbackend = \"null\"\n",
            dir.join("music")
        ),
    )
    .unwrap();
    let mut config = ServiceConfig::new(dirs);
    config.library = LibraryChoice::Open(MusicLibrary::open_existing(&database).unwrap());
    config.thumbnails = false;
    config.worker_program = Some(worker);
    let running = Service::start(config).unwrap();
    let backend = service::connect(
        running,
        service::ServiceOptions {
            mpris: false,
            inhibit_suspend: false,
        },
    )
    .unwrap();
    (backend, files)
}

/// The track keys by file name.
fn keys(backend: &Backend) -> Vec<(String, TrackKey)> {
    let library = backend.library();
    let keys = library.track_keys(&TrackQuery::default()).unwrap();
    let rows = library.track_rows(&keys).unwrap();
    let mut found: Vec<(String, TrackKey)> = rows
        .into_iter()
        .map(|r| {
            let name = r
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, r.key)
        })
        .collect();
    found.sort();
    found
}

fn album_entry(editor: &gtk::Window, text: &str) -> gtk::Entry {
    descendants(editor.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Entry>().ok())
        .find(|e| e.text() == text)
        .unwrap_or_else(|| panic!("no entry holding {text}"))
}

fn writes_tags_into_the_files(app: &gtk::Application) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ui-tag-editing");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (backend, files) = library(&dir);
    install_actions(app, &backend);
    let window = MainWindow::new(app, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(100);
    let keys = keys(&backend);
    assert_eq!(keys.len(), 3, "three tracks scanned: {keys:?}");
    let flac: Vec<TrackKey> = keys[..2].iter().map(|(_, k)| *k).collect();

    // Two FLAC files: the album is renamed in both, the titles stay.
    window.edit_tags(&flac);
    wait_for("the tag editor", || toplevel("tag-editor").is_some());
    let editor = toplevel("tag-editor").unwrap();
    album_entry(&editor, "Old Name").set_text("New Name");
    button(&editor, "Save 2 files").emit_clicked();
    wait_for("the save", || toplevel("tag-editor").is_none());
    wait_for("the toast", || {
        window
            .toast_text()
            .is_some_and(|t| t.contains("Saved 2 files"))
    });
    for (file, title) in files[..2].iter().zip(["One", "Two"]) {
        let read = read_file_info(file).unwrap();
        assert_eq!(read.tags.album.as_deref(), Some("New Name"), "{file:?}");
        assert_eq!(read.tags.title.as_deref(), Some(title), "{file:?}");
    }
    // The library read the files again at once.
    let services = backend.services();
    let reopened = services.edit_tracks(&flac).unwrap();
    for track in &reopened {
        let album = &track.fields[&helikodu_ui_gtk::backend::services::TagField::Album];
        assert_eq!(album, &vec!["New Name".to_owned()]);
        assert!(track.read_only.is_none());
    }
    let library = backend.library();
    let rows = library.track_rows(&flac).unwrap();
    assert!(
        rows.iter().all(|r| r.album_title == "New Name"),
        "the library has the new album: {rows:?}"
    );

    // With the Matroska file: its tags can only be read, so it keeps them and
    // its result says why; the FLAC files are written.
    let all: Vec<TrackKey> = keys.iter().map(|(_, k)| *k).collect();
    window.edit_tags(&all);
    wait_for("the tag editor", || toplevel("tag-editor").is_some());
    let editor = toplevel("tag-editor").unwrap();
    assert!(
        labels(&editor).iter().any(|l| l == "1 file keeps its tags"),
        "the notice names the read-only file"
    );
    let entries: Vec<gtk::Entry> = descendants(editor.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Entry>().ok())
        .filter(|e| e.placeholder_text().as_deref() == Some("<keep>"))
        .collect();
    assert!(!entries.is_empty(), "the albums differ");
    let artist = descendants(editor.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Entry>().ok())
        .find(|e| e.text() == "Ann")
        .expect("the shared artist");
    artist.set_text("Bo");
    button(&editor, "Save 3 files").emit_clicked();
    wait_for("the failure toast", || {
        window
            .toast_text()
            .is_some_and(|t| t.contains("1 of 3 files could not be saved"))
    });
    for file in &files[..2] {
        let read = read_file_info(file).unwrap();
        assert_eq!(read.tags.artists, vec!["Bo".to_owned()], "{file:?}");
    }
    let mka = read_file_info(&files[2]).unwrap();
    assert_eq!(mka.tags.artists, vec!["Ann".to_owned()]);
    editor.close();
    window.destroy();
}

type Test = (&'static str, fn(&gtk::Application));

const TESTS: &[Test] = &[("writes_tags_into_the_files", writes_tags_into_the_files)];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("tag_editing: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if !have_ffmpeg() {
        println!("SKIPPED: ffmpeg is not installed");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("tag_editing: skipped, no display");
        return ExitCode::SUCCESS;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.TagEditingTests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let failures = Rc::new(RefCell::new(Vec::new()));
    let f = failures.clone();
    application.connect_activate(move |application| {
        let _hold = application.hold();
        for (name, test) in TESTS {
            match catch_unwind(AssertUnwindSafe(|| test(application))) {
                Ok(()) => println!("test {name} ... ok"),
                Err(_) => {
                    println!("test {name} ... FAILED");
                    f.borrow_mut().push(*name);
                }
            }
            for window in gtk::Window::list_toplevels() {
                if let Ok(window) = window.downcast::<gtk::Window>() {
                    window.destroy();
                }
            }
            spin(20);
        }
        application.quit();
    });
    application.run_with_args(&["tag_editing"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("tag_editing: {} passed", TESTS.len());
        ExitCode::SUCCESS
    } else {
        println!("tag_editing: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
