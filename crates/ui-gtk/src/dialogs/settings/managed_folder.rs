// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Library, the Managed folder group. While it is off, one row
//! offers "Choose a folder…". While it is on, a bordered list: the folder with
//! "Change…" and "Turn off" (which asks nothing and touches no file), the names
//! the files get (a pattern with "Insert field", an example and what it is
//! for), whether added music is copied or moved, the characters in names, and a
//! footer with "Add music…", "Gather the library into it…" and how many library
//! files are outside the folder. A folder outside the library folders becomes
//! one, and the group says so once.
//!
//! While a run works the footer gives its progress; while a run is cut short
//! the footer says so and "Details…" opens its dialog.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_model::expression::FieldMap;
use helikodu_model::query::Field;
use helikodu_model::settings::ManagedMode;

use super::Context;
use crate::backend::services::{CharacterSet, NamingPattern};
use crate::dialogs::common::{compact_button, dropdown, note, on_selected, row, tilde};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::tools::managed_runs;
use crate::widgets::label;

/// How long the pattern waits for typing to pause before it is used.
const TYPING: Duration = Duration::from_millis(300);

thread_local! {
    /// The folder that became a library folder when it was chosen, and the
    /// Settings window it was chosen in: the group says so while that window
    /// is open.
    static ADDED: RefCell<Option<(glib::WeakRef<gtk::Window>, PathBuf)>> =
        const { RefCell::new(None) };
}

/// The line under the managed folder's row in the Folders group, when `path`
/// is the managed folder: "The managed folder · watched for changes · 17,282
/// tracks".
pub(super) fn folder_line(
    context: &Context,
    folder: &crate::backend::services::LibraryFolder,
) -> Option<String> {
    use crate::backend::services::FolderState;
    let settings = context.settings();
    if settings.library.managed.folder.as_deref() != Some(folder.path.as_path()) {
        return None;
    }
    let how = match folder.state {
        FolderState::Watched => tr("The managed folder · watched for changes"),
        FolderState::Polled(minutes) => fill(
            &ntr(
                "The managed folder · checked every {} minute",
                "The managed folder · checked every {} minutes",
                minutes as usize,
            ),
            &[&minutes.to_string()],
        ),
        FolderState::Manual => tr("The managed folder · scanned when you ask"),
        FolderState::NotConnected => tr("The managed folder · not connected"),
    };
    Some(match folder.tracks {
        Some(n) if folder.state != FolderState::NotConnected => format!(
            "{how} · {}",
            fill(&ntr("{} track", "{} tracks", n), &[&format::count(n)])
        ),
        _ => how,
    })
}

/// The file name `pattern` gives an invented track, as an example.
fn example(pattern: &NamingPattern, characters: CharacterSet) -> String {
    let values = FieldMap::default()
        .with(Field::AlbumArtist, "Mari Kask")
        .with(Field::Artist, "Mari Kask")
        .with(Field::Album, "Tidewater Rooms")
        .with(Field::Title, "Tidewater")
        .with(Field::Track, "1")
        .with(Field::Year, "2024")
        .with(Field::Genre, "Folk")
        .with(Field::Codec, "FLAC");
    let mut path = pattern
        .render_path(&values, characters, &tr("Unknown"), ".flac")
        .to_string_lossy()
        .into_owned();
    path.push_str(".flac");
    path
}

/// Turns the application's actions of the managed folder on or off with it.
fn sync_actions(context: &Context) {
    let on = context.settings().library.managed.is_on();
    if let Some(application) = context.main.upgrade().and_then(|m| m.application()) {
        managed_runs::sync_actions(&application, on);
    }
}

/// Asks for the managed folder and turns the managed folder on there.
fn choose(context: &Rc<Context>) {
    let start = context
        .settings()
        .library
        .managed
        .folder
        .clone()
        .or_else(|| context.settings().library.folders.first().cloned())
        .unwrap_or_else(glib::home_dir);
    let weak = Rc::downgrade(context);
    crate::chooser::select_folder(
        &context.window,
        &tr("Choose the managed folder"),
        Some(&tr("Choose")),
        Some(&start),
        move |path| {
            if let (Some(context), Some(path)) = (weak.upgrade(), path) {
                set_folder(&context, path);
            }
        },
    );
}

/// Makes `folder` the managed folder.
pub(super) fn set_folder(context: &Rc<Context>, folder: PathBuf) {
    let services = context.services.clone();
    let weak = Rc::downgrade(context);
    glib::spawn_future_local(async move {
        let chosen = folder.clone();
        let result = gio::spawn_blocking(move || services.set_managed_folder(&chosen))
            .await
            .unwrap_or_else(|_| {
                Err(crate::backend::BackendError(tr(
                    "The folder could not be used.",
                )))
            });
        let Some(context) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(added) => {
                if added {
                    let stored = context
                        .settings()
                        .library
                        .managed
                        .folder
                        .clone()
                        .unwrap_or(folder);
                    ADDED.with(|a| a.replace(Some((context.window.downgrade(), stored))));
                }
                sync_actions(&context);
                context.rebuild();
            }
            Err(error) => context.toast(&error.0),
        }
    });
}

