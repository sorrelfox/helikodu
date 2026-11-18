// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialogs against the sample backend: settings that round-trip through
//! the store, the full-output confirmation (also for requests from other
//! programs), the tag editor with mixed values, the first run, the output
//! switcher, Supported formats, About and Keyboard shortcuts.
//!
//! Like `widgets.rs`, this program runs only under the headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test dialogs
//! ```

#![forbid(unsafe_code)]
// A panic is how a test fails; these are tests without the test harness.
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_model::settings::VolumeControl;
use helikodu_ui_gtk::backend::{AlbumQuery, Backend, PlayerRequest, TrackKey, Update, sample};
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
    let end = Instant::now() + Duration::from_secs(3);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        spin(10);
    }
}

fn options() -> sample::SampleOptions {
    sample::SampleOptions {
        albums: 40,
        playing: None,
    }
}

fn window_over(application: &gtk::Application, backend: Backend) -> (MainWindow, Backend) {
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(100);
    (window, backend)
}

fn window(application: &gtk::Application) -> (MainWindow, Backend) {
    window_over(application, sample::backend(&options()))
}

/// The open window called `name`.
fn toplevel(name: &str) -> Option<gtk::Window> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .find(|w| w.widget_name() == name && w.is_visible())
}

/// Every descendant of `root`.
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

fn named<T: IsA<gtk::Widget>>(root: &impl IsA<gtk::Widget>, name: &str) -> T {
    descendants(root.upcast_ref())
        .into_iter()
        .find(|w| w.widget_name() == name)
        .and_then(|w| w.downcast::<T>().ok())
        .unwrap_or_else(|| panic!("no widget {name}"))
}

/// The button whose label reads `text`.
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

fn first_album_tracks(backend: &Backend, n: usize) -> Vec<TrackKey> {
    let library = backend.library();
    let key = library.album_keys(&AlbumQuery::default()).unwrap()[0];
    let album = library.album(key).unwrap().unwrap();
    album.tracks().take(n).map(|t| t.key).collect()
}

fn settings_round_trip(app: &gtk::Application) {
    let (window, backend) = window(app);
    let services = backend.services();
    assert_eq!(services.settings().playback.seek_step_s, 5);
    let settings = window.open_settings("playback");
    wait_for("the playback page", || {
        window.settings_page().as_deref() == Some("playback")
    });
    let step: gtk::DropDown = named(&settings, "seek-step");
    // 2, 5, 10, 15 or 30 seconds.
    step.set_selected(2);
    assert_eq!(services.settings().playback.seek_step_s, 10);
    let awake: gtk::Switch = named(&settings, "keep-awake");
    assert!(awake.is_active());
    awake.set_active(false);
    assert!(!services.settings().playback.keep_awake);
    settings.close();
    spin(50);
    // Opened again, the controls show the stored values.
    let settings = window.open_settings("playback");
    spin(50);
    let step: gtk::DropDown = named(&settings, "seek-step");
    assert_eq!(step.selected(), 2);
    let awake: gtk::Switch = named(&settings, "keep-awake");
    assert!(!awake.is_active());
    // A value the store refuses changes nothing.
    let refused = services.update_settings(&mut |s| s.output.buffer_ms = 10);
    assert!(refused.is_err());
    assert_eq!(services.settings().output.buffer_ms, 200);
    for page in ["library", "output", "loudness", "appearance", "advanced"] {
        window.open_settings(page);
        wait_for(page, || window.settings_page().as_deref() == Some(page));
    }
    settings.close();
    window.destroy();
}

fn the_full_output_confirmation(app: &gtk::Application) {
    let (window, backend) = window(app);
    let services = backend.services();
    services
        .update_settings(&mut |s| s.output.volume = VolumeControl::Fixed)
        .unwrap();
    let album = backend
        .library()
        .album_keys(&AlbumQuery::default())
        .unwrap()[0];
    let play = PlayerRequest::PlayAlbum {
        album,
        start: 0,
        shuffle: false,
    };
    // Cancel: nothing plays and nothing is remembered.
    backend.send(play.clone());
    wait_for("the question", || toplevel("volume-confirm").is_some());
    let dialog = toplevel("volume-confirm").unwrap();
    spin(50);
    assert!(!window.is_playing(), "nothing plays before the answer");
    let cancel = button(&dialog, "Cancel");
    assert_eq!(
        gtk::prelude::RootExt::focus(&dialog),
        Some(cancel.clone().upcast()),
        "Cancel has the focus"
    );
    cancel.emit_clicked();
    spin(50);
    assert!(toplevel("volume-confirm").is_none());
    assert!(!window.is_playing());
    assert!(
        services
            .settings()
            .output
            .profiles
            .values()
            .all(|p| !p.fixed_volume_confirmed)
    );
    // Yes: playback starts and the answer is kept for the device.
    backend.send(play.clone());
    wait_for("the question again", || {
        toplevel("volume-confirm").is_some()
    });
    button(&toplevel("volume-confirm").unwrap(), "Play at full output").emit_clicked();
    wait_for("playback", || window.is_playing());
    assert!(
        services
            .settings()
            .output
            .profiles
            .values()
            .any(|p| p.fixed_volume_confirmed)
    );
    // Asked once for each device.
    backend.send(PlayerRequest::Stop);
    spin(30);
    backend.send(play);
    spin(50);
    assert!(toplevel("volume-confirm").is_none());
    assert!(window.is_playing());
    window.destroy();
}

