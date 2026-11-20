// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The managed folder against the sample backend: its group in Settings ›
//! Library off and on (a folder outside the library becoming a library folder,
//! a pattern that cannot be read, turning it off, the menu items that follow
//! it), the plan of adding music with its figures, the plan that does not fit,
//! a run with its end and "Show in the library", gathering the library at work
//! with Stop, and the dialog of a run that was cut short going on or rolling
//! back.
//!
//! Like `tools.rs`, this program runs only under the headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test managed
//! ```

#![forbid(unsafe_code)]
// A panic is how a test fails; these are tests without the test harness.
#![allow(clippy::unwrap_used)]

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_ui_gtk::backend::{Backend, sample};
use helikodu_ui_gtk::chooser;
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice};
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
    let end = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        spin(10);
    }
}

fn window(application: &gtk::Application) -> (MainWindow, Backend) {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 12,
        playing: None,
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(100);
    (window, backend)
}

fn music() -> PathBuf {
    glib::home_dir().join("Music")
}

/// Turns the managed folder on at `folder`, as Settings does.
fn turn_on(application: &gtk::Application, backend: &Backend, folder: PathBuf) {
    backend.services().set_managed_folder(&folder).unwrap();
    // The settings group turns the actions on; here the test does.
    for name in ["managed-add", "managed-gather"] {
        application
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
            .unwrap()
            .set_enabled(true);
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

fn find<T: IsA<gtk::Widget>>(root: &impl IsA<gtk::Widget>, name: &str) -> Option<T> {
    descendants(root.upcast_ref())
        .into_iter()
        .find(|w| w.widget_name() == name)
        .and_then(|w| w.downcast::<T>().ok())
}

fn named<T: IsA<gtk::Widget>>(root: &impl IsA<gtk::Widget>, name: &str) -> T {
    find(root, name).unwrap_or_else(|| panic!("no widget {name}"))
}

fn texts(root: &impl IsA<gtk::Widget>) -> String {
    descendants(root.upcast_ref())
        .iter()
        .filter(|w| w.is_mapped())
        .filter_map(|w| w.downcast_ref::<gtk::Label>().map(|l| l.text().to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn toplevel(name: &str) -> Option<gtk::Window> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .find(|w| w.widget_name() == name && w.is_visible())
}

/// Saves `window` to `HELIKODU_MANAGED_SHOTS/name.png` when that is set.
fn shot(window: &impl IsA<gtk::Window>, name: &str) {
    let Some(dir) = std::env::var_os("HELIKODU_MANAGED_SHOTS") else {
        return;
    };
    spin(300);
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    if let Err(error) = snapshot::save_png(window, &path) {
        println!("{name}: {error:?}");
    }
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

fn action_enabled(application: &gtk::Application, name: &str) -> bool {
    application
        .lookup_action(name)
        .is_some_and(|a| a.is_enabled())
}

/// Answers the folder chooser with `answer`.
fn answer_chooser(answer: PathBuf) {
    chooser::replace_folder_chooser(Some(Rc::new(
        move |_title: &str, reply: chooser::FolderReply| {
            let answer = answer.clone();
            glib::idle_add_local_once(move || reply(Some(answer)));
        },
    )));
}

/// The group of Settings › Library: off, then on at a folder outside the
/// library (which becomes a library folder), a pattern that cannot be read,
/// and off again; the menu items follow it.
fn the_settings_group_turns_on_and_off(app: &gtk::Application) {
    let (main, backend) = window(app);
    assert!(!action_enabled(app, "managed-add"));
    assert!(!action_enabled(app, "managed-gather"));
    let settings = main.open_settings("library");
    spin(200);
    let choose: gtk::Button = named(&settings, "managed-choose");
    assert!(texts(&settings).contains("Let Helikodu keep one folder in order"));
    shot(&settings, "SettingsLibrary");

    let outside = PathBuf::from("/srv/Managed");
    answer_chooser(outside.clone());
    choose.emit_clicked();
    let result = catch_unwind(AssertUnwindSafe(|| {
        wait_for("the group on", || {
            find::<gtk::Box>(&settings, "managed-on").is_some()
        });
        spin(200);
        let shown = texts(&settings);
        assert!(shown.contains("/srv/Managed"), "{shown}");
        assert!(
            shown.contains("This folder was added to the library's folders."),
            "{shown}"
        );
        assert!(
            shown.contains("Mari Kask/Tidewater Rooms/01 Tidewater.flac"),
            "{shown}"
        );
        assert!(
            backend
                .services()
                .settings()
                .library
                .folders
                .contains(&outside)
        );
        assert!(action_enabled(app, "managed-add"));
        assert!(action_enabled(app, "managed-gather"));
        // The sample's music folder is outside the managed folder.
        wait_for("the files outside", || {
            texts(&settings).contains("1,120 library files are outside this folder")
        });
        assert!(named::<gtk::Button>(&settings, "managed-gather").is_sensitive());
        shot(&settings, "SettingsLibraryManaged");

        // A pattern that cannot be read is reported; the stored one stays.
        let pattern: gtk::Entry = named(&settings, "managed-pattern");
        let stored = backend
            .services()
            .settings()
            .library
            .managed
            .pattern
            .clone();
        pattern.set_text("{albumartist}/{nope}");
        spin(500);
        let problem: gtk::Label = named(&settings, "managed-pattern-problem");
        assert!(problem.is_visible());
        assert!(
            problem.text().contains("There is no field called nope"),
            "{}",
            problem.text()
        );
        assert_eq!(
            backend.services().settings().library.managed.pattern,
            stored
        );
        pattern.set_text("{artist}/{title}");
        spin(500);
        assert!(!problem.is_visible());
        assert_eq!(
            backend.services().settings().library.managed.pattern,
            "{artist}/{title}"
        );
        let example: gtk::Label = named(&settings, "managed-example");
        assert_eq!(example.text(), "Mari Kask/Tidewater.flac");
        assert_idle(&settings);

        // Turning it off asks nothing.
        let off: gtk::Button = named(&settings, "managed-turn-off");
        off.emit_clicked();
        wait_for("the group off", || {
            find::<gtk::Button>(&settings, "managed-choose").is_some()
        });
        assert!(!backend.services().settings().library.managed.is_on());
        assert!(!action_enabled(app, "managed-add"));

        // A folder in the library stays as it is: no line about it.
        answer_chooser(music().join("Library"));
        named::<gtk::Button>(&settings, "managed-choose").emit_clicked();
        wait_for("the group on again", || {
            find::<gtk::Box>(&settings, "managed-on").is_some()
        });
        assert!(find::<gtk::Label>(&settings, "managed-added").is_none());
    }));
    chooser::replace_folder_chooser(None);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    settings.close();
    main.destroy();
}

fn sources() -> Vec<PathBuf> {
    let home = glib::home_dir();
    vec![
        home.join("Downloads/bandcamp"),
        PathBuf::from("/run/media/user/USB-STICK"),
        home.join("Music/Incoming/new"),
    ]
}

/// The plan of adding music: its figures and rows, Copy or Move, the rows
/// shown, a run to its end, and "Show in the library".
fn the_plan_of_adding_music(app: &gtk::Application) {
    let (main, backend) = window(app);
    turn_on(app, &backend, music().join("Library"));
    let tool = main.open_managed_add(sources()).unwrap();
    wait_for("the plan", || {
        texts(&tool).contains("Add 214 files to the managed folder")
    });
    let shown = texts(&tool);
    for text in [
        "From 3 places · copied · named by the pattern",
        "to copy",
        "get a number",
        "already there",
        "left where they are",
        "Gets a number",
        "Copy, named like its track",
        "CUE sheet updated",
        "An identical file is already there",
        "Nothing changes until you start. Your choices here are for this run only.",
        "Nothing is replaced: a name that is taken gets a number.",
        "~/Downloads/bandcamp",
        "Add more…",
    ] {
        assert!(shown.contains(text), "{text} in {shown}");
    }
    let run: gtk::Button = named(&tool, "managed-run");
    assert_eq!(run.label().unwrap(), "Copy 214 files");
    assert!(run.is_sensitive());
    assert!(!named::<gtk::Box>(&tool, "managed-notice").is_visible());
    shot(&tool, "ManagedImport");

    // Only the files left where they are: 9 identical and 4 others.
    let which: gtk::DropDown = named(&tool, "managed-shown");
    let view: gtk::ListView = named(&tool, "managed-table");
    which.set_selected(2);
    spin(100);
    assert_eq!(view.model().unwrap().n_items(), 13);
    which.set_selected(0);

    // Move for this run only.
    let transfer: gtk::Box = named(&tool, "managed-transfer");
    let move_button = descendants(transfer.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::ToggleButton>().ok())
        .nth(1)
        .unwrap();
    move_button.set_active(true);
    wait_for("the moving plan", || {
        run.label().is_some_and(|l| l == "Move 214 files")
    });
    assert_eq!(
        backend.services().settings().library.managed.mode,
        helikodu_model::settings::ManagedMode::Copy,
        "the setting stays"
    );
    assert_idle(&tool);

    // The run, to its end: no undo, "Show in the library".
    run.emit_clicked();
    wait_for("the run at work", || main.managed_footer_shown());
    let show: gtk::Button = named(&tool, "managed-show");
    wait_for("the end", || show.is_visible());
    let summary: gtk::Label = named(&tool, "managed-summary");
    assert!(summary.text().starts_with("Done."), "{}", summary.text());
    assert!(texts(&tool).contains("moved"));
    assert!(!main.managed_footer_shown());
    // Closing the window tells the end in a toast, with Show and no Undo.
    tool.close();
    spin(100);
    assert_eq!(
        main.toast_text().as_deref(),
        Some("214 files were added to the managed folder")
    );
    shot(&main, "ToastAdded");
    main.destroy();
}

/// A plan that does not fit says so and cannot start.
fn a_plan_that_does_not_fit(app: &gtk::Application) {
    let (main, backend) = window(app);
    turn_on(app, &backend, PathBuf::from("/run/media/user/SMALL/Music"));
    let tool = main.open_managed_add(sources()).unwrap();
    let notice: gtk::Box = named(&tool, "managed-notice");
    wait_for("the notice", || notice.is_visible());
    let shown = texts(&tool);
    assert!(
        shown.contains("The drive of the managed folder has 1.1"),
        "{shown}"
    );
    assert!(shown.contains("The run cannot start until the files fit."));
    shot(&tool, "ManagedImportFull");
    assert!(!named::<gtk::Button>(&tool, "managed-run").is_sensitive());
    tool.close();
    main.destroy();
}

/// Gathering the library at work, stopped: the run is cut short, its toast
/// leads to the dialog, and Continue places the rest; stopped again, Roll back
/// settles it.
fn gathering_is_cut_short_and_settled(app: &gtk::Application) {
    let (main, backend) = window(app);
    turn_on(app, &backend, music().join("Library"));
    app.activate_action("managed-gather", None);
    wait_for("the window", || toplevel("managed-folder").is_some());
    let tool = toplevel("managed-folder").unwrap();
    wait_for("the plan", || {
        texts(&tool).contains("Gather 1,098 files into the managed folder")
    });
    assert!(texts(&tool).contains("A copy leaves the original on the disk"));
    assert!(!named::<gtk::SearchEntry>(&tool, "managed-filter").is_visible());
    named::<gtk::Button>(&tool, "managed-run").emit_clicked();
    let work: gtk::Box = named(&tool, "managed-work");
    wait_for("the run at work", || work.is_visible());
    let placed = || texts(&tool).contains("Copied") && texts(&tool).contains(" written");
    let end = Instant::now() + Duration::from_secs(10);
    while !placed() {
        assert!(Instant::now() < end, "no file placed: {}", texts(&tool));
        spin(10);
    }
    assert!(main.managed_footer_shown());
    shot(&tool, "ManagedGather");
    shot(&main, "ManagedGatherMain");
    let stop = descendants(work.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .last()
        .unwrap();
    stop.emit_clicked();
    wait_for("the cut-short toast", || {
        main.toast_text().as_deref() == Some("Gathering the library was cut short")
    });
    let summary: gtk::Label = named(&tool, "managed-summary");
    wait_for("the footer", || {
        summary.text().starts_with("A run was cut short.")
    });
    assert!(named::<gtk::Button>(&tool, "managed-details").is_visible());
    // The toast stays: it waits for a decision.
    spin(6000);
    assert_eq!(
        main.toast_text().as_deref(),
        Some("Gathering the library was cut short")
    );

    // The dialog: what was placed and what waits; Continue places the rest.
    let dialog = main.open_managed_cut_short().unwrap();
    spin(100);
    let shown = texts(&dialog);
    assert!(
        shown.contains("Gathering the library was cut short"),
        "{shown}"
    );
    assert!(shown.contains("Continue places the rest."), "{shown}");
    assert!(shown.contains("No new run can start until you choose."));
    shot(&dialog, "ManagedCutShort");
    shot(&main, "ToastCutShort");
    let placed: gtk::Label = named(&dialog, "cut-short-placed");
    assert!(
        placed.text().contains(" of 1,098 files"),
        "{}",
        placed.text()
    );
    named::<gtk::Button>(&dialog, "continue").emit_clicked();
    wait_for("the run settled", || {
        toplevel("managed-cut-short").is_none() && !main.managed_footer_shown()
    });
    assert_ne!(
        main.toast_text().as_deref(),
        Some("Gathering the library was cut short"),
        "the toast goes once the run is settled"
    );
    assert!(
        backend
            .services()
            .interrupted_managed_run()
            .unwrap()
            .is_none()
    );

    // A second run, stopped and rolled back.
    tool.close();
    spin(100);
    let tool = main.open_managed_gather().unwrap();
    let run: gtk::Button = named(&tool, "managed-run");
    wait_for("the plan again", || run.is_sensitive());
    run.emit_clicked();
    let work: gtk::Box = named(&tool, "managed-work");
    wait_for("the run at work", || work.is_visible());
    spin(50);
    descendants(work.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .last()
        .unwrap()
        .emit_clicked();
    wait_for("the run cut short", || {
        backend
            .services()
            .interrupted_managed_run()
            .is_ok_and(|r| r.is_some())
    });
    // Nothing new can start until it is settled.
    wait_for("the footer", || {
        named::<gtk::Label>(&tool, "managed-summary")
            .text()
            .starts_with("A run was cut short.")
    });
    assert!(!run.is_sensitive());
    let dialog = main.open_managed_cut_short().unwrap();
    spin(100);
    named::<gtk::Button>(&dialog, "roll-back").emit_clicked();
    wait_for("the roll back", || toplevel("managed-cut-short").is_none());
    assert!(
        backend
            .services()
            .interrupted_managed_run()
            .unwrap()
            .is_none()
    );
    assert_eq!(
        main.toast_text().as_deref(),
        Some("The run was rolled back.")
    );
    tool.close();
    main.destroy();
}

/// The group shows a run that works and a run cut short.
fn the_settings_group_follows_the_runs(app: &gtk::Application) {
    let (main, backend) = window(app);
    turn_on(app, &backend, music().join("Library"));
    let settings = main.open_settings("library");
    let text: gtk::Label = named(&settings, "managed-footer-text");
    wait_for("the count", || text.text().contains("1,120"));
    let tool = main.open_managed_gather().unwrap();
    let run: gtk::Button = named(&tool, "managed-run");
    wait_for("the plan", || run.is_sensitive());
    run.emit_clicked();
    wait_for("the progress", || text.text().contains(" of 1,098 files"));
    assert!(!named::<gtk::Button>(&settings, "managed-add").is_sensitive());
    assert!(!named::<gtk::Button>(&settings, "managed-gather").is_sensitive());
    let work: gtk::Box = named(&tool, "managed-work");
    descendants(work.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .last()
        .unwrap()
        .emit_clicked();
    wait_for("the run cut short", || {
        text.text() == "A run was cut short. Nothing can be added until it is settled."
    });
    let details: gtk::Button = named(&settings, "managed-details");
    assert!(details.is_visible());
    assert!(!named::<gtk::Button>(&settings, "managed-add").is_visible());
    details.emit_clicked();
    wait_for("the dialog", || toplevel("managed-cut-short").is_some());
    let dialog = toplevel("managed-cut-short").unwrap();
    named::<gtk::Button>(&dialog, "roll-back").emit_clicked();
    wait_for("the run settled", || text.text().contains("1,120"));
    tool.close();
    settings.close();
    main.destroy();
}

type Test = (&'static str, fn(&gtk::Application));

const TESTS: &[Test] = &[
    (
        "the_settings_group_turns_on_and_off",
        the_settings_group_turns_on_and_off,
    ),
    ("the_plan_of_adding_music", the_plan_of_adding_music),
    ("a_plan_that_does_not_fit", a_plan_that_does_not_fit),
    (
        "gathering_is_cut_short_and_settled",
        gathering_is_cut_short_and_settled,
    ),
    (
        "the_settings_group_follows_the_runs",
        the_settings_group_follows_the_runs,
    ),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("managed: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("managed: skipped, no display");
        return ExitCode::SUCCESS;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.ManagedTests")
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
    application.run_with_args(&["managed"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("managed: {} passed", TESTS.len());
        ExitCode::SUCCESS
    } else {
        println!("managed: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