/// Whether the line "This folder was added to the library's folders." belongs
/// under `folder` in the Settings window of `context`.
fn was_added(context: &Context, folder: &Path) -> bool {
    ADDED.with(|a| {
        a.borrow().as_ref().is_some_and(|(window, added)| {
            added == folder && window.upgrade().as_ref() == Some(&context.window)
        })
    })
}

/// The group while the managed folder is off.
fn off(context: &Rc<Context>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    line.set_widget_name("managed-off");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let title = gtk::Label::new(Some(&tr("Let Helikodu keep one folder in order")));
    title.set_xalign(0.0);
    title.set_wrap(true);
    text.append(&title);
    text.append(&note(&tr(
        "It copies or moves the music you add into that folder and names the files for you. Your other folders stay as they are.",
    )));
    line.append(&text);
    let button = compact_button(&tr("Choose a folder…"));
    button.set_widget_name("managed-choose");
    let weak = Rc::downgrade(context);
    button.connect_clicked(move |_| {
        if let Some(context) = weak.upgrade() {
            choose(&context);
        }
    });
    line.append(&button);
    row(&tr("Managed folder"), None, &line)
}

/// A row of the bordered list: a 150 px label in `t2`, then `control`; the
/// label is at the `top` of a control of several lines.
fn list_row(title: &str, top: bool, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.add_css_class("hk-managed-setting");
    let name = label(title, &["hk-t2", "hk-managed-label"]);
    name.set_size_request(150, -1);
    if top {
        name.set_valign(gtk::Align::Start);
        name.add_css_class("hk-managed-top");
    } else {
        name.set_valign(gtk::Align::Center);
    }
    name.set_ellipsize(gtk::pango::EllipsizeMode::None);
    name.set_wrap(true);
    line.append(&name);
    let control = control.as_ref();
    control.set_hexpand(true);
    control.update_relation(&[gtk::accessible::Relation::LabelledBy(&[name.upcast_ref()])]);
    line.append(control);
    line
}

/// The folder row: the path, what turning it off does, "Change…" and "Turn
/// off".
fn folder_row(context: &Rc<Context>, folder: &Path) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.add_css_class("hk-folder-row");
    let image = gtk::Image::from_icon_name("folder-symbolic");
    image.add_css_class("hk-t2");
    line.append(&image);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let path = label(&tilde(folder), &["hk-mono", "hk-path"]);
    path.set_widget_name("managed-path");
    path.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    text.append(&path);
    text.append(&label(
        &tr("Turning it off leaves every file where it is."),
        &["hk-t3", "hk-secondary"],
    ));
    if was_added(context, folder) {
        let added = label(
            &tr("This folder was added to the library's folders."),
            &["hk-t3", "hk-secondary"],
        );
        added.set_widget_name("managed-added");
        text.append(&added);
    }
    line.append(&text);
    let change = compact_button(&tr("Change…"));
    change.set_widget_name("managed-change");
    let weak = Rc::downgrade(context);
    change.connect_clicked(move |_| {
        if let Some(context) = weak.upgrade() {
            choose(&context);
        }
    });
    line.append(&change);
    let off = compact_button(&tr("Turn off"));
    off.set_widget_name("managed-turn-off");
    let weak = Rc::downgrade(context);
    off.connect_clicked(move |_| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        if context.update(|s| s.library.managed.folder = None) {
            ADDED.with(|a| a.replace(None));
            sync_actions(&context);
            context.rebuild();
        }
    });
    line.append(&off);
    line
}

