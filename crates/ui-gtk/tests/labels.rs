// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Labels against the sample backend: the sidebar row, the Labels view and its
//! empty state, the Labels popover with its states and its one Undo, the chips
//! of the album and artist pages, renaming, merging and removing a label.
//!
//! Like `views.rs`, this program has its own main function and runs only under
//! the headless compositor script:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test labels
//! ```
//!
//! With `HELIKODU_LABELS_SHOTS=DIR` it saves the screens to `DIR`.

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
use helikodu_ui_gtk::backend::{AlbumKey, Backend, LabelState, TrackKey, sample};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display, snapshot};

/// Runs the main loop for `ms` milliseconds.
fn spin(ms: u64) {
    let context = glib::MainContext::default();
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Runs the main loop until `done` holds, for at most `seconds`.
fn wait_for(what: &str, seconds: u64, done: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(seconds);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        spin(10);
    }
}

fn window_sized(application: &gtk::Application, width: i32) -> (MainWindow, Backend) {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 300,
        playing: None,
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(width, 900);
    window.present();
    spin(150);
    (window, backend)
}

fn window(application: &gtk::Application) -> (MainWindow, Backend) {
    window_sized(application, 1440)
}

fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(widget) = pending.pop() {
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c.clone());
        }
        out.push(widget);
    }
    out
}

/// The texts of the mapped labels of `root`.
fn shown_texts(root: &gtk::Widget) -> Vec<String> {
    descendants(root)
        .into_iter()
        .filter(gtk::Widget::is_mapped)
        .filter_map(|w| w.downcast::<gtk::Label>().ok())
        .map(|l| l.text().to_string())
        .collect()
}

/// The sidebar's Labels row: whether it is shown, and its count.
fn sidebar_labels(window: &MainWindow) -> (bool, String) {
    let row = descendants(window.upcast_ref())
        .into_iter()
        .find(|w| w.is::<gtk::ListBoxRow>() && w.widget_name() == "labels")
        .expect("the Labels row");
    let count = descendants(&row)
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Label>().ok())
        .find(|l| l.has_css_class("hk-count"))
        .map(|l| l.text().to_string())
        .unwrap_or_default();
    (row.is_visible(), count)
}

/// Saves the window to `HELIKODU_LABELS_SHOTS/name.png` when that is set.
fn shot(window: &MainWindow, name: &str) {
    let Some(dir) = std::env::var_os("HELIKODU_LABELS_SHOTS") else {
        return;
    };
    spin(300);
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    snapshot::save_png(window, &path).unwrap();
}

/// Saves a popover or a dialog when `HELIKODU_LABELS_SHOTS` is set.
fn shot_widget(widget: &impl IsA<gtk::Widget>, name: &str) {
    let Some(dir) = std::env::var_os("HELIKODU_LABELS_SHOTS") else {
        return;
    };
    spin(300);
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    snapshot::save_widget_png(widget, &path).unwrap();
}

/// Presses Undo: the toast's, as Ctrl+Z does while it is shown.
fn undo(window: &MainWindow) {
    WidgetExt::activate_action(window, "win.undo", None).unwrap();
}

fn the_sidebar_row_and_the_view(app: &gtk::Application) {
    let (window, _) = window(app);
    wait_for("the Labels row", 3, || sidebar_labels(&window).0);
    assert_eq!(sidebar_labels(&window).1, "12");
    window.open_page("labels");
    assert_eq!(window.visible_view().as_deref(), Some("labels"));
    wait_for("the labels", 3, || window.label_list().len() == 12);
    let names: Vec<String> = window.label_list().into_iter().map(|l| l.0).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "in name order");
    assert_eq!(names[4], "late night");
    window.choose_label(4);
    wait_for("the detail", 3, || {
        window
            .label_detail_texts()
            .iter()
            .any(|t| t.starts_with("track:"))
    });
    let texts = window.label_detail_texts();
    assert_eq!(texts[0], "late night");
    assert!(
        texts[1].starts_with("Given to 1 artist, 2 albums and 14 tracks · "),
        "{}",
        texts[1]
    );
    assert!(texts[1].ends_with(" tracks in all"), "{}", texts[1]);
    assert_eq!(
        texts.iter().filter(|t| t.starts_with("artist:")).count(),
        1,
        "{texts:?}"
    );
    assert_eq!(texts.iter().filter(|t| t.starts_with("album:")).count(), 2);
    assert_eq!(texts.iter().filter(|t| t.starts_with("track:")).count(), 14);
    let shown = shown_texts(window.upcast_ref());
    assert!(shown.iter().any(|t| t == "14 labelled one by one"));
    shot(&window, "Labels");
    // Play all plays the label.
    let play = descendants(window.upcast_ref())
        .into_iter()
        .filter(gtk::Widget::is_mapped)
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| shown_texts(b.upcast_ref()).iter().any(|t| t == "Play all"))
        .expect("Play all");
    play.emit_clicked();
    wait_for("playback", 3, || window.is_playing());
    window.destroy();
}

