// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Three screens of design 1.4 against the sample backend: File info with the
//! track analysis (analysed, not analysed yet, could not be read), the Tracks
//! view grouped by a field, and the offer to import from another player after
//! the first scan.
//!
//! Like `widgets.rs`, this program runs only under the headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test optional
//! ```
//!
//! With `HELIKODU_OPTIONAL_SHOTS=DIR` it saves the screens to `DIR`.

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
use helikodu_ui_gtk::backend::services::AnalysisFacts;
use helikodu_ui_gtk::backend::{Backend, TrackColumn, TrackGrouping, TrackKey, TrackQuery, sample};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display, snapshot};

type Test = (&'static str, fn(&gtk::Application));

fn spin(ms: u64) {
    let context = glib::MainContext::default();
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for(what: &str, seconds: u64, done: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(seconds);
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

/// Saves `window` to `HELIKODU_OPTIONAL_SHOTS/name.png` when that is set.
fn shot(window: &impl IsA<gtk::Window>, name: &str) {
    let Some(dir) = std::env::var_os("HELIKODU_OPTIONAL_SHOTS") else {
        return;
    };
    spin(300);
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    snapshot::save_png(window, &path).unwrap();
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

fn named(root: &impl IsA<gtk::Widget>, name: &str) -> Vec<gtk::Widget> {
    descendants(root.upcast_ref())
        .into_iter()
        .filter(|w| w.widget_name() == name)
        .collect()
}

fn texts(widgets: &[gtk::Widget]) -> Vec<String> {
    widgets
        .iter()
        .filter_map(|w| w.downcast_ref::<gtk::Label>())
        .map(|l| l.text().to_string())
        .collect()
}

/// A sample track whose analysis is in the state `state` picks.
fn track_in(backend: &Backend, state: fn(&AnalysisFacts) -> bool) -> TrackKey {
    let keys = backend
        .library()
        .track_keys(&TrackQuery::default())
        .unwrap();
    let services = backend.services();
    keys.chunks(20)
        .flat_map(|chunk| services.edit_tracks(chunk).unwrap())
        .find(|t| state(&t.facts.analysis) && t.facts.duration.is_some_and(|d| d.as_secs() > 200))
        .map(|t| t.key)
        .expect("a sample track in that state")
}

/// Opens the tag editor for `track` on its File info tab.
fn file_info(window: &MainWindow, track: TrackKey) -> gtk::Window {
    window.edit_tags(&[track]);
    wait_for("the tag editor", 3, || toplevel("tag-editor").is_some());
    let editor = toplevel("tag-editor").unwrap();
    let tab = descendants(editor.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::ToggleButton>().ok())
        .find(|b| b.label().as_deref() == Some("File info"))
        .unwrap();
    tab.set_active(true);
    spin(200);
    editor
}

fn file_info_shows_the_analysis(app: &gtk::Application) {
    let (window, backend) = window_over(app, sample::backend(&options()));

    let analysed = track_in(&backend, |a| matches!(a, AnalysisFacts::Analysed(_)));
    let editor = file_info(&window, analysed);
    let band = named(&editor, "analysis-band");
    assert_eq!(band.len(), 1, "one Analysis band");
    let curve = named(&editor, "loudness-curve");
    assert_eq!(curve.len(), 1, "the loudness curve");
    assert!(
        curve[0].is_drawable() && curve[0].height() >= 30,
        "drawn, 30 high"
    );
    let facts = texts(&named(&editor, "analysis-fact"));
    assert_eq!(facts.len(), 3, "sound, silence inside, fade-out: {facts:?}");
    assert!(
        facts.iter().any(|f| f.starts_with("0:00.42 to ")),
        "{facts:?}"
    );
    // "Tags in the file" is a row of the File group now.
    let labels: Vec<String> = descendants(editor.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Label>().ok())
        .map(|l| l.text().to_string())
        .collect();
    assert!(labels.iter().any(|t| t == "Tags"));
    assert!(
        !labels
            .iter()
            .any(|t| t.eq_ignore_ascii_case("Tags in the file"))
    );
    shot(&editor, "file-info-analysed");
    // Nothing redraws while the tab stays as it is.
    let clock = curve[0].frame_clock().unwrap();
    spin(200);
    let before = clock.frame_counter();
    spin(600);
    assert!(
        clock.frame_counter() - before <= 1,
        "no frames while idle ({} drawn)",
        clock.frame_counter() - before
    );
    editor.destroy();
    wait_for("the editor to close", 3, || {
        toplevel("tag-editor").is_none()
    });

    let pending = track_in(&backend, |a| matches!(a, AnalysisFacts::NotYet));
    let editor = file_info(&window, pending);
    assert!(named(&editor, "loudness-curve").is_empty());
    let note = texts(&named(&editor, "analysis-note"));
    assert!(note[0].starts_with("Not analysed yet."), "{note:?}");
    shot(&editor, "file-info-not-analysed");
    editor.destroy();
    wait_for("the editor to close", 3, || {
        toplevel("tag-editor").is_none()
    });

    let failed = track_in(&backend, |a| matches!(a, AnalysisFacts::Failed));
    let editor = file_info(&window, failed);
    assert!(named(&editor, "loudness-curve").is_empty());
    let note = texts(&named(&editor, "analysis-note"));
    assert!(
        note[0].starts_with("This file could not be read for analysis."),
        "{note:?}"
    );
    let warning = descendants(editor.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Image>().ok())
        .any(|i| i.icon_name().as_deref() == Some("helikodu-warning-symbolic"));
    assert!(warning, "a warning icon");
    shot(&editor, "file-info-failed");
    editor.destroy();
    window.destroy();
}

fn count_of(text: &str) -> usize {
    text.split_whitespace()
        .next()
        .unwrap()
        .replace(',', "")
        .parse()
        .unwrap()
}

fn tracks_group_by_a_field(app: &gtk::Application) {
    let (window, backend) = window_over(app, sample::backend(&options()));
    let total = backend.library().counts().unwrap().tracks;
    window.open_page("tracks");
    wait_for("the track keys", 3, || {
        window.track_table_stats().0 as usize == total
    });
    assert_eq!(window.grouping_texts().0, "Group");

    window.sort_tracks(&[(TrackColumn::Title, false)]);
    window.group_tracks(Some(TrackGrouping::Year));
    wait_for("the year groups", 3, || !window.track_groups().is_empty());
    let groups = window.track_groups();
    let years: Vec<&str> = groups.iter().map(|(v, _, _)| v.as_str()).collect();
    let mut sorted = years.clone();
    sorted.sort_unstable();
    assert_eq!(years, sorted, "groups in the order of their values");
    let counted: usize = groups.iter().map(|(_, c, _)| count_of(c)).sum();
    assert_eq!(counted, total, "every track in one group");
    assert_eq!(
        window.track_table_stats().0 as usize,
        total + groups.len(),
        "a row for each group"
    );
    let (button, status) = window.grouping_texts();
    assert_eq!(button, "Group by Year");
    assert_eq!(status, "Grouped by year");
    // Within a group, the order the headers show: by title.
    let rows = backend
        .library()
        .track_rows(&window.track_keys(total + groups.len()))
        .unwrap();
    let first_group = count_of(&groups[0].1);
    let titles: Vec<String> = rows[..first_group]
        .iter()
        .map(|r| r.title.to_lowercase())
        .collect();
    assert!(titles.windows(2).all(|w| w[0] <= w[1]), "sorted by title");
    // The group rows are drawn across the table.
    spin(200);
    let drawn = descendants(window.upcast_ref())
        .into_iter()
        .filter(|w| w.has_css_class("hk-group"))
        .count();
    assert!(drawn > 0, "group rows are shown");
    shot(&window, "tracks-grouped-by-year");

    // Activating a group row closes it, and again opens it; Left and Right too.
    let first = groups[0].0.clone();
    assert!(window.activate_track_group(&first));
    spin(100);
    assert!(!window.track_groups()[0].2, "closed");
    assert_eq!(
        window.track_table_stats().0 as usize,
        total + groups.len() - first_group
    );
    assert!(window.focus_track_group(&first));
    spin(100);
    assert!(window.press_on_track_group(true));
    spin(100);
    assert!(window.track_groups()[0].2, "open again");
    assert!(window.press_on_track_group(false));
    spin(100);
    assert!(!window.track_groups()[0].2, "closed by Left");

    // The context menu of a group row acts on all its tracks, closed or not.
    assert!(window.select_track_group(&first));
    spin(50);
    assert_eq!(window.selected_track_keys().len(), first_group);

    // One of the user's fields: the tracks without a value come first.
    window.group_tracks(Some(TrackGrouping::Field("mood".into())));
    wait_for("the mood groups", 3, || {
        window
            .track_groups()
            .first()
            .is_some_and(|(v, _, _)| v == "No value")
    });
    assert_eq!(window.grouping_texts().1, "Grouped by mood");
    window.group_tracks(Some(TrackGrouping::Field("decade".into())));
    wait_for("the decade groups", 3, || {
        window
            .track_groups()
            .iter()
            .any(|(v, _, _)| v.ends_with("0s"))
    });
    shot(&window, "tracks-grouped-by-decade");
    window.set_default_size(700, 800);
    spin(300);
    // Narrow: the table scrolls sideways, the group rows stay in place.
    if let Some(table) = descendants(window.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::ColumnView>().ok())
        .find(|t| t.is_mapped())
        && let Some(across) = table.hadjustment()
    {
        across.set_value(across.upper() - across.page_size());
        spin(100);
    }
    shot(&window, "tracks-grouped-narrow");
    window.set_default_size(1440, 900);
    window.group_tracks(Some(TrackGrouping::Field("mood".into())));
    wait_for("the mood groups again", 3, || {
        window.grouping_texts().1 == "Grouped by mood"
    });
    // An expression.
    window.group_tracks(Some(TrackGrouping::Expression("{left(title, 1)}".into())));
    wait_for("the expression groups", 3, || {
        window.track_groups().len() > 2
            && window.grouping_texts().1 == "Grouped by {left(title, 1)}"
    });
    // No groups.
    window.group_tracks(None);
    wait_for("no groups", 3, || window.track_groups().is_empty());
    assert_eq!(window.track_table_stats().0 as usize, total);
    assert_eq!(window.grouping_texts(), ("Group".to_owned(), String::new()));
    window.destroy();
}

fn the_import_is_offered_once(app: &gtk::Application) {
    let (window, backend) = window_over(app, sample::import_offer_backend(&options()));
    // After the window, whose actions it replaces: the Import window itself
    // is tested with its own suite.
    let opened: Rc<RefCell<Vec<String>>> = Rc::default();
    let action = gio::SimpleAction::new("import-from-named-player", Some(glib::VariantTy::STRING));
    let o = opened.clone();
    action.connect_activate(move |_, value| {
        o.borrow_mut().push(
            value
                .and_then(|v| v.str().map(str::to_owned))
                .unwrap_or_default(),
        );
    });
    app.add_action(&action);
    let services = backend.services();
    wait_for("the first run", 3, || window.first_run_shown());
    descendants(window.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| {
            descendants(b.upcast_ref()).iter().any(|w| {
                w.downcast_ref::<gtk::Label>()
                    .is_some_and(|l| l.text() == "Scan and start")
            }) || b.label().as_deref() == Some("Scan and start")
        })
        .unwrap()
        .emit_clicked();
    spin(50);
    assert!(!window.first_run_shown());
    services
        .scan(&[std::path::PathBuf::from("/music")])
        .unwrap();
    let offer = "Rhythmbox’s plays and ratings can be brought over";
    wait_for("the offer", 12, || {
        window.toast_text().as_deref() == Some(offer)
    });
    shot(&window, "import-offer");
    assert!(services.import_offer().is_empty(), "made once, ever");
    // It stays until it is dismissed.
    spin(6_000);
    assert_eq!(window.toast_text().as_deref(), Some(offer));
    let import = descendants(window.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.label().as_deref() == Some("Import…"))
        .unwrap();
    import.emit_clicked();
    spin(50);
    assert_eq!(*opened.borrow(), ["rhythmbox"]);
    assert!(window.toast_text().is_none());
    // The next scan offers nothing.
    services
        .scan(&[std::path::PathBuf::from("/music")])
        .unwrap();
    spin(7_000);
    assert_ne!(window.toast_text().as_deref(), Some(offer));
    app.remove_action("import-from-named-player");
    window.destroy();
}

const TESTS: &[Test] = &[
    ("file_info_shows_the_analysis", file_info_shows_the_analysis),
    ("tracks_group_by_a_field", tracks_group_by_a_field),
    ("the_import_is_offered_once", the_import_is_offered_once),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("optional: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("optional: skipped, no display");
        return ExitCode::SUCCESS;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.OptionalTests")
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
    application.run_with_args(&["optional"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("optional: {} passed", TESTS.len());
        ExitCode::SUCCESS
    } else {
        println!("optional: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
