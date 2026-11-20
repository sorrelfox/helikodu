// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Your fields and the Insert popover against the sample backend: a computed
//! field with its example values and the field that uses it, a field for a tag,
//! a name and an expression that cannot be used, a new field saved and in use
//! at once, the question about unsaved changes, and the Insert popover of a
//! pattern entry and of the expression, with no frames drawn while idle.
//!
//! Like `tools.rs`, this program runs only under the headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test fields
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
use helikodu_model::query::Field;
use helikodu_ui_gtk::backend::sample;
use helikodu_ui_gtk::snapshot;
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

/// Saves `widget` (a window or an open popover) to
/// `HELIKODU_FIELDS_SHOTS/name.png` when that is set.
fn shot(widget: &impl IsA<gtk::Widget>, name: &str) {
    let Some(dir) = std::env::var_os("HELIKODU_FIELDS_SHOTS") else {
        return;
    };
    spin(300);
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    match widget.dynamic_cast_ref::<gtk::Window>() {
        Some(window) => snapshot::save_png(window, &path).unwrap(),
        None => snapshot::save_widget_png(widget, &path).unwrap(),
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

/// Opens Your fields from the Library menu's action and waits for its fields.
fn open_fields(app: &gtk::Application) -> gtk::Window {
    app.activate_action("user-fields", None);
    wait_for("Your fields", || toplevel("user-fields").is_some());
    let tool = toplevel("user-fields").unwrap();
    wait_for("the fields", || {
        find::<gtk::ListBoxRow>(&tool, "field-decade").is_some()
    });
    tool
}

fn choose(tool: &gtk::Window, row: &str) {
    let row: gtk::ListBoxRow = named(tool, row);
    let list = row.parent().and_downcast::<gtk::ListBox>().unwrap();
    list.select_row(Some(&row));
    spin(50);
}

fn value_rows(tool: &gtk::Window) -> u32 {
    let values: gtk::Box = named(tool, "fields-values");
    let mut count = 0;
    let mut child = values.first_child();
    while let Some(c) = child {
        count += 1;
        child = c.next_sibling();
    }
    count
}

fn expression_text(tool: &gtk::Window) -> String {
    let view: gtk::TextView = named(tool, "fields-expression");
    let buffer = view.buffer();
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .to_string()
}

/// A computed field: its expression, the field that uses it (so Remove is
/// insensitive and the footer says why), and example values that follow the
/// expression as it is typed.
fn a_computed_field_shows_values_as_typed(app: &gtk::Application) {
    let main = window(app);
    let tool = open_fields(app);
    let name: gtk::Entry = named(&tool, "fields-name");
    wait_for("decade chosen", || name.text() == "decade");
    assert_eq!(expression_text(&tool), "{left(year,3)}0s");
    let all = texts(&tool);
    assert!(all.contains("Computed · used by the field era"), "{all}");
    assert!(all.contains("The field era uses decade, so it cannot be removed."));
    assert!(all.contains("Fields are kept in the library, not in the files."));
    let remove: gtk::Button = named(&tool, "fields-remove");
    let save: gtk::Button = named(&tool, "fields-save");
    assert!(!remove.is_sensitive());
    // As drawn, Save stays offered while the field can be read.
    assert!(save.is_sensitive());
    assert_eq!(save.label().as_deref(), Some("Save"));
    wait_for("example values", || value_rows(&tool) > 0);
    assert!(texts(&tool).contains("0s"));
    shot(&tool, "fields");

    let view: gtk::TextView = named(&tool, "fields-expression");
    view.buffer().set_text("{upper(left(title, 2))}!");
    wait_for("the new values", || texts(&tool).contains('!'));
    assert!(save.is_sensitive());
    assert_idle(&tool);
    // Choosing another field asks first; Discard leaves the change.
    choose(&tool, "field-era");
    let question = wait_question();
    click(&question, "Discard");
    wait_for("era chosen", || name.text() == "era");
    tool.close();
    main.destroy();
}

/// The question about unsaved changes, once it is up.
fn wait_question() -> gtk::Window {
    let find_question = || {
        gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Window>().ok())
            .find(|w| w.is_visible() && w.is_modal() && texts(w).contains("Save the changes to"))
    };
    wait_for("the question", || find_question().is_some());
    find_question().unwrap()
}

fn click(window: &gtk::Window, label: &str) {
    let button = descendants(window.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.label().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no button {label}"));
    button.emit_clicked();
    spin(50);
}

/// A field for a tag: the tag's entry instead of the expression, its values,
/// and Remove, since no field uses it.
fn a_tag_field_shows_its_tag(app: &gtk::Application) {
    let main = window(app);
    let tool = open_fields(app);
    choose(&tool, "field-mood");
    let tag: gtk::Entry = named(&tool, "fields-tag");
    wait_for("mood chosen", || tag.text() == "MOOD");
    assert!(tag.is_mapped());
    let view: gtk::TextView = named(&tool, "fields-expression");
    assert!(!view.is_mapped());
    let all = texts(&tool);
    assert!(all.contains("A tag of the files"), "{all}");
    assert!(all.contains("Fields are kept in the library and apply at once."));
    assert!(all.contains("are read as the same tag"));
    let remove: gtk::Button = named(&tool, "fields-remove");
    assert!(remove.is_sensitive());
    let tagged: gtk::ToggleButton = named(&tool, "fields-kind-tag");
    assert!(tagged.is_active());
    wait_for("the moods", || {
        let all = texts(&tool);
        all.contains("calm") || all.contains("solemn")
    });
    shot(&tool, "fields-tag");
    tool.close();
    main.destroy();
}

/// A new field with a name Helikodu has and an expression that cannot be
/// read: both are explained under their entries, the example values give way
/// to a line, and nothing can be saved.
fn errors_are_explained_and_block_saving(app: &gtk::Application) {
    let main = window(app);
    let tool = open_fields(app);
    choose(&tool, "new-field");
    let name: gtk::Entry = named(&tool, "fields-name");
    wait_for("a new field", || {
        name.text().is_empty() && texts(&tool).contains("Not saved yet")
    });
    let save: gtk::Button = named(&tool, "fields-save");
    assert_eq!(save.label().as_deref(), Some("Add field"));
    assert!(!save.is_sensitive());
    let remove: gtk::Button = named(&tool, "fields-remove");
    assert!(!remove.is_visible());
    name.set_text("year");
    let view: gtk::TextView = named(&tool, "fields-expression");
    view.buffer().set_text("{if(year > 1999, \"new\", \"old\"}");
    spin(300);
    let name_notice: gtk::Box = named(&tool, "fields-name-notice");
    let expression_notice: gtk::Box = named(&tool, "fields-expression-notice");
    assert!(name_notice.is_visible());
    assert!(expression_notice.is_visible());
    let all = texts(&tool);
    assert!(
        all.contains("year is a field Helikodu already has."),
        "{all}"
    );
    assert!(all.contains("A name has the letters a to z and _"));
    assert!(all.contains("A parenthesis is not closed: at character 4."));
    assert!(all.contains("Nothing is saved while there is a problem."));
    assert!(
        all.contains("The example values come back when the name and the expression can be read.")
    );
    assert!(!save.is_sensitive());
    shot(&tool, "fields-error");
    // A field that comes back to itself.
    name.set_text("loop");
    view.buffer().set_text("{loop}");
    spin(100);
    assert!(
        texts(&tool).contains("loop uses itself."),
        "{}",
        texts(&tool)
    );
    // Closing asks about the unsaved field; Cancel keeps the window.
    tool.close();
    let question = wait_question();
    click(&question, "Cancel");
    assert!(tool.is_visible());
    tool.destroy();
    main.destroy();
}

/// A new field saved is in the list and in use at once; a name another field
/// needs cannot change.
fn a_saved_field_is_in_use_at_once(app: &gtk::Application) {
    let main = window(app);
    let tool = open_fields(app);
    choose(&tool, "new-field");
    let name: gtk::Entry = named(&tool, "fields-name");
    name.set_text("first_letter");
    let view: gtk::TextView = named(&tool, "fields-expression");
    view.buffer().set_text("{initial(artist)}");
    let save: gtk::Button = named(&tool, "fields-save");
    wait_for("Add field", || save.is_sensitive());
    save.emit_clicked();
    wait_for("the saved field", || {
        find::<gtk::ListBoxRow>(&tool, "field-first_letter").is_some()
    });
    assert_eq!(
        Field::from_name("first_letter"),
        Some(Field::Computed("first_letter".into()))
    );
    wait_for("chosen after saving", || {
        texts(&tool).contains("Computed") && save.label().as_deref() == Some("Save")
    });
    // decade cannot be renamed while era uses it.
    choose(&tool, "field-decade");
    wait_for("decade chosen", || name.text() == "decade");
    name.set_text("years");
    spin(50);
    assert!(
        texts(&tool).contains("The field era uses decade, so the name cannot change."),
        "{}",
        texts(&tool)
    );
    assert!(!save.is_sensitive());
    name.set_text("decade");
    // The new field can go again.
    choose(&tool, "field-first_letter");
    let remove: gtk::Button = named(&tool, "fields-remove");
    wait_for("Remove offered", || {
        remove.is_sensitive() && remove.is_visible()
    });
    remove.emit_clicked();
    let confirm = {
        let find_confirm = || {
            gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Window>().ok())
                .find(|w| w.is_visible() && texts(w).contains("Remove the field first_letter?"))
        };
        wait_for("the confirmation", || find_confirm().is_some());
        find_confirm().unwrap()
    };
    click(&confirm, "Remove");
    wait_for("the field gone", || {
        find::<gtk::ListBoxRow>(&tool, "field-first_letter").is_none()
    });
    assert!(Field::from_name("first_letter").is_none());
    tool.close();
    main.destroy();
}

/// The popover of a pattern entry: the three sections, the search, nothing
/// found, and a function inserted at the cursor with its first argument
/// selected.
fn the_insert_popover_inserts_at_the_cursor(app: &gtk::Application) {
    let main = window(app);
    app.activate_action("organize-files", None);
    wait_for("Organize files", || toplevel("organize-files").is_some());
    let organizer = toplevel("organize-files").unwrap();
    let entry: gtk::Entry = named(&organizer, "organize-pattern");
    let insert: gtk::MenuButton = named(&organizer, "organize-pattern-insert");
    // The saved pattern fills the entry first.
    let view: gtk::ListView = named(&organizer, "organize-table");
    wait_for("the preview", || {
        view.model().is_some_and(|m| m.n_items() > 0)
    });
    entry.set_text("{albumartist}/");
    entry.set_position(-1);
    insert.popup();
    let popover = insert.popover().unwrap();
    wait_for("the popover", || popover.is_visible());
    let all = texts(&popover);
    for part in [
        "Fields",
        "Your fields",
        "Functions",
        "decade",
        "Computed · {left(year,3)}0s",
        "The tag MOOD",
        "if(test, then, else)",
        "Expressions in the user guide",
    ] {
        assert!(all.contains(part), "{part} missing:\n{all}");
    }
    shot(&organizer, "insert-window");
    shot(&popover, "insert");
    let search: gtk::SearchEntry = named(&popover, "insert-search");
    search.set_text("zzzz");
    wait_for("nothing found", || {
        texts(&popover).contains("No field or function has this name.")
    });
    // decade's expression has "left" too.
    search.set_text("left(text");
    // Setting the text empties the entry first, which shows everything for a
    // moment.
    wait_for("left found", || {
        let all = texts(&popover);
        all.contains("left(text, n)") && !all.contains("Album artist") && !all.contains("decade")
    });
    search.emit_by_name::<()>("activate", &[]);
    let end = Instant::now() + Duration::from_secs(5);
    while entry.text() != "{albumartist}/{left(text, n)}" {
        assert!(
            Instant::now() < end,
            "the call is not inserted: {}",
            entry.text()
        );
        spin(10);
    }
    spin(50);
    let (start, end) = entry.selection_bounds().unwrap();
    assert_eq!((start, end), (20, 24));
    assert!(!popover.is_visible());
    // A field typed over the selected argument goes in without braces.
    insert.popup();
    wait_for("the popover again", || popover.is_visible());
    let search: gtk::SearchEntry = named(&popover, "insert-search");
    search.set_text("albumartist");
    wait_for("albumartist found", || {
        !texts(&popover).contains("Track artist")
    });
    search.emit_by_name::<()>("activate", &[]);
    wait_for("the field inserted", || {
        entry.text() == "{albumartist}/{left(albumartist, n)}"
    });
    organizer.close();
    main.destroy();
}

/// The expression of Your fields has the same popover; a field inside braces
/// goes in without them.
fn the_expression_has_the_insert_popover(app: &gtk::Application) {
    let main = window(app);
    let tool = open_fields(app);
    let view: gtk::TextView = named(&tool, "fields-expression");
    wait_for("decade chosen", || {
        expression_text(&tool) == "{left(year,3)}0s"
    });
    let buffer = view.buffer();
    buffer.set_text("{concat(");
    buffer.place_cursor(&buffer.end_iter());
    let insert: gtk::MenuButton = named(&tool, "fields-insert");
    insert.popup();
    let popover = insert.popover().unwrap();
    wait_for("the popover", || popover.is_visible());
    let search: gtk::SearchEntry = named(&popover, "insert-search");
    search.set_text("track artist");
    wait_for("one choice", || !texts(&popover).contains("Album title"));
    search.emit_by_name::<()>("activate", &[]);
    wait_for("artist inserted", || {
        expression_text(&tool) == "{concat(artist"
    });
    insert.popdown();
    tool.destroy();
    main.destroy();
}

/// "Your Fields…" is an action of the application, as the Library menu has it.
fn the_library_menu_opens_your_fields(app: &gtk::Application) {
    let main = window(app);
    assert!(app.lookup_action("user-fields").is_some());
    let tool = open_fields(app);
    assert_eq!(tool.title().as_deref(), Some("Your fields"));
    tool.close();
    main.destroy();
}

type Test = fn(&gtk::Application);

const TESTS: &[(&str, Test)] = &[
    (
        "a_computed_field_shows_values_as_typed",
        a_computed_field_shows_values_as_typed,
    ),
    ("a_tag_field_shows_its_tag", a_tag_field_shows_its_tag),
    (
        "errors_are_explained_and_block_saving",
        errors_are_explained_and_block_saving,
    ),
    (
        "a_saved_field_is_in_use_at_once",
        a_saved_field_is_in_use_at_once,
    ),
    (
        "the_insert_popover_inserts_at_the_cursor",
        the_insert_popover_inserts_at_the_cursor,
    ),
    (
        "the_expression_has_the_insert_popover",
        the_expression_has_the_insert_popover,
    ),
    (
        "the_library_menu_opens_your_fields",
        the_library_menu_opens_your_fields,
    ),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("fields: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("fields: skipped, no display");
        return ExitCode::SUCCESS;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.FieldTests")
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
    application.run_with_args(&["fields"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("fields: {} passed", TESTS.len());
        ExitCode::SUCCESS
    } else {
        println!("fields: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
