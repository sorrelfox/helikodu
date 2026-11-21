// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The queue panel while endless play or the Auto-DJ is on, against the
//! sample backend: the "Added by Helikodu" section and the spark on the rows
//! Helikodu added, the block after the last row with what comes next and its
//! "Change…" and "Turn off", the Auto-DJ's block (also with an empty queue),
//! nothing left to add, and its toast shown once while the queue is not in
//! view. `HELIKODU_QUEUE_SHOTS=DIR` saves the three states, dark and light, to
//! `DIR`.
//!
//! GTK needs a display and the main thread, so this program has its own main
//! function and runs only under the headless compositor script:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test queue
//! ```
//!
//! Arguments that do not start with `-` select tests by name.

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
use helikodu_ui_gtk::backend::{Backend, PlayerRequest, QueueSnapshot, RepeatMode, Update, sample};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice, set_choice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display, snapshot};

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

/// The texts of the labels under `root`, in order.
fn texts(root: &impl IsA<gtk::Widget>) -> Vec<String> {
    let mut found = Vec::new();
    fn walk(widget: &gtk::Widget, found: &mut Vec<String>) {
        if let Some(label) = widget.downcast_ref::<gtk::Label>() {
            found.push(label.text().to_string());
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            walk(&c, found);
            child = c.next_sibling();
        }
    }
    walk(root.upcast_ref(), &mut found);
    found
}

/// The button labelled `text` under `root`.
fn button(root: &impl IsA<gtk::Widget>, text: &str) -> gtk::Button {
    descendants(root.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.label().as_deref() == Some(text))
        .unwrap_or_else(|| panic!("no button {text}"))
}

/// The rows of the queue that carry the spark.
fn sparks(window: &MainWindow) -> usize {
    descendants(window.queue_list().upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Image>().ok())
        .filter(|i| i.icon_name().as_deref() == Some("helikodu-spark-symbolic"))
        .count()
}

/// The block after the last row, if it is shown.
fn end_block(window: &MainWindow) -> Option<gtk::Widget> {
    descendants(window.queue_list().upcast_ref())
        .into_iter()
        .find(|w| w.widget_name() == "queue-end")
}

fn end_texts(window: &MainWindow) -> Vec<String> {
    end_block(window).map(|b| texts(&b)).unwrap_or_default()
}

fn toplevel(name: &str) -> Option<gtk::Window> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .find(|w| w.widget_name() == name && w.is_visible())
}