/// Names: the pattern with "Insert field", an example and what it is for. A
/// pattern that cannot be read is reported under it, and the one stored stays
/// in use.
fn names(context: &Rc<Context>) -> gtk::Box {
    let settings = context.settings();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let pattern = crate::widgets::pattern_entry::PatternEntry::new("managed-pattern");
    let entry = pattern.entry.clone();
    entry.set_text(&settings.library.managed.pattern);
    column.append(&pattern.root);
    let problem = label("", &["hk-query-problem"]);
    problem.set_widget_name("managed-pattern-problem");
    problem.set_wrap(true);
    problem.set_ellipsize(gtk::pango::EllipsizeMode::None);
    problem.set_visible(false);
    column.append(&problem);
    let characters =
        CharacterSet::from_id(&settings.library.managed.characters).unwrap_or_default();
    let shown = label(
        &NamingPattern::parse(&settings.library.managed.pattern)
            .map(|p| example(&p, characters))
            .unwrap_or_default(),
        &["hk-mono", "hk-t3", "hk-managed-example"],
    );
    shown.set_widget_name("managed-example");
    shown.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    column.append(&shown);
    column.append(&note(&tr(
        "For music added from now on. Files already in the folder keep their names; Organize files renames them.",
    )));
    let typing = Rc::new(Cell::new(0_u64));
    let weak = Rc::downgrade(context);
    let (problem_weak, shown_weak) = (problem.downgrade(), shown.downgrade());
    entry.connect_changed(move |entry| {
        let tick = typing.get() + 1;
        typing.set(tick);
        let typing = typing.clone();
        let (weak, problem_weak, shown_weak) =
            (weak.clone(), problem_weak.clone(), shown_weak.clone());
        let entry = entry.downgrade();
        glib::timeout_add_local_once(TYPING, move || {
            if typing.get() != tick {
                return;
            }
            let (Some(context), Some(entry), Some(problem), Some(shown)) = (
                weak.upgrade(),
                entry.upgrade(),
                problem_weak.upgrade(),
                shown_weak.upgrade(),
            ) else {
                return;
            };
            let text = entry.text().to_string();
            match NamingPattern::parse(&text) {
                Ok(pattern) => {
                    entry.remove_css_class("error");
                    problem.set_visible(false);
                    let characters =
                        CharacterSet::from_id(&context.settings().library.managed.characters)
                            .unwrap_or_default();
                    shown.set_text(&example(&pattern, characters));
                    if context.settings().library.managed.pattern != text {
                        context.update(|s| s.library.managed.pattern.clone_from(&text));
                    }
                }
                Err(error) => {
                    entry.add_css_class("error");
                    problem.set_text(&crate::tools::naming_problem(&error));
                    problem.set_visible(true);
                }
            }
        });
    });
    list_row(&tr("Names"), true, &column)
}

/// "When music is added": copy or move.
fn mode(context: &Rc<Context>) -> gtk::Box {
    let settings = context.settings();
    let chosen = usize::from(settings.library.managed.mode == ManagedMode::Move);
    let choice = dropdown(&[tr("Copy the files"), tr("Move the files")], chosen, 200);
    choice.set_widget_name("managed-mode");
    let weak = Rc::downgrade(context);
    on_selected(&choice, move |index| {
        if let Some(context) = weak.upgrade() {
            let mode = if index == 1 {
                ManagedMode::Move
            } else {
                ManagedMode::Copy
            };
            context.update(|s| s.library.managed.mode = mode);
        }
    });
    list_row(&tr("When music is added"), false, &choice)
}

/// "Characters in names", as in Organize files.
fn characters(context: &Rc<Context>, example_label: Option<gtk::Label>) -> gtk::Box {
    let settings = context.settings();
    let current = CharacterSet::from_id(&settings.library.managed.characters).unwrap_or_default();
    let names: Vec<String> = CharacterSet::ALL
        .iter()
        .map(|s| crate::tools::organizer::set_name(*s))
        .collect();
    let index = CharacterSet::ALL
        .iter()
        .position(|s| *s == current)
        .unwrap_or(0);
    let choice = dropdown(&names, index, 260);
    choice.set_widget_name("managed-characters");
    let weak = Rc::downgrade(context);
    on_selected(&choice, move |index| {
        let (Some(context), Some(set)) = (weak.upgrade(), CharacterSet::ALL.get(index)) else {
            return;
        };
        if context.update(|s| set.id().clone_into(&mut s.library.managed.characters))
            && let Some(shown) = example_label.as_ref()
            && let Ok(pattern) = NamingPattern::parse(&context.settings().library.managed.pattern)
        {
            shown.set_text(&example(&pattern, *set));
        }
    });
    list_row(&tr("Characters in names"), false, &choice)
}