fn the_empty_view_and_undo(app: &gtk::Application) {
    let (window, _) = window(app);
    window.open_page("labels");
    wait_for("the labels", 3, || window.label_list().len() == 12);
    for left in (0..12).rev() {
        window.label_action("delete", 0);
        wait_for("the label to go", 3, || window.label_list().len() == left);
    }
    assert_eq!(
        window.toast_text().as_deref(),
        Some("Removed “work” from everything")
    );
    wait_for("the empty state", 3, || window.labels_empty_shown());
    wait_for("the Labels row to go", 3, || !sidebar_labels(&window).0);
    let texts = shown_texts(window.upcast_ref());
    assert!(texts.iter().any(|t| t == "No labels yet"), "{texts:?}");
    shot(&window, "LabelsEmpty");
    // Undo brings the last one back.
    undo(&window);
    wait_for("the label back", 3, || window.label_list().len() == 1);
    assert_eq!(window.label_list()[0].0, "work");
    wait_for("the Labels row back", 3, || {
        sidebar_labels(&window) == (true, "1".into())
    });
    assert!(!window.labels_empty_shown());
    // The narrow window keeps the same block.
    window.label_action("delete", 0);
    wait_for("the empty state", 3, || window.labels_empty_shown());
    window.set_default_size(640, 900);
    spin(300);
    shot(&window, "LabelsEmptyNarrow");
    window.destroy();
}

fn the_popover_shows_and_changes_the_states(app: &gtk::Application) {
    let (window, backend) = window(app);
    // The first two tracks of the third album have "night drive"; the album has
    // "night shift" and its artist "late night".
    let album = backend.library().album(AlbumKey(3)).unwrap().unwrap();
    let keys: Vec<TrackKey> = album.tracks().take(3).map(|t| t.key).collect();
    window.demo_labels_popover(keys.clone());
    wait_for("the rows", 3, || window.labels_popover_rows().len() > 2);
    assert!(
        window.labels_popover_entry_focused(),
        "the entry has the focus"
    );
    let rows = window.labels_popover_rows();
    let row = |name: &str| rows.iter().find(|r| r.0 == name).cloned().unwrap();
    assert_eq!(row("night drive").2, Some(LabelState::Some));
    assert_eq!(row("night shift").2, Some(LabelState::FromAlbum));
    assert_eq!(row("night shift").1, "from the album");
    assert!(!row("night shift").3, "an inherited label is insensitive");
    assert_eq!(row("late night").2, Some(LabelState::FromArtist));
    assert_eq!(row("late night").1, "from the artist");
    assert_eq!(row("winter").2, Some(LabelState::None));
    assert_eq!(rows.last().unwrap().0, "Show all labels");
    // A mixed box says so.
    let position = rows.iter().position(|r| r.0 == "night drive").unwrap();
    assert_eq!(window.labels_popover_checks()[position], (false, true));

    // Typing filters; "New label" shows while the text is no label's name.
    window.labels_popover_type("night", false);
    let rows = window.labels_popover_rows();
    let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(
        names,
        [
            "late night",
            "night drive",
            "night shift",
            "New label “night”",
            "Show all labels"
        ]
    );
    let popover = window.labels_popover().expect("the popover");
    assert!(popover.is_visible(), "the popover is open");
    shot_widget(&popover, "LabelsPopover");
    window.labels_popover_type("night drive", false);
    assert!(
        !window
            .labels_popover_rows()
            .iter()
            .any(|r| r.0.starts_with("New label")),
        "the name of a label is no new label"
    );
    window.labels_popover_type("", false);

    // A click on the mixed one gives it to all; the toast says so.
    assert!(window.labels_popover_click("night drive"));
    wait_for("night drive for all", 3, || {
        window
            .labels_popover_rows()
            .iter()
            .any(|r| r.0 == "night drive" && r.2 == Some(LabelState::All))
    });
    assert_eq!(
        window.toast_text().as_deref(),
        Some("Gave “night drive” to 3 tracks")
    );
    // A second click takes it from all.
    assert!(window.labels_popover_click("night drive"));
    wait_for("night drive for none", 3, || {
        window
            .labels_popover_rows()
            .iter()
            .any(|r| r.0 == "night drive" && r.2 == Some(LabelState::None))
    });
    // Enter gives a new label and clears the entry.
    window.labels_popover_type("fresh", true);
    wait_for("the new label", 3, || {
        window
            .labels_popover_rows()
            .iter()
            .any(|r| r.0 == "fresh" && r.2 == Some(LabelState::All))
    });
    assert!(
        window
            .labels_popover_rows()
            .iter()
            .all(|r| !r.0.starts_with("New label")),
        "the entry is empty again"
    );
    wait_for("the count", 3, || sidebar_labels(&window).1 == "13");
    // The first toast is still up, and its Undo takes back all three changes.
    assert_eq!(
        window.toast_text().as_deref(),
        Some("Gave “night drive” to 3 tracks")
    );
    undo(&window);
    wait_for("everything undone", 3, || {
        let rows = window.labels_popover_rows();
        !rows.iter().any(|r| r.0 == "fresh")
            && rows
                .iter()
                .any(|r| r.0 == "night drive" && r.2 == Some(LabelState::Some))
    });
    wait_for("the count back", 3, || sidebar_labels(&window).1 == "12");
    // Show all labels opens the view.
    assert!(window.labels_popover_click("Show all labels"));
    wait_for("the Labels view", 3, || {
        window.visible_view().as_deref() == Some("labels")
    });
    // The closed popover lets go of the window.
    spin(100);
    window.destroy();
}

