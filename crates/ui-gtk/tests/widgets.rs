// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The main window, its views and the theme against the sample backend.
//!
//! GTK needs a display and runs on the main thread only, so this test program
//! has its own main function and runs only when `HELIKODU_GTK_TESTS` is set,
//! as `tests/headless.sh` does with a headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test widgets
//! ```
//!
//! Without it, the program says it skipped and succeeds, so that it never
//! opens windows on the developer's desktop.

#![forbid(unsafe_code)]
// A panic is how a test fails; these are tests without the test harness.
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_ui_gtk::backend::{AlbumKey, AlbumQuery, PlayerRequest, sample};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice, set_choice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display, snapshot};

/// Runs the main loop for `ms` milliseconds.
fn spin(ms: u64) {
    let context = glib::MainContext::default();
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs the main loop until `done` holds, for at most two seconds.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(2);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        spin(10);
    }
}

fn window(
    application: &gtk::Application,
    style: WindowStyle,
) -> (MainWindow, helikodu_ui_gtk::backend::Backend) {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 300,
        playing: None,
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, style);
    window.set_default_size(1440, 900);
    window.present();
    spin(100);
    (window, backend)
}

fn first_album(backend: &helikodu_ui_gtk::backend::Backend) -> AlbumKey {
    backend
        .library()
        .album_keys(&AlbumQuery::default())
        .unwrap()[0]
}

fn the_album_grid_loads_lazily(app: &gtk::Application) {
    let (window, _) = window(app, WindowStyle::Integrated);
    assert_eq!(window.visible_view().as_deref(), Some("albums"));
    wait_for("the album keys", || window.album_grid_stats().0 == 300);
    wait_for("the first rows", || window.album_grid_stats().1 > 0);
    let (_, fetches) = window.album_grid_stats();
    assert!(
        fetches <= 3,
        "rows are fetched in batches, not one by one ({fetches} fetches)"
    );
    window.destroy();
}

fn the_album_page_and_the_history(app: &gtk::Application) {
    let (window, backend) = window(app, WindowStyle::Integrated);
    window.open_album(first_album(&backend));
    spin(50);
    assert_eq!(window.visible_view().as_deref(), Some("album"));
    window.go_back();
    assert_eq!(window.visible_view().as_deref(), Some("albums"));
    window.destroy();
}

fn the_panes_toggle(app: &gtk::Application) {
    let (window, _) = window(app, WindowStyle::Classic);
    assert_eq!(window.panes_shown(), (true, true));
    WidgetExt::activate_action(&window, "win.toggle-sidebar", None).unwrap();
    WidgetExt::activate_action(&window, "win.toggle-panel", None).unwrap();
    assert_eq!(window.panes_shown(), (false, false));
    WidgetExt::activate_action(&window, "win.toggle-sidebar", None).unwrap();
    assert_eq!(window.panes_shown(), (true, false));
    window.destroy();
}

fn the_scheme_follows_the_choice(app: &gtk::Application) {
    let (window, _) = window(app, WindowStyle::Integrated);
    set_choice(SchemeChoice::Light, AccentChoice::Blue);
    wait_for("the light scheme", || window.has_css_class("hk-light"));
    assert!(!window.has_css_class("hk-dark"));
    set_choice(SchemeChoice::Dark, AccentChoice::Amber);
    wait_for("the dark scheme", || window.has_css_class("hk-dark"));
    window.destroy();
}

fn playing_and_toasts(app: &gtk::Application) {
    let (window, backend) = window(app, WindowStyle::Integrated);
    backend.send(PlayerRequest::PlayAlbum {
        album: first_album(&backend),
        start: 1,
        shuffle: false,
    });
    spin(50);
    app.activate_action("next", None);
    app.activate_action("play-pause", None);
    spin(50);
    window.toast("Something happened", false);
    assert_eq!(window.toast_text().as_deref(), Some("Something happened"));
    window.destroy();
}

fn the_panes_are_remembered(app: &gtk::Application) {
    let (window, backend) = window(app, WindowStyle::Integrated);
    WidgetExt::activate_action(&window, "win.toggle-panel", None).unwrap();
    let state = backend.ui_state().load();
    assert!(!state.panes.queue_visible);
    assert!(state.panes.sidebar_visible);
    window.destroy();
}

fn play_modes_and_the_queue(app: &gtk::Application) {
    let (window, backend) = window(app, WindowStyle::Integrated);
    backend.send(PlayerRequest::PlayAlbum {
        album: first_album(&backend),
        start: 0,
        shuffle: false,
    });
    spin(50);
    app.activate_action("stop-after", None);
    spin(50);
    let state = app
        .lookup_action("stop-after")
        .and_then(|a| a.state())
        .and_then(|s| s.get::<bool>());
    assert_eq!(
        state,
        Some(true),
        "the stop-after action follows the player"
    );
    app.activate_action("clear-queue", None);
    spin(50);
    window.destroy();
}

fn the_window_renders_to_png(app: &gtk::Application) {
    let (window, _) = window(app, WindowStyle::Integrated);
    spin(300);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join("widgets-main.png");
    snapshot::save_png(&window, &path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 10_000);
    window.destroy();
}

type Test = (&'static str, fn(&gtk::Application));

const TESTS: &[Test] = &[
    ("the_album_grid_loads_lazily", the_album_grid_loads_lazily),
    (
        "the_album_page_and_the_history",
        the_album_page_and_the_history,
    ),
    ("the_panes_toggle", the_panes_toggle),
    (
        "the_scheme_follows_the_choice",
        the_scheme_follows_the_choice,
    ),
    ("playing_and_toasts", playing_and_toasts),
    ("the_panes_are_remembered", the_panes_are_remembered),
    ("play_modes_and_the_queue", play_modes_and_the_queue),
    ("the_window_renders_to_png", the_window_renders_to_png),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("widgets: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("widgets: skipped, no display");
        return ExitCode::SUCCESS;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.Tests")
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
        }
        application.quit();
    });
    application.run_with_args(&["widgets"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("widgets: {} passed", TESTS.len());
        ExitCode::SUCCESS
    } else {
        println!("widgets: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