/// The footer: "Add music…", "Gather the library into it…" and the files
/// outside; or the progress of a run; or a run cut short with "Details…".
fn footer(context: &Rc<Context>) -> gtk::Box {
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("hk-list-footer");
    footer.add_css_class("hk-managed-footer");
    let add = compact_button(&tr("Add music…"));
    add.set_widget_name("managed-add");
    let gather = compact_button(&tr("Gather the library into it…"));
    gather.set_widget_name("managed-gather");
    let text = label("", &["hk-t3", "hk-secondary"]);
    text.set_widget_name("managed-footer-text");
    text.set_hexpand(true);
    text.set_xalign(1.0);
    text.set_wrap(true);
    text.set_ellipsize(gtk::pango::EllipsizeMode::None);
    let details = compact_button(&tr("Details…"));
    details.set_widget_name("managed-details");
    footer.append(&add);
    footer.append(&gather);
    footer.append(&text);
    footer.append(&details);

    let weak = Rc::downgrade(context);
    add.connect_clicked(move |_| {
        if let Some(main) = weak.upgrade().and_then(|c| c.main.upgrade()) {
            managed_runs::add_music(&main);
        }
    });
    let weak = Rc::downgrade(context);
    gather.connect_clicked(move |_| {
        if let Some(main) = weak.upgrade().and_then(|c| c.main.upgrade()) {
            crate::tools::managed_folder::open_gather(&main);
        }
    });
    let weak = Rc::downgrade(context);
    details.connect_clicked(move |_| {
        if let Some(main) = weak.upgrade().and_then(|c| c.main.upgrade()) {
            crate::tools::managed_cut_short::open(&main);
        }
    });

    // How many library files are outside, read once and after each run.
    let outside: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    let update: Rc<dyn Fn()> = {
        let (add, gather, text, details) = (
            add.downgrade(),
            gather.downgrade(),
            text.downgrade(),
            details.downgrade(),
        );
        let outside = outside.clone();
        Rc::new(move || {
            let (Some(add), Some(gather), Some(text), Some(details)) = (
                add.upgrade(),
                gather.upgrade(),
                text.upgrade(),
                details.upgrade(),
            ) else {
                return;
            };
            let run = managed_runs::current().filter(managed_runs::RunView::is_working);
            let cut_short = managed_runs::interrupted().is_some();
            details.set_visible(cut_short && run.is_none());
            add.set_visible(!cut_short || run.is_some());
            gather.set_visible(!cut_short || run.is_some());
            if let Some(run) = run {
                add.set_sensitive(false);
                gather.set_sensitive(false);
                text.set_text(&managed_runs::progress_text(&run));
                return;
            }
            if cut_short {
                text.set_text(&tr(
                    "A run was cut short. Nothing can be added until it is settled.",
                ));
                return;
            }
            add.set_sensitive(true);
            match outside.get() {
                Some(0) => {
                    gather.set_sensitive(false);
                    text.set_text(&tr("Every library file is in this folder"));
                }
                Some(n) => {
                    gather.set_sensitive(true);
                    text.set_text(&fill(
                        &ntr(
                            "{} library file is outside this folder",
                            "{} library files are outside this folder",
                            n,
                        ),
                        &[&format::count(n)],
                    ));
                }
                None => {
                    gather.set_sensitive(true);
                    text.set_text("");
                }
            }
        })
    };
    let count: Rc<dyn Fn()> = {
        let services = context.services.clone();
        let outside = outside.clone();
        let update = update.clone();
        Rc::new(move || {
            let services = services.clone();
            let outside = outside.clone();
            let update = update.clone();
            glib::spawn_future_local(async move {
                let n = gio::spawn_blocking(move || services.managed_files_outside())
                    .await
                    .ok()
                    .and_then(Result::ok);
                outside.set(n);
                update();
            });
        })
    };
    count();
    update();
    let was_working = Cell::new(managed_runs::is_working());
    let id = managed_runs::listen(move || {
        let working = managed_runs::is_working();
        if was_working.replace(working) && !working {
            count();
        }
        update();
    });
    // Pages are built again while the window is open; their listeners go
    // with the window.
    context
        .window
        .connect_destroy(move |_| managed_runs::unlisten(id));
    footer
}

/// The group while the managed folder is on.
fn on(context: &Rc<Context>, folder: &Path) -> gtk::Box {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("hk-bordered-list");
    list.set_widget_name("managed-on");
    list.append(&folder_row(context, folder));
    let names = names(context);
    let example_label = names.last_child().and_then(|column| {
        let mut child = column.first_child();
        while let Some(widget) = child {
            if widget.widget_name() == "managed-example" {
                return widget.downcast::<gtk::Label>().ok();
            }
            child = widget.next_sibling();
        }
        None
    });
    list.append(&names);
    list.append(&mode(context));
    list.append(&characters(context, example_label));
    list.append(&footer(context));
    row(
        &tr("Managed folder"),
        Some(&tr("Music you add goes here, named by the pattern.")),
        &list,
    )
}

/// The Managed folder group of Settings › Library.
pub(super) fn group(context: &Rc<Context>) -> gtk::Box {
    let settings = context.settings();
    match settings.library.managed.folder.clone() {
        Some(folder) => on(context, &folder),
        None => off(context),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_follows_the_pattern() {
        let pattern =
            NamingPattern::parse("{albumartist}/{album}/[{disc}-]{track} {title}").unwrap();
        assert_eq!(
            example(&pattern, CharacterSet::Windows),
            "Mari Kask/Tidewater Rooms/01 Tidewater.flac"
        );
    }
}