fn the_album_and_artist_pages_show_chips(app: &gtk::Application) {
    let (window, backend) = window(app);
    window.open_album(AlbumKey(1));
    wait_for("the chips", 3, || window.label_chip_texts().len() == 3);
    assert_eq!(
        window.label_chip_texts(),
        ["late night", "road trip", "Add label"]
    );
    let texts = shown_texts(window.upcast_ref());
    assert!(texts.iter().any(|t| t == "Record label"), "{texts:?}");
    assert!(!texts.iter().any(|t| t == "Label"), "{texts:?}");
    shot(&window, "Album");
    // "Add label" opens the popover for the album; a label given shows as a chip.
    assert!(window.click_label_chip("Add label"));
    assert!(
        window.labels_popover().is_some(),
        "Add label opens the popover"
    );
    // Without input the compositor closes a popover that grabs it; this one
    // does not grab, and takes the place of the first.
    window.demo_album_labels_popover(AlbumKey(1), "the album");
    wait_for("the popover", 3, || window.labels_popover_rows().len() > 2);
    let rows = window.labels_popover_rows();
    assert!(
        rows.iter()
            .any(|r| r.0 == "road trip" && r.2 == Some(LabelState::All))
    );
    window.labels_popover_type("winter", true);
    wait_for("the new chip", 3, || {
        window.label_chip_texts() == ["late night", "road trip", "winter", "Add label"]
    });
    window.labels_popover().unwrap().popdown();
    spin(100);
    assert!(window.labels_popover().is_none(), "closed");
    // A chip opens its label.
    assert!(window.click_label_chip("road trip"));
    wait_for("the label", 3, || {
        window.visible_view().as_deref() == Some("labels")
            && window.label_detail_texts().first().map(String::as_str) == Some("road trip")
    });
    // The artist of the first album has "favourite".
    let artist = backend
        .library()
        .album(AlbumKey(1))
        .unwrap()
        .unwrap()
        .row
        .artist;
    let key = backend.library().find_artist(&artist).unwrap().unwrap();
    window.demo_open_artist(key);
    wait_for("the artist's chips", 3, || {
        window.label_chip_texts().first().map(String::as_str) == Some("favourite")
    });
    assert_eq!(window.label_chip_texts().last().unwrap(), "Add label");
    shot(&window, "Artists");
    window.destroy();
}

