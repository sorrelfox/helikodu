// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Convert, Sync and export and the cover manager against the sample backend:
//! opened from the Tools menu, a conversion that runs and reports each track, a
//! pattern that cannot be read, a sync plan with its figures, the cover groups
//! with their counts and ticking, and no frames drawn while idle.
//!
//! Like `dialogs.rs`, this program runs only under the headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test tools
//! ```

#![forbid(unsafe_code)]
// A panic is how a test fails; these are tests without the test harness.
#![allow(clippy::unwrap_used)]

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_ui_gtk::backend::sample;
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
    let end = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        spin(10);
    }
}

fn window(application: &gtk::Application) -> MainWindow {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 12,
        playing: None,
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(100);
    window
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

fn named<T: IsA<gtk::Widget>>(root: &impl IsA<gtk::Widget>, name: &str) -> T {
    descendants(root.upcast_ref())
        .into_iter()
        .find(|w| w.widget_name() == name)
        .and_then(|w| w.downcast::<T>().ok())
        .unwrap_or_else(|| panic!("no widget {name}"))
}

fn texts(root: &impl IsA<gtk::Widget>) -> String {
    descendants(root.upcast_ref())
        .iter()
        .filter(|w| w.is_mapped())
        .filter_map(|w| w.downcast_ref::<gtk::Label>().map(|l| l.text().to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn open_tool(app: &gtk::Application, action: &str, name: &str) -> gtk::Window {
    app.activate_action(action, None);
    wait_for(name, || toplevel(name).is_some());
    toplevel(name).unwrap()
}

/// No frame is drawn while nothing changes.
fn assert_idle(window: &gtk::Window) {
    spin(600);
    let frames = Rc::new(Cell::new(0u32));
    let clock = window.frame_clock().expect("a frame clock");
    let f = frames.clone();
    let id = clock.connect_after_paint(move |_| f.set(f.get() + 1));
    spin(1200);
    clock.disconnect(id);
    assert_eq!(frames.get(), 0, "frames drawn while idle");
}

/// The converter plans every track, converts them in the background and reports
/// each; a pattern that cannot be read stops it with a note.
fn the_converter_converts(app: &gtk::Application) {
    let main = window(app);
    let tool = open_tool(app, "convert", "converter");
    let view: gtk::ListView = named(&tool, "convert-table");
    wait_for("the plan", || view.model().is_some_and(|m| m.n_items() > 0));
    let convert: gtk::Button = named(&tool, "convert");
    assert!(convert.is_sensitive());
    assert!(convert.label().unwrap().starts_with("Convert "));

    let pattern: gtk::Entry = named(&tool, "pattern");
    pattern.set_text("{albumartist}/{nope}");
    spin(100);
    assert!(
        texts(&tool).contains("{nope} is not a field"),
        "{}",
        texts(&tool)
    );
    assert_eq!(view.model().unwrap().n_items(), 0);
    pattern.set_text("{albumartist}/{album}/{track} {title}");
    wait_for("the plan again", || {
        view.model().is_some_and(|m| m.n_items() > 0)
    });

    // Another format changes the To column.
    let format: gtk::DropDown = named(&tool, "format");
    format.set_selected(4);
    spin(200);
    assert!(texts(&tool).contains("FLAC"));

    assert_idle(&tool);
    convert.emit_clicked();
    let open: gtk::Button = named(&tool, "open-folder");
    wait_for("the end", || open.is_visible());
    spin(100);
    let shown = texts(&tool);
    assert!(shown.contains("files written"), "{shown}");
    assert!(shown.contains('✓'), "each track reports its size: {shown}");
    // Done goes back to the plan.
    convert.emit_clicked();
    spin(200);
    assert!(convert.label().unwrap().starts_with("Convert "));
    tool.close();
    main.destroy();
}

/// Sync and export shows the destinations, the plan's five figures and its
/// rows, and the choices of what is copied.
fn sync_shows_the_plan(app: &gtk::Application) {
    let main = window(app);
    let tool = open_tool(app, "sync-export", "sync-export");
    wait_for("the plan", || texts(&tool).contains("201"));
    let shown = texts(&tool);
    for text in [
        "Phone",
        "Car stick",
        "to convert",
        "already there",
        "Remove",
    ] {
        assert!(shown.contains(text), "{text} in {shown}");
    }
    let files: gtk::DropDown = named(&tool, "sync-files");
    assert!(files.model().unwrap().n_items() >= 15, "presets in Files");
    assert_idle(&tool);
    tool.close();
    main.destroy();
}

/// The cover manager counts each group and ticks albums for its action.
fn covers_group_and_tick(app: &gtk::Application) {
    let main = window(app);
    let tool = open_tool(app, "cover-manager", "cover-manager");
    let view: gtk::ListView = named(&tool, "cover-table");
    wait_for("the albums", || {
        view.model().is_some_and(|m| m.n_items() > 0)
    });
    let primary: gtk::Button = named(&tool, "use-pictures");
    assert!(!primary.is_sensitive(), "nothing ticked yet");
    let checks: Vec<gtk::CheckButton> = descendants(view.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::CheckButton>().ok())
        .filter(|c| c.widget_name().starts_with("album-"))
        .collect();
    assert!(!checks.is_empty());
    for check in &checks {
        check.set_active(true);
    }
    spin(50);
    assert!(texts(&tool).contains("albums selected") || texts(&tool).contains("album selected"));
    assert!(primary.is_sensitive(), "a picture was found for some");
    assert_idle(&tool);
    tool.close();
    main.destroy();
}

type Test = (&'static str, fn(&gtk::Application));

const TESTS: &[Test] = &[
    ("the_converter_converts", the_converter_converts),
    ("sync_shows_the_plan", sync_shows_the_plan),
    ("covers_group_and_tick", covers_group_and_tick),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("tools: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("tools: skipped, no display");
        return ExitCode::SUCCESS;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.ToolTests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let failures = Rc::new(RefCell::new(Vec::new()));
    let f = failures.clone();
    application.connect_activate(move |application| {
        let _hold = application.hold();
        let filters: Vec<String> = std::env::args()
            .skip(1)
            .filter(|a| !a.starts_with('-'))
            .collect();
        for (name, test) in TESTS {
            if !filters.is_empty() && !filters.iter().any(|f| name.contains(f.as_str())) {
                continue;
            }
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
    application.run_with_args(&["tools"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("tools: {} passed", TESTS.len());
        ExitCode::SUCCESS
    } else {
        println!("tools: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