/// The window over the sample backend, playing the first album's third
/// track; with the sender of its updates.
fn playing_window(
    application: &gtk::Application,
) -> (MainWindow, Backend, async_channel::Sender<Update>) {
    let (backend, updates) = sample::backend_with_updates(&sample::SampleOptions {
        albums: 40,
        playing: Some((0, 2, 102)),
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(200);
    (window, backend, updates)
}

/// Plays the last entry of the queue, so that nothing is left after it.
fn play_last(window: &MainWindow, backend: &Backend) {
    let last = descendants(window.queue_list().upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::ListBoxRow>().ok())
        .filter_map(|r| r.widget_name().parse::<u64>().ok())
        .max()
        .unwrap();
    backend.send(PlayerRequest::PlayEntry(last));
    spin(50);
}

fn related_tracks_are_added_and_marked(app: &gtk::Application) {
    let (window, backend, _) = playing_window(app);
    play_last(&window, &backend);
    assert_eq!(sparks(&window), 0);
    backend.send(PlayerRequest::SetRepeatMode(RepeatMode::Endless));
    wait_for("the added tracks", || sparks(&window) == 5);
    let list = texts(&window.queue_list());
    assert!(list.iter().any(|t| t == "Added by Helikodu"), "{list:?}");
    let end = end_texts(&window);
    assert!(
        end[0].starts_with("Then: tracks related to “"),
        "the block names the last track: {end:?}"
    );
    assert!(
        end[1].ends_with(" · 5 at a time, by artist and genre")
            || end[1].ends_with(" · 10 at a time, by artist and genre"),
        "{end:?}"
    );
    // The spark is described for screen readers.
    let spark = descendants(window.queue_list().upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Image>().ok())
        .find(|i| i.icon_name().as_deref() == Some("helikodu-spark-symbolic"))
        .unwrap();
    assert_eq!(spark.tooltip_text().as_deref(), Some("Added by Helikodu"));
    // Turn off sets Repeat to Off; the added tracks stay.
    let block = end_block(&window).unwrap();
    let off = button(&block, "Turn off");
    assert_eq!(off.tooltip_text().as_deref(), Some("Sets Repeat to Off"));
    off.emit_clicked();
    wait_for("the block to go", || end_block(&window).is_none());
    assert_eq!(sparks(&window), 5);
    // Change… opens Settings › Playback.
    backend.send(PlayerRequest::SetRepeatMode(RepeatMode::Endless));
    wait_for("the block", || end_block(&window).is_some());
    button(&end_block(&window).unwrap(), "Change…").emit_clicked();
    wait_for("the settings", || toplevel("settings").is_some());
    wait_for("the playback page", || {
        window.settings_page().as_deref() == Some("playback")
    });
    toplevel("settings").unwrap().close();
    window.destroy();
    spin(50);
}

fn the_auto_dj_states(app: &gtk::Application) {
    let (window, backend, updates) = playing_window(app);
    backend.send(PlayerRequest::SetRepeatMode(RepeatMode::AutoDj));
    wait_for("the Auto-DJ block", || {
        end_texts(&window).first().map(String::as_str) == Some("Auto-DJ keeps 5 tracks ahead")
    });
    assert_eq!(end_texts(&window)[1], "From the whole library");
    // With an empty queue, the list holds only the block.
    updates
        .try_send(Update::Queue(QueueSnapshot::default()))
        .unwrap();
    wait_for("the waiting block", || {
        end_texts(&window).first().map(String::as_str)
            == Some("Auto-DJ is on. Press Play and it fills the queue.")
    });
    // A filter that no more tracks match.
    backend
        .services()
        .update_settings(&mut |s| s.playback.auto_dj.filter = "genre:Tango".into())
        .unwrap();
    let nothing = |on: bool| QueueSnapshot {
        nothing_to_add: on,
        ..QueueSnapshot::default()
    };
    updates.try_send(Update::Queue(nothing(true))).unwrap();
    wait_for("nothing to add", || {
        end_texts(&window).first().map(String::as_str) == Some("Nothing more to add")
    });
    assert_eq!(
        end_texts(&window)[1],
        "No track matches the Auto-DJ filter, genre:Tango. The music stops after the last track."
    );
    // The queue was in view: no toast.
    assert!(
        !window
            .toast_text()
            .is_some_and(|t| t.starts_with("Nothing more to add"))
    );
    // With the panel hidden, the toast shows once.
    updates.try_send(Update::Queue(nothing(false))).unwrap();
    spin(50);
    WidgetExt::activate_action(&window, "win.toggle-panel", None).unwrap();
    spin(400);
    updates.try_send(Update::Queue(nothing(true))).unwrap();
    wait_for("the toast", || {
        window.toast_text().as_deref()
            == Some("Nothing more to add: no track matches the Auto-DJ filter")
    });
    window.destroy();
    spin(50);
}

/// Saves the three states of the queue panel, dark and light.
fn queue_screenshots(app: &gtk::Application) {
    let Some(folder) = std::env::var_os("HELIKODU_QUEUE_SHOTS").map(std::path::PathBuf::from)
    else {
        println!("queue_screenshots: skipped; set HELIKODU_QUEUE_SHOTS");
        return;
    };
    std::fs::create_dir_all(&folder).unwrap();
    for (scheme, suffix) in [(SchemeChoice::Dark, ""), (SchemeChoice::Light, ".light")] {
        set_choice(scheme, AccentChoice::Amber);
        let (window, backend, updates) = playing_window(app);
        let save = |name: &str| {
            spin(300);
            let file = folder.join(format!("{name}{suffix}.png"));
            snapshot::save_png(&window, &file).unwrap();
            println!("wrote {}", file.display());
        };
        play_last(&window, &backend);
        backend.send(PlayerRequest::SetRepeatMode(RepeatMode::Endless));
        wait_for("the added tracks", || sparks(&window) == 5);
        let list = window.queue_list();
        if let Some(scroller) = list.ancestor(gtk::ScrolledWindow::static_type()) {
            let adjustment = scroller
                .downcast::<gtk::ScrolledWindow>()
                .unwrap()
                .vadjustment();
            spin(100);
            adjustment.set_value(adjustment.upper());
        }
        save("QueueEndless");
        backend.send(PlayerRequest::SetRepeatMode(RepeatMode::AutoDj));
        spin(100);
        save("QueueAutoDj");
        backend
            .services()
            .update_settings(&mut |s| s.playback.auto_dj.filter = "genre:Tango".into())
            .unwrap();
        updates
            .try_send(Update::Queue(QueueSnapshot {
                nothing_to_add: true,
                ..QueueSnapshot::default()
            }))
            .unwrap();
        save("QueueNothingToAdd");
        window.destroy();
        spin(50);
    }
    set_choice(SchemeChoice::Dark, AccentChoice::Amber);
}

type Test = (&'static str, fn(&gtk::Application));

const TESTS: &[Test] = &[
    (
        "related_tracks_are_added_and_marked",
        related_tracks_are_added_and_marked,
    ),
    ("the_auto_dj_states", the_auto_dj_states),
    ("queue_screenshots", queue_screenshots),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("queue: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("queue: skipped, no display");
        return ExitCode::SUCCESS;
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let chosen = |name: &str| filter.is_empty() || filter.iter().any(|f| name.contains(f.as_str()));
    let names: Vec<&'static str> = TESTS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| chosen(name))
        .collect();
    let selected = names.len();
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.QueueTests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let failures = Rc::new(RefCell::new(Vec::new()));
    let f = failures.clone();
    application.connect_activate(move |application| {
        let _hold = application.hold();
        for (name, test) in TESTS {
            if !names.contains(name) {
                continue;
            }
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
    application.run_with_args(&["queue"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("queue: {selected} passed");
        ExitCode::SUCCESS
    } else {
        println!("queue: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
