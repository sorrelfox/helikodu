// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Import from another player, the "Replaced" section of Missing files and
//! "Your data" against the sample backend: the plan read in the background
//! with its working state, the figures and the entries left out, an import
//! with its toast and undo (only the newest, never by Ctrl+Z), folders that
//! moved, a Helikodu data file and files that cannot be read, statistics
//! carried to replacement files and undone, and the export with its toast.
//!
//! Like `tools.rs`, this program runs only under the headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test interchange
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
use helikodu_ui_gtk::backend::sample;
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
    find(root, name).unwrap_or_else(|| panic!("no widget {name}"))
}

fn find<T: IsA<gtk::Widget>>(root: &impl IsA<gtk::Widget>, name: &str) -> Option<T> {
    descendants(root.upcast_ref())
        .into_iter()
        .find(|w| w.widget_name() == name)
        .and_then(|w| w.downcast::<T>().ok())
}

fn texts(root: &impl IsA<gtk::Widget>) -> String {
    descendants(root.upcast_ref())
        .iter()
        .filter(|w| w.is_mapped())
        .filter_map(|w| {
            w.downcast_ref::<gtk::Label>()
                .map(|l| l.text().to_string())
                .or_else(|| {
                    w.downcast_ref::<gtk::Button>()
                        .and_then(|b| b.label())
                        .map(|l| l.to_string())
                })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The open popover called `name`, among the toplevels' descendants.
fn popover(root: &impl IsA<gtk::Widget>, name: &str) -> Option<gtk::Popover> {
    descendants(root.upcast_ref())
        .into_iter()
        .filter(|w| w.is_visible())
        .find(|w| w.widget_name() == name)
        .and_then(|w| w.downcast::<gtk::Popover>().ok())
}

/// Answers file requests with `path`.
fn choose_file(path: Option<PathBuf>) {
    chooser::replace_folder_chooser(Some(Rc::new(
        move |_title: &str, reply: chooser::FolderReply| {
            reply(path.clone());
        },
    )));
}

/// The toast's button labelled `text`.
fn toast_button(main: &MainWindow, text: &str) -> gtk::Button {
    descendants(main.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.has_css_class("action") && b.label().as_deref() == Some(text))
        .unwrap_or_else(|| panic!("no toast button {text}"))
}

fn open_import(main: &MainWindow, player: Option<&str>) -> gtk::Window {
    let window = main.open_import(player).unwrap();
    wait_for("the import window", || toplevel("import").is_some());
    window
}

fn import_button(tool: &gtk::Window) -> gtk::Button {
    named(tool, "import-apply")
}

/// Saves `window` to `HELIKODU_IMPORT_SHOTS/name.png` when that is set.
fn shot(window: &gtk::Window, name: &str) {
    let Some(dir) = std::env::var_os("HELIKODU_IMPORT_SHOTS") else {
        return;
    };
    spin(300);
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    snapshot::save_png(window, &path).unwrap();
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

/// The Library menu opens the window on Rhythmbox; it reads in the
/// background, then shows the plan; the import counts the tracks that would
/// change and is undone only by its own Undo, the newest first.
fn rhythmbox_is_planned_imported_and_undone(app: &gtk::Application) {
    let main = window(app);
    app.activate_action("import-from-player", None);
    wait_for("the import window", || toplevel("import").is_some());
    let tool = toplevel("import").unwrap();
    let import = import_button(&tool);
    // Reading: the figures show "…" and the button waits.
    wait_for("the working state", || {
        texts(&tool).contains("Reading rhythmdb.xml…")
    });
    assert!(!import.is_sensitive());
    assert!(texts(&tool).contains('…'));
    wait_for("the plan", || import.is_sensitive());
    assert_eq!(import.label().as_deref(), Some("Import for 7,904 tracks"));
    let shown = texts(&tool);
    for text in [
        "Rhythmbox",
        "rhythmdb.xml and playlists.xml · 9,120 entries",
        "~/.local/share/rhythmbox",
        "Not found on this computer",
        "~/.var/app/org.gnome.Lollypop",
        "Choose its Library.xml…",
        "8,214",
        "7,900 by path · 314 by tags",
        "880",
        "Show the first 200",
        "nothing new for them",
        "Plays added",
        "48,330",
        "6,102",
        "Your rating kept",
        "Left out by the reader",
        "6 in the file",
        "Late evening",
        "45 tracks, 3 not in your library",
        "added<30d sort:added desc",
        "Left out",
        "Imported before",
        "From this file on 2 Nov 2026",
        "Plays are added to yours. Importing this file again later adds only the plays made since.",
        "Lollypop · 2 Nov 2026",
        "Replaced files · 28 Oct 2026",
        "Undone",
    ] {
        assert!(shown.contains(text), "missing {text:?} in {shown}");
    }
    assert!(
        !shown.contains("Skips added"),
        "rows with nothing are hidden"
    );
    shot(&tool, "Import");
    // The entries left out, in words.
    let show: gtk::Button = named(&tool, "figure-unmatched-show");
    show.emit_clicked();
    wait_for("the list", || {
        popover(&tool, "import-list-popover").is_some()
    });
    let list = popover(&tool, "import-list-popover").unwrap();
    assert!(
        texts(&list).contains("The first 200 of 880"),
        "{}",
        texts(&list)
    );
    assert!(texts(&list).contains("Mari Kask, Departure"));
    shot(&tool, "ImportList");
    list.popdown();
    spin(50);
    assert_idle(&tool);

    // An option plans again.
    let ratings: gtk::CheckButton = named(&tool, "import-replace-ratings");
    ratings.set_active(true);
    wait_for("the plan again", || {
        import.is_sensitive() && !texts(&tool).contains("Your rating kept")
    });
    ratings.set_active(false);
    wait_for("the plan once more", || {
        import.is_sensitive() && texts(&tool).contains("Your rating kept")
    });

    import.emit_clicked();
    let undo: gtk::Button = named(&tool, "import-undo");
    wait_for("the import", || undo.is_visible() && undo.is_sensitive());
    let shown = texts(&tool);
    assert!(shown.contains("What changed"), "{shown}");
    assert!(shown.contains("made beside yours"), "{shown}");
    assert!(shown.contains("Imported today at"), "{shown}");
    assert!(shown.contains("Rhythmbox · today, "), "{shown}");
    assert!(
        shown.contains("Undo takes back what this import added."),
        "{shown}"
    );
    assert!(!import.is_visible(), "no primary button after an import");
    shot(&tool, "ImportDone");
    shot(main.upcast_ref(), "ImportToast");
    assert!(
        main.toast_text()
            .is_some_and(|t| t == "Imported plays and ratings for 7,904 tracks"),
        "{:?}",
        main.toast_text()
    );
    // Ctrl+Z does not undo an import.
    WidgetExt::activate_action(&main, "win.undo", None).unwrap();
    spin(300);
    assert!(undo.is_visible() && undo.is_sensitive());
    assert!(!texts(&tool).contains("Undone on"));

    // An older import cannot be undone before the newer one.
    let runs: gtk::ListBox = named(&tool, "import-runs");
    runs.select_row(runs.row_at_index(1).as_ref());
    wait_for("the older import", || !undo.is_sensitive());
    assert!(texts(&tool).contains("Undo the newer imports first."));
    runs.select_row(runs.row_at_index(3).as_ref());
    wait_for("the undone import", || {
        texts(&tool).contains("Undone on 12 Oct 2026")
    });
    assert!(!undo.is_visible());

    // "Undo this import" on the newest.
    runs.select_row(runs.row_at_index(0).as_ref());
    wait_for("the newest import", || undo.is_sensitive());
    undo.emit_clicked();
    wait_for("the import undone", || texts(&tool).contains("Undone on "));
    assert!(!undo.is_visible());

    // Imported again, the toast's Undo takes it back.
    let from: gtk::ListBox = named(&tool, "import-from");
    from.select_row(from.row_at_index(0).as_ref());
    wait_for("the plan again", || {
        import.is_visible() && import.is_sensitive()
    });
    import.emit_clicked();
    wait_for("the second import", || {
        undo.is_visible() && undo.is_sensitive()
    });
    toast_button(&main, "Undo").emit_clicked();
    wait_for("the undo", || {
        main.toast_text().as_deref() == Some("The import was undone.")
    });
    wait_for("the second import undone", || !undo.is_visible());
    assert!(texts(&tool).contains("Undone on "));
    chooser::replace_folder_chooser(None);
}

/// An iTunes library from another computer matches nothing by path until a
/// folder that moved is added; a new folder that does not exist is refused.
fn folders_that_moved(app: &gtk::Application) {
    let main = window(app);
    choose_file(Some(PathBuf::from("/backup/iTunes/Library.xml")));
    let tool = open_import(&main, Some("itunes"));
    let import = import_button(&tool);
    wait_for("the plan", || import.is_sensitive());
    assert_eq!(import.label().as_deref(), Some("Import for 5,320 tracks"));
    let notice: gtk::Box = named(&tool, "import-moved-notice");
    assert!(notice.is_visible());
    let shown = texts(&tool);
    for text in [
        "None of the 9,120 entries were found by their path.",
        "0 by path · 5,480 by tags",
        "/backup/iTunes/Library.xml",
        "Trips / ",
        "The 25 tracks it held: its rules cannot be read",
        "Skips added",
        "Marked loved",
    ] {
        assert!(shown.contains(text), "missing {text:?} in {shown}");
    }
    shot(&tool, "ImportMoved");

    let add: gtk::Button = named(&tool, "import-add-moved");
    add.emit_clicked();
    wait_for("the popover", || {
        popover(&tool, "import-moved-popover").is_some()
    });
    let moved = popover(&tool, "import-moved-popover").unwrap();
    let was: gtk::Entry = named(&moved, "moved-was");
    let now: gtk::Entry = named(&moved, "moved-now");
    let confirm: gtk::Button = named(&moved, "moved-add");
    assert!(!confirm.is_sensitive(), "Add waits for both folders");
    was.set_text("C:\\Users\\me\\Music");
    assert!(!confirm.is_sensitive());
    now.set_text("/no/such/folder/here");
    assert!(confirm.is_sensitive());
    confirm.emit_clicked();
    spin(50);
    let missing: gtk::Box = named(&moved, "moved-missing");
    assert!(
        missing.is_visible(),
        "a folder that does not exist is reported"
    );
    assert!(texts(&moved).contains("There is no folder /no/such/folder/here."));
    shot(&tool, "ImportMovedMissing");
    let here = std::env::temp_dir();
    now.set_text(&here.to_string_lossy());
    assert!(!missing.is_visible());
    confirm.emit_clicked();
    wait_for("the plan with the pair", || {
        import.is_sensitive() && import.label().as_deref() == Some("Import for 6,700 tracks")
    });
    assert!(!notice.is_visible(), "the notice goes once a pair is added");
    let shown = texts(&tool);
    assert!(shown.contains("2,140 by path · 4,770 by tags"), "{shown}");
    assert!(shown.contains("C:\\Users\\me\\Music → "), "{shown}");
    shot(&tool, "ImportMovedPairs");
    chooser::replace_folder_chooser(None);
}

/// A Helikodu data file shows who saved it and the rows of a data file; a
/// newer file, a file that is not one and a player's file that cannot be read
/// say so; an import made before with nothing new cannot be made again.
fn data_files_and_files_that_cannot_be_read(app: &gtk::Application) {
    let main = window(app);
    choose_file(Some(PathBuf::from("/home/me/helikodu-data.json")));
    let tool = open_import(&main, Some("data"));
    let import = import_button(&tool);
    wait_for("the plan", || import.is_sensitive());
    assert_eq!(import.label().as_deref(), Some("Import for 14,830 tracks"));
    let shown = texts(&tool);
    for text in [
        "helikodu-data.json",
        "Saved by Helikodu 1.0 on 18 Nov 2026 · 18,240 tracks",
        "Play counts raised",
        "Listening history entries added",
        "Labels given",
        "212",
        " · ",
        "Labels left out",
        " · nothing to give them to",
        "Fields defined",
        "4 in the file",
        "96 tracks are added to yours",
        "Fields that differ",
        "the definition here is kept",
        "era",
        "{decade or \"unknown\"}",
        "{decade or \"?\"}",
        "Counts take the higher of the two values, so importing this file twice changes nothing.",
    ] {
        assert!(shown.contains(text), "missing {text:?} in {shown}");
    }
    assert!(
        !shown.contains("Plays added"),
        "counts are raised, not added"
    );
    shot(&tool, "ImportUserData");

    // A newer file.
    choose_file(Some(PathBuf::from("/home/me/newer.json")));
    let another: gtk::Button = named(&tool, "import-choose-another");
    let from: gtk::ListBox = named(&tool, "import-from");
    from.emit_by_name::<()>("row-activated", &[&from.selected_row().unwrap()]);
    wait_for("the newer file", || {
        texts(&tool)
            .contains("This file was saved by a newer Helikodu. Update Helikodu to read it.")
    });
    assert!(!import.is_visible(), "no primary button for a newer file");
    shot(&tool, "ImportNewer");
    // A file that is not a data file.
    choose_file(Some(PathBuf::from("/home/me/notes.txt")));
    another.emit_clicked();
    wait_for("the broken file", || {
        texts(&tool).contains("This file cannot be read as a Helikodu data file.")
    });
    // A player that is not found asks for its file; this one cannot be read.
    choose_file(Some(PathBuf::from("/home/me/strawberry.txt")));
    let strawberry = find::<gtk::ListBoxRow>(&tool, "import-from-strawberry").unwrap();
    from.select_row(Some(&strawberry));
    wait_for("the unreadable library", || {
        texts(&tool).contains("This file cannot be read as a Strawberry library.")
    });
    assert!(texts(&tool).contains("strawberry.txt: the file is not a database"));
    // Cancelling the chooser keeps what was chosen.
    choose_file(None);
    let clementine = find::<gtk::ListBoxRow>(&tool, "import-from-clementine").unwrap();
    from.select_row(Some(&clementine));
    spin(100);
    assert_eq!(from.selected_row(), Some(strawberry));
    // Lollypop was imported before and has nothing new.
    let lollypop = find::<gtk::ListBoxRow>(&tool, "import-from-lollypop").unwrap();
    from.select_row(Some(&lollypop));
    wait_for("the empty plan", || {
        texts(&tool)
            .contains("Nothing to import: this file was imported before and has no new plays.")
    });
    assert!(import.is_visible() && !import.is_sensitive());
    chooser::replace_folder_chooser(None);
}

/// Missing files lists the tracks replaced by another copy; carrying their
/// statistics takes them out of the list as one run, which the Import window
/// lists and the toast's Undo takes back.
fn missing_files_carry_to_replacements(app: &gtk::Application) {
    let main = window(app);
    app.activate_action("missing-files", None);
    wait_for("missing files", || toplevel("missing-files").is_some());
    let tool = toplevel("missing-files").unwrap();
    let replaced: gtk::ListBox = named(&tool, "missing-replaced");
    wait_for("the replaced row", || replaced.is_visible());
    assert!(texts(&tool).contains("Replaced by another copy"));
    replaced.select_row(replaced.row_at_index(0).as_ref());
    let carry: gtk::Button = named(&tool, "replaced-carry");
    wait_for("the replaced page", || carry.is_mapped());
    let shown = texts(&tool);
    for text in [
        "12 missing tracks have a new file of the same recording",
        "The same recording means the same title and artist and a length within 2 seconds.",
        "Track, its old file and its new file",
        "What moves",
        "Shoreline, 6 a.m.",
        "142 plays",
        "96 history entries · 3 playlists",
        "1 of 2 possible files",
    ] {
        assert!(shown.contains(text), "missing {text:?} in {shown}");
    }
    assert_eq!(carry.label().as_deref(), Some("Carry to 12 tracks"));
    let check: gtk::CheckButton = named(&tool, "carry-900001");
    check.set_active(false);
    assert_eq!(carry.label().as_deref(), Some("Carry to 11 tracks"));
    let choice: gtk::DropDown = named(&tool, "carry-choice-900002");
    choice.set_selected(1);
    spin(50);
    assert!(texts(&tool).contains("2 of 2 possible files"));
    shot(&tool, "MissingReplaced");
    assert_idle(&tool);

    carry.emit_clicked();
    wait_for("the carry", || {
        main.toast_text().as_deref() == Some("Carried plays and history to 11 tracks")
    });
    wait_for("the rows to leave", || {
        texts(&tool).contains("1 missing track has a new file of the same recording")
    });
    // The Import window lists it under Earlier imports.
    let import = open_import(&main, None);
    wait_for("the run", || {
        texts(&import).contains("Replaced files · today, ")
    });
    toast_button(&main, "Undo").emit_clicked();
    wait_for("the undo", || {
        texts(&tool).contains("12 missing tracks have a new file of the same recording")
    });
    assert_eq!(carry.label().as_deref(), Some("Carry to 11 tracks"));
}

/// "Export Your Data…" says where the file went, with Show; Settings ›
/// Advanced has the "Your data" row, whose Import… opens the Import window
/// with a data file to choose.
fn your_data_is_exported_and_imported(app: &gtk::Application) {
    let main = window(app);
    main.export_user_data_to(std::env::temp_dir().join("helikodu-data.json"));
    wait_for("the toast", || {
        main.toast_text().as_deref() == Some("Your data was saved to helikodu-data.json")
    });
    let _ = toast_button(&main, "Show");
    let settings = main.open_settings("advanced");
    wait_for("the advanced page", || {
        main.settings_page().as_deref() == Some("advanced")
    });
    let shown = texts(&settings);
    assert!(shown.contains("Your data"), "{shown}");
    assert!(
        shown.contains("Ratings, play counts, history, playlists, labels and fields in one file."),
        "{shown}"
    );
    let _export: gtk::Button = named(&settings, "export-user-data");
    shot(&settings, "SettingsAdvanced");
    shot(main.upcast_ref(), "ExportToast");
    choose_file(Some(PathBuf::from("/home/me/helikodu-data.json")));
    let import: gtk::Button = named(&settings, "import-user-data");
    import.emit_clicked();
    wait_for("the import window", || toplevel("import").is_some());
    let tool = toplevel("import").unwrap();
    wait_for("the data file", || {
        texts(&tool).contains("Saved by Helikodu 1.0 on 18 Nov 2026")
    });
    chooser::replace_folder_chooser(None);
}

type Test = fn(&gtk::Application);

const TESTS: &[(&str, Test)] = &[
    (
        "rhythmbox_is_planned_imported_and_undone",
        rhythmbox_is_planned_imported_and_undone,
    ),
    ("folders_that_moved", folders_that_moved),
    (
        "data_files_and_files_that_cannot_be_read",
        data_files_and_files_that_cannot_be_read,
    ),
    (
        "missing_files_carry_to_replacements",
        missing_files_carry_to_replacements,
    ),
    (
        "your_data_is_exported_and_imported",
        your_data_is_exported_and_imported,
    ),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("interchange: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("interchange: skipped, no display");
        return ExitCode::SUCCESS;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.InterchangeTests")
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
            chooser::replace_folder_chooser(None);
            for window in gtk::Window::list_toplevels() {
                if let Ok(window) = window.downcast::<gtk::Window>() {
                    window.destroy();
                }
            }
            spin(20);
        }
        application.quit();
    });
    application.run_with_args(&["interchange"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("interchange: {} passed", TESTS.len());
        ExitCode::SUCCESS
    } else {
        println!("interchange: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