fn labels_are_renamed_merged_and_removed(app: &gtk::Application) {
    let (window, _) = window(app);
    window.open_page("labels");
    wait_for("the labels", 3, || window.label_list().len() == 12);
    // Renamed in place.
    window.choose_label(1);
    window.label_action("rename", 1);
    let entry = window.label_rename_entry().expect("the rename entry");
    assert_eq!(entry.text(), "evening walk");
    entry.set_text("evening stroll");
    entry.emit_activate();
    wait_for("the new name", 3, || {
        window.label_list().iter().any(|l| l.0 == "evening stroll")
    });
    assert_eq!(
        window.toast_text().as_deref(),
        Some("Renamed “evening walk” to “evening stroll”")
    );
    undo(&window);
    wait_for("the old name", 3, || {
        window.label_list().iter().any(|l| l.0 == "evening walk")
    });
    // Renaming to another label's name asks whether to merge.
    let tracks: usize = window.label_list()[0].1 + window.label_list()[1].1;
    window.choose_label(0);
    window.label_action("rename", 0);
    let entry = window.label_rename_entry().expect("the rename entry");
    entry.set_text("Evening walk");
    entry.emit_activate();
    let dialog = || {
        gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Window>().ok())
            .find(|w| w.widget_name() == "label-merge" && w.is_visible())
    };
    wait_for("the merge dialog", 3, || dialog().is_some());
    let dialog = dialog().unwrap();
    let texts = shown_texts(dialog.upcast_ref());
    assert!(
        texts
            .iter()
            .any(|t| t == "Merge “after hours” into another label"),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.ends_with("This can be undone.")),
        "{texts:?}"
    );
    let dropdown = descendants(dialog.upcast_ref())
        .into_iter()
        .find_map(|w| w.downcast::<gtk::DropDown>().ok())
        .unwrap();
    assert_eq!(dropdown.selected(), 0, "the label of that name is chosen");
    shot_widget(&dialog, "LabelMerge");
    let merge = descendants(dialog.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.label().as_deref() == Some("Merge"))
        .unwrap();
    merge.emit_clicked();
    wait_for("the merge", 3, || window.label_list().len() == 11);
    let walk = window
        .label_list()
        .into_iter()
        .find(|l| l.0 == "evening walk")
        .unwrap();
    assert!(walk.1 <= tracks && walk.1 > 0);
    assert_eq!(
        window.toast_text().as_deref(),
        Some("Merged “after hours” into “evening walk”")
    );
    undo(&window);
    wait_for("both back", 3, || window.label_list().len() == 12);
    window.destroy();
}

fn the_narrow_window_keeps_list_and_detail(app: &gtk::Application) {
    let (window, _) = window_sized(app, 640);
    window.open_page("labels");
    wait_for("the labels", 3, || window.label_list().len() == 12);
    window.choose_label(4);
    wait_for("the detail", 3, || {
        window.label_detail_texts().first().map(String::as_str) == Some("late night")
    });
    spin(200);
    let texts = shown_texts(window.upcast_ref());
    assert!(texts.iter().any(|t| t == "late night"));
    assert!(texts.iter().any(|t| t == "Play all"));
    shot(&window, "LabelsNarrow");
    window.destroy();
}

type Test = fn(&gtk::Application);

const TESTS: &[(&str, Test)] = &[
    ("the_sidebar_row_and_the_view", the_sidebar_row_and_the_view),
    ("the_empty_view_and_undo", the_empty_view_and_undo),
    (
        "the_popover_shows_and_changes_the_states",
        the_popover_shows_and_changes_the_states,
    ),
    (
        "the_album_and_artist_pages_show_chips",
        the_album_and_artist_pages_show_chips,
    ),
    (
        "labels_are_renamed_merged_and_removed",
        labels_are_renamed_merged_and_removed,
    ),
    (
        "the_narrow_window_keeps_list_and_detail",
        the_narrow_window_keeps_list_and_detail,
    ),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("labels: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("labels: skipped, no display");
        return ExitCode::SUCCESS;
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let scheme = if std::env::var_os("HELIKODU_LABELS_LIGHT").is_some() {
        SchemeChoice::Light
    } else {
        SchemeChoice::Dark
    };
    prepare_display(scheme, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.LabelsTests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let failures = Rc::new(RefCell::new(Vec::new()));
    let ran = Rc::new(RefCell::new(0));
    let (f, r) = (failures.clone(), ran.clone());
    application.connect_activate(move |application| {
        let _hold = application.hold();
        for (name, test) in TESTS {
            if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
                continue;
            }
            *r.borrow_mut() += 1;
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
    application.run_with_args(&["labels"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("labels: {} passed", ran.borrow());
        ExitCode::SUCCESS
    } else {
        println!("labels: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