fn outside_requests_pass_the_gate(app: &gtk::Application) {
    let (backend, updates) = sample::backend_with_updates(&sample::SampleOptions {
        albums: 40,
        playing: Some((0, 0, 10)),
    });
    let (window, backend) = window_over(app, backend);
    wait_for("playback", || window.is_playing());
    backend.send(PlayerRequest::PlayPause);
    wait_for("the pause", || !window.is_playing());
    backend
        .services()
        .update_settings(&mut |s| s.output.volume = VolumeControl::Fixed)
        .unwrap();
    // Another program (MPRIS) asks to play: the window asks first.
    updates
        .try_send(Update::Request(PlayerRequest::Play))
        .unwrap();
    wait_for("the question", || toplevel("volume-confirm").is_some());
    spin(50);
    assert!(!window.is_playing(), "nothing plays before the answer");
    button(&toplevel("volume-confirm").unwrap(), "Cancel").emit_clicked();
    spin(50);
    assert!(!window.is_playing());
    // The same for PlayPause.
    updates
        .try_send(Update::Request(PlayerRequest::PlayPause))
        .unwrap();
    wait_for("the question again", || {
        toplevel("volume-confirm").is_some()
    });
    button(&toplevel("volume-confirm").unwrap(), "Play at full output").emit_clicked();
    wait_for("playback", || window.is_playing());
    window.destroy();
}

fn the_tag_editor_shows_mixed_values(app: &gtk::Application) {
    let (window, backend) = window(app);
    let tracks = first_album_tracks(&backend, 3);
    let services = backend.services();
    let before = services.edit_tracks(&tracks).unwrap();
    let album = before[0].fields[&helikodu_ui_gtk::backend::services::TagField::Album][0].clone();
    window.edit_tags(&tracks);
    wait_for("the tag editor", || toplevel("tag-editor").is_some());
    let editor = toplevel("tag-editor").unwrap();
    let entries: Vec<gtk::Entry> = descendants(editor.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Entry>().ok())
        .collect();
    let keep = entries
        .iter()
        .filter(|e| e.placeholder_text().as_deref() == Some("<keep>"))
        .count();
    assert!(keep >= 2, "titles and track numbers differ ({keep} kept)");
    let album_entry = entries
        .iter()
        .find(|e| e.text() == album)
        .expect("the shared album");
    let save = button(&editor, "Save 3 files");
    assert!(!save.is_sensitive(), "nothing to save yet");
    album_entry.set_text("Renamed Album");
    assert!(album_entry.has_css_class("edited"));
    assert!(save.is_sensitive());
    save.emit_clicked();
    wait_for("the save", || toplevel("tag-editor").is_none());
    let after = services.edit_tracks(&tracks).unwrap();
    for (old, new) in before.iter().zip(&after) {
        let field = helikodu_ui_gtk::backend::services::TagField::Album;
        assert_eq!(new.fields[&field], vec!["Renamed Album".to_owned()]);
        let title = helikodu_ui_gtk::backend::services::TagField::Title;
        assert_eq!(
            new.fields[&title], old.fields[&title],
            "untouched fields keep each file's value"
        );
    }
    wait_for("the toast", || {
        window
            .toast_text()
            .is_some_and(|t| t.contains("Saved 3 files"))
    });
    window.destroy();
}

fn the_first_run(app: &gtk::Application) {
    let (window, backend) = window_over(app, sample::first_run_backend(&options()));
    wait_for("the first run", || window.first_run_shown());
    let start = button(&window, "Scan and start");
    start.emit_clicked();
    spin(50);
    assert!(!window.first_run_shown());
    let settings = backend.services().settings();
    assert!(settings.library.watch);
    // A scan shows its progress in the sidebar and hides it at the end.
    window.show_scan_status(&helikodu_ui_gtk::backend::services::ScanStatus {
        done: 12,
        total: Some(100),
        ..Default::default()
    });
    assert!(window.scan_footer_shown());
    window.show_scan_status(&helikodu_ui_gtk::backend::services::ScanStatus {
        finished: true,
        ..Default::default()
    });
    assert!(!window.scan_footer_shown());
    window.destroy();
    // With a folder, the library opens at once.
    let (window, _) = window_over(app, sample::backend(&options()));
    spin(100);
    assert!(!window.first_run_shown());
    window.destroy();
}

fn the_output_switcher(app: &gtk::Application) {
    let (window, backend) = window(app);
    let popover = window.open_output_switcher().unwrap();
    wait_for("the devices", || {
        descendants(popover.upcast_ref()).iter().any(|w| {
            w.downcast_ref::<gtk::Label>()
                .is_some_and(|l| l.text() == "Studio Interface")
        })
    });
    button(&popover, "Studio Interface").emit_clicked();
    spin(50);
    let output = backend.services().output();
    assert_eq!(
        format!("{:?}", output.device),
        format!(
            "{:?}",
            helikodu_ui_gtk::backend::services::DeviceSelection::Device(
                helikodu_ui_gtk::backend::services::DeviceId::new(
                    "alsa_output.usb-Studio_Interface-00.analog-stereo"
                )
                .unwrap()
            )
        )
    );
    window.destroy();
}

fn formats_and_about(app: &gtk::Application) {
    let (window, _) = window(app);
    gio::prelude::ActionGroupExt::activate_action(app, "formats", None);
    wait_for("Supported formats", || toplevel("formats").is_some());
    gio::prelude::ActionGroupExt::activate_action(app, "about", None);
    wait_for("About", || toplevel("about").is_some());
    toplevel("about").unwrap().close();
    toplevel("formats").unwrap().close();
    window.destroy();
}

fn keyboard_shortcuts(app: &gtk::Application) {
    let (window, _) = window(app);
    assert_eq!(
        app.accels_for_action("app.shortcuts")
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["<Control>question"]
    );
    gio::prelude::ActionGroupExt::activate_action(app, "shortcuts", None);
    wait_for("Keyboard shortcuts", || toplevel("shortcuts").is_some());
    let dialog = toplevel("shortcuts").unwrap();
    let texts = |root: &gtk::Window| -> Vec<String> {
        descendants(root.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Label>().ok())
            .filter(|l| l.is_mapped())
            .map(|l| l.text().to_string())
            .collect()
    };
    spin(50);
    let shown = texts(&dialog);
    for expected in [
        "Next track",
        "Search",
        "Sidebar",
        "Ctrl",
        "→",
        "F9",
        "Not set",
    ] {
        assert!(
            shown.iter().any(|t| t == expected),
            "{expected} is shown: {shown:?}"
        );
    }
    let filter = descendants(dialog.upcast_ref())
        .into_iter()
        .find_map(|w| w.downcast::<gtk::SearchEntry>().ok())
        .unwrap();
    filter.set_text("f9");
    spin(400);
    let shown = texts(&dialog);
    assert!(shown.iter().any(|t| t == "Sidebar"));
    assert!(!shown.iter().any(|t| t == "Next track"), "{shown:?}");
    dialog.close();
    window.destroy();
}

type Test = (&'static str, fn(&gtk::Application));

const TESTS: &[Test] = &[
    ("settings_round_trip", settings_round_trip),
    ("the_full_output_confirmation", the_full_output_confirmation),
    (
        "outside_requests_pass_the_gate",
        outside_requests_pass_the_gate,
    ),
    (
        "the_tag_editor_shows_mixed_values",
        the_tag_editor_shows_mixed_values,
    ),
    ("the_first_run", the_first_run),
    ("the_output_switcher", the_output_switcher),
    ("formats_and_about", formats_and_about),
    ("keyboard_shortcuts", keyboard_shortcuts),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("dialogs: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("dialogs: skipped, no display");
        return ExitCode::SUCCESS;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.DialogTests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let failures = Rc::new(RefCell::new(Vec::new()));
    let f = failures.clone();
    application.connect_activate(move |application| {
        let _hold = application.hold();
        for (name, test) in TESTS {
            let result = catch_unwind(AssertUnwindSafe(|| test(application)));
            match result {
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
    application.run_with_args(&["dialogs"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("dialogs: {} passed", TESTS.len());
        ExitCode::SUCCESS
    } else {
        println!("dialogs: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
