// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Organize files: the saved patterns at the left; the pattern with "Insert
//! field", where the files go and which characters names may hold; a preview
//! of every file, the path now over the path after the move, with what happens
//! to it; and the footer with the counts, "Undo the last run" and "Move N
//! files". The preview follows the pattern as it is typed, and nothing moves
//! before the button is used. Every move is written to the journal first, so a
//! run can be undone from here, from the toast in the main window, or later.
//!
//! The table is a list view over the preview: a single click selects a row,
//! Space ticks it, and a double click or Enter plays its track.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::file_tools::{
    WorkBar, choose_folder, mono_row, problems_box, relative, run_in_background, run_summary,
    toast_run,
};
use super::tool_window::{CHECK, Frame, GAP, footer, frame, primary_button, table_header};
use crate::backend::services::{
    CancelToken, Change, CharacterSet, Destination, FileId, FileRunSummary, MoveRequest,
    OrganizeOptions, PlannedMove, SavedPattern, Services,
};
use crate::backend::{Backend, PlayerRequest, TrackKey};
use crate::dialogs::common::{compact_button, dropdown, find_open, on_selected, tilde};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "organize-files";

/// The width of the Change column.
const CHANGE: i32 = 150;

/// How long the preview waits for typing to pause.
const TYPING: Duration = Duration::from_millis(250);

/// The names of the patterns the library starts with, for their translations.
fn pattern_name(name: &str) -> String {
    match name {
        "Artist, album, track" => tr("Artist, album, track"),
        "With year and disc" => tr("With year and disc"),
        "Classical" => tr("Classical"),
        "One folder for everything" => tr("One folder for everything"),
        other => other.to_owned(),
    }
}

/// The character sets, in the order of their dropdown.
const SETS: [CharacterSet; 3] = CharacterSet::ALL;

pub(crate) fn set_name(set: CharacterSet) -> String {
    match set {
        CharacterSet::AsIs => tr("Names as they are"),
        CharacterSet::Ascii => tr("Plain ASCII only"),
        _ => tr("Names that Windows can read"),
    }
}

fn change_text(change: &Change) -> String {
    match change {
        Change::Move => tr("Move"),
        Change::Rename => tr("Rename"),
        Change::MoveAndRename => tr("Move and rename"),
        Change::InPlace => tr("Already in place"),
        Change::NameTaken => tr("Name taken"),
        Change::Missing => tr("Missing"),
        Change::CueSheet => tr("Kept for its CUE sheet"),
        _ => String::new(),
    }
}

fn change_tooltip(change: &Change) -> Option<String> {
    match change {
        Change::NameTaken => Some(tr(
            "A file with that name is already there, or another file here gets the same name",
        )),
        Change::Missing => Some(tr("The file is missing, so it cannot move")),
        Change::CueSheet => Some(tr("A CUE sheet refers to this file by its name")),
        _ => None,
    }
}

struct Organizer {
    window: gtk::Window,
    backend: Backend,
    tracks: Option<Vec<TrackKey>>,
    roots: RefCell<Vec<PathBuf>>,
    patterns: RefCell<Vec<SavedPattern>>,
    chosen: Cell<Option<i64>>,
    planned: RefCell<Vec<PlannedMove>>,
    ticked: RefCell<HashSet<FileId>>,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    list: gtk::ListBox,
    pattern: gtk::Entry,
    pattern_note: gtk::Label,
    into: gtk::DropDown,
    into_folders: RefCell<Vec<Option<PathBuf>>>,
    chosen_folder: RefCell<Option<PathBuf>>,
    characters: gtk::DropDown,
    companions: gtk::CheckButton,
    title: gtk::Label,
    header_summary: gtk::Label,
    summary: gtk::Label,
    footer: gtk::Box,
    work: WorkBar,
    undo: gtk::Button,
    run: gtk::Button,
    results: gtk::Box,
    empty: gtk::Label,
    stack: gtk::Stack,
    last: RefCell<Option<FileRunSummary>>,
    generation: Cell<u64>,
    typing: Cell<u64>,
    filling: Cell<bool>,
    busy: Cell<bool>,
    cancel: RefCell<Option<CancelToken>>,
}

impl Organizer {
    fn services(&self) -> Arc<dyn Services> {
        self.backend.services()
    }

    fn options(&self) -> OrganizeOptions {
        let into_index = self.into.selected() as usize;
        let into = self
            .into_folders
            .borrow()
            .get(into_index)
            .cloned()
            .flatten()
            .map_or(Destination::OwnLibraryFolder, Destination::Folder);
        OrganizeOptions {
            pattern: self.pattern.text().to_string(),
            into,
            characters: SETS
                .get(self.characters.selected() as usize)
                .copied()
                .unwrap_or_default(),
            unknown: tr("Unknown"),
        }
    }

    /// Fills the "Into" dropdown: each file's own folder, each library folder,
    /// a folder chosen before, and "Choose a folder…".
    fn fill_into(&self) {
        let mut items = vec![tr("The library folder each file is in")];
        let mut folders = vec![None];
        for root in self.roots.borrow().iter() {
            items.push(tilde(root));
            folders.push(Some(root.clone()));
        }
        if let Some(chosen) = self.chosen_folder.borrow().as_ref()
            && !folders.contains(&Some(chosen.clone()))
        {
            items.push(tilde(chosen));
            folders.push(Some(chosen.clone()));
        }
        items.push(tr("Choose a folder…"));
        folders.push(None);
        let selected = self
            .chosen_folder
            .borrow()
            .as_ref()
            .and_then(|c| folders.iter().position(|f| f.as_ref() == Some(c)))
            .unwrap_or(0);
        self.filling.set(true);
        let strings: Vec<&str> = items.iter().map(String::as_str).collect();
        self.into.set_model(Some(&gtk::StringList::new(&strings)));
        self.into.set_selected(u32::try_from(selected).unwrap_or(0));
        self.filling.set(false);
        self.into_folders.replace(folders);
    }

    /// Shows the saved patterns, choosing `choose` (or the first).
    fn fill_patterns(self: &Rc<Self>, choose: Option<i64>) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        for saved in self.patterns.borrow().iter() {
            let (row, _, _) = mono_row(&pattern_name(&saved.name), &saved.pattern);
            row.set_widget_name(&format!("pattern-{}", saved.id));
            self.list.append(&row);
        }
        let (row, _) = new_pattern_row();
        self.list.append(&row);
        let index = choose
            .and_then(|id| self.patterns.borrow().iter().position(|p| p.id == id))
            .unwrap_or(0);
        if let Some(row) = self.list.row_at_index(i32::try_from(index).unwrap_or(0)) {
            self.list.select_row(Some(&row));
        }
    }

    fn load_patterns(self: &Rc<Self>, choose: Option<i64>) {
        let services = self.services();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (sender, receiver) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = sender.send_blocking((
                    services.naming_patterns(),
                    services.library_folders(),
                    services.last_file_run(),
                ));
            });
            let Ok((patterns, folders, last)) = receiver.recv().await else {
                return;
            };
            let Some(this) = weak.upgrade() else { return };
            this.roots.replace(
                folders
                    .map(|f| f.into_iter().map(|f| f.path).collect())
                    .unwrap_or_default(),
            );
            this.fill_into();
            this.patterns.replace(patterns.unwrap_or_default());
            this.last.replace(last.ok().flatten());
            this.update_undo();
            this.fill_patterns(choose);
        });
    }

    /// The chosen pattern was picked in the list.
    fn choose(self: &Rc<Self>, index: usize) {
        let saved = self.patterns.borrow().get(index).cloned();
        match saved {
            Some(saved) => {
                self.chosen.set(Some(saved.id));
                self.title.set_text(&pattern_name(&saved.name));
                if self.pattern.text() != saved.pattern {
                    self.filling.set(true);
                    self.pattern.set_text(&saved.pattern);
                    self.filling.set(false);
                }
                self.preview();
            }
            None => self.new_pattern(),
        }
    }

    /// "New pattern": a copy of the pattern in the entry, at the end of the list.
    fn new_pattern(self: &Rc<Self>) {
        let services = self.services();
        let count = self.patterns.borrow().len() + 1;
        let name = fill(&tr("Pattern {}"), &[&count.to_string()]);
        let text = self.pattern.text().to_string();
        let text = if text.trim().is_empty() {
            crate::backend::services::NamingPattern::DEFAULT.to_owned()
        } else {
            text
        };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (sender, receiver) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = sender.send_blocking(services.save_naming_pattern(None, &name, &text));
            });
            let Ok(saved) = receiver.recv().await else {
                return;
            };
            let Some(this) = weak.upgrade() else { return };
            match saved {
                Ok(id) => this.load_patterns(Some(id)),
                Err(error) => this.pattern_note.set_text(&error.0),
            }
        });
    }

    /// The saved pattern of a row of the list.
    fn saved_of(&self, row: &gtk::ListBoxRow) -> Option<SavedPattern> {
        let name = row.widget_name();
        let id: i64 = name.strip_prefix("pattern-")?.parse().ok()?;
        self.patterns.borrow().iter().find(|p| p.id == id).cloned()
    }

    /// Asks for a new name of the pattern of `row`.
    fn rename_pattern(self: &Rc<Self>, row: &gtk::ListBoxRow) {
        let Some(saved) = self.saved_of(row) else {
            return;
        };
        let weak = Rc::downgrade(self);
        crate::window::playlists::ask_name(
            row.upcast_ref(),
            &pattern_name(&saved.name),
            &tr("Pattern name"),
            move |name| {
                let Some(this) = weak.upgrade() else { return };
                let services = this.services();
                let pattern = saved.pattern.clone();
                this.change_patterns(Some(saved.id), move || {
                    services.save_naming_pattern(Some(saved.id), &name, &pattern)
                });
            },
        );
    }

    /// Asks before deleting the pattern of `row`.
    fn delete_pattern(self: &Rc<Self>, row: &gtk::ListBoxRow) {
        let Some(saved) = self.saved_of(row) else {
            return;
        };
        let weak = Rc::downgrade(self);
        crate::tools::confirm_delete(
            &self.window,
            &fill(
                &tr("Delete the pattern “{}”?"),
                &[&pattern_name(&saved.name)],
            ),
            &tr("Files it organized stay where they are."),
            move || {
                let Some(this) = weak.upgrade() else { return };
                let services = this.services();
                let keep = this.chosen.get().filter(|id| *id != saved.id);
                this.change_patterns(keep, move || {
                    services.delete_naming_pattern(saved.id).map(|()| 0)
                });
            },
        );
    }

    /// Runs `change` on a thread of its own, then lists the patterns again
    /// with `choose` chosen; a failure is shown under the pattern.
    fn change_patterns(
        self: &Rc<Self>,
        choose: Option<i64>,
        change: impl FnOnce() -> Result<i64, crate::backend::BackendError> + Send + 'static,
    ) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (sender, receiver) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = sender.send_blocking(change());
            });
            let Ok(result) = receiver.recv().await else {
                return;
            };
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(_) => this.load_patterns(choose),
                Err(error) => this.pattern_note.set_text(&error.0),
            }
        });
    }

    /// Keeps the edited text with the chosen pattern.
    fn save_pattern(&self) {
        let Some(id) = self.chosen.get() else { return };
        let text = self.pattern.text().to_string();
        let name = {
            let mut patterns = self.patterns.borrow_mut();
            let Some(saved) = patterns.iter_mut().find(|p| p.id == id) else {
                return;
            };
            if saved.pattern == text {
                return;
            }
            saved.pattern.clone_from(&text);
            saved.name.clone()
        };
        let mut row = self.list.first_child();
        while let Some(widget) = row {
            row = widget.next_sibling();
            if widget.widget_name() == format!("pattern-{id}") {
                let line = widget
                    .first_child()
                    .and_then(|b| b.last_child())
                    .and_downcast::<gtk::Label>();
                if let Some(line) = line {
                    line.set_text(&text);
                }
            }
        }
        let services = self.services();
        std::thread::spawn(move || {
            let _ = services.save_naming_pattern(Some(id), &name, &text);
        });
    }

    /// The pattern changed: a new preview once typing pauses.
    fn typed(self: &Rc<Self>) {
        if self.filling.get() {
            return;
        }
        let tick = self.typing.get() + 1;
        self.typing.set(tick);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(TYPING, move || {
            if let Some(this) = weak.upgrade()
                && this.typing.get() == tick
            {
                this.preview();
            }
        });
    }

    /// Asks for the preview of the current options.
    fn preview(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        let options = self.options();
        // A pattern that cannot be read is reported at once, under the entry.
        if let Err(error) = crate::backend::services::NamingPattern::parse(&options.pattern) {
            self.pattern.add_css_class("error");
            crate::widgets::pattern_entry::underline(&self.pattern, Some((error.start, error.end)));
            self.pattern_note
                .set_text(&crate::tools::naming_problem(&error));
            self.pattern_note.add_css_class("hk-query-problem");
            self.planned.borrow_mut().clear();
            self.rebuild();
            return;
        }
        self.pattern.remove_css_class("error");
        crate::widgets::pattern_entry::underline(&self.pattern, None);
        self.pattern_note.remove_css_class("hk-query-problem");
        self.pattern_note.set_text(&tr(
            "Text in square brackets is left out when a field inside it is empty. The file extension is kept.",
        ));
        self.save_pattern();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let services = self.services();
        let tracks = self.tracks.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (sender, receiver) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = sender.send_blocking(services.plan_organize(tracks.as_deref(), &options));
            });
            let Ok(planned) = receiver.recv().await else {
                return;
            };
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            match planned {
                Ok(planned) => {
                    this.ticked.replace(
                        planned
                            .iter()
                            .filter(|p| p.change.moves())
                            .map(|p| p.file)
                            .collect(),
                    );
                    this.planned.replace(planned);
                }
                Err(error) => {
                    this.pattern_note.set_text(&error.0);
                    this.planned.borrow_mut().clear();
                }
            }
            this.rebuild();
        });
    }

    fn rebuild(&self) {
        let count = self.planned.borrow().len();
        let items: Vec<glib::BoxedAnyObject> = (0..count).map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &items);
        self.stack
            .set_visible_child_name(if count == 0 { "empty" } else { "table" });
        self.empty
            .set_text(&if self.pattern.has_css_class("error") {
                tr("The preview waits for a pattern that can be read.")
            } else {
                tr("There are no files to organize.")
            });
        let files = fill(&ntr("{} file", "{} files", count), &[&format::count(count)]);
        let place = match (&self.tracks, self.roots.borrow().as_slice()) {
            (Some(_), _) => tr("the selected tracks"),
            (None, [one]) => fill(&tr("in {}"), &[&tilde(one)]),
            (None, _) => tr("the whole library"),
        };
        self.header_summary
            .set_text(&fill(&tr("{} · {}"), &[&files, &place]));
        self.update_footer();
    }

    fn update_footer(&self) {
        let planned = self.planned.borrow();
        let ticked = self.ticked.borrow();
        let moving = planned
            .iter()
            .filter(|p| p.change.moves() && ticked.contains(&p.file))
            .count();
        let in_place = planned
            .iter()
            .filter(|p| p.change == Change::InPlace)
            .count();
        let taken = planned
            .iter()
            .filter(|p| p.change == Change::NameTaken)
            .count();
        let mut parts = vec![fill(
            &ntr("{} file will move", "{} files will move", moving),
            &[&format::count(moving)],
        )];
        parts.push(fill(
            &ntr(
                "{} is already in place",
                "{} are already in place",
                in_place,
            ),
            &[&format::count(in_place)],
        ));
        if taken > 0 {
            parts.push(fill(
                &ntr("{} name is taken", "{} names are taken", taken),
                &[&format::count(taken)],
            ));
        }
        self.summary.set_text(&parts.join(" · "));
        self.run.set_label(&fill(
            &ntr("Move {} file", "Move {} files", moving),
            &[&format::count(moving)],
        ));
        self.run.set_sensitive(moving > 0 && !self.busy.get());
    }

    fn update_undo(&self) {
        let last = self.last.borrow();
        let (label, sensitive) = match last.as_ref() {
            Some(run) if run.can_undo() => (tr("Undo the last run"), true),
            Some(run) if run.can_redo() => (tr("Redo the last run"), true),
            _ => (tr("Undo the last run"), false),
        };
        self.undo.set_label(&label);
        self.undo.set_sensitive(sensitive && !self.busy.get());
    }

    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        self.footer.set_visible(!busy);
        self.work.root.set_visible(busy);
        if busy {
            self.work.show(0, 0);
        }
        self.update_footer();
        self.update_undo();
    }

    fn main_window(&self) -> Option<MainWindow> {
        self.window.transient_for().and_downcast::<MainWindow>()
    }

    fn show_results(&self, report: &crate::backend::services::RunReport) {
        while let Some(child) = self.results.first_child() {
            self.results.remove(&child);
        }
        if report.problems().next().is_some() {
            self.results
                .append(&problems_box(report, &self.roots.borrow()));
            self.results.set_visible(true);
        } else {
            self.results.set_visible(false);
        }
    }

    /// "Move N files": the ticked rows, on a thread of their own.
    fn start(self: &Rc<Self>) {
        let moves: Vec<MoveRequest> = {
            let ticked = self.ticked.borrow();
            self.planned
                .borrow()
                .iter()
                .filter(|p| p.change.moves() && ticked.contains(&p.file))
                .map(|p| MoveRequest {
                    file: p.file,
                    from: p.from.clone(),
                    to: p.to.clone(),
                })
                .collect()
        };
        if moves.is_empty() {
            return;
        }
        let pattern = self.pattern.text().to_string();
        let companions = self.companions.is_active();
        let cancel = CancelToken::new();
        self.cancel.replace(Some(cancel.clone()));
        self.set_busy(true);
        let services = self.services();
        let weak = Rc::downgrade(self);
        let weak_done = Rc::downgrade(self);
        run_in_background(
            move |progress| {
                let report = |d: usize, t: usize| {
                    let _ = progress.send_blocking((d, t));
                };
                services.organize(&moves, Some(&pattern), companions, &report, &cancel)
            },
            move |done, total| {
                if let Some(this) = weak.upgrade() {
                    this.work.show(done, total);
                }
            },
            move |result| {
                let Some(this) = weak_done.upgrade() else {
                    return;
                };
                this.cancel.replace(None);
                this.set_busy(false);
                match result {
                    Ok(report) => {
                        this.show_results(&report);
                        if let (Some(run), Some(main)) = (report.run, this.main_window()) {
                            let refresh = this.refresher();
                            toast_run(
                                &main,
                                this.services(),
                                run,
                                &run_summary(&report, None),
                                false,
                                refresh,
                            );
                        }
                    }
                    Err(error) => this.summary.set_text(&error.0),
                }
                this.after_change();
            },
        );
    }

    /// What runs after the journal changed elsewhere: a fresh preview and
    /// "Undo the last run".
    fn refresher(self: &Rc<Self>) -> Rc<dyn Fn()> {
        let weak = Rc::downgrade(self);
        Rc::new(move || {
            if let Some(this) = weak.upgrade() {
                this.after_change();
            }
        })
    }

    fn after_change(self: &Rc<Self>) {
        let services = self.services();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (sender, receiver) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = sender.send_blocking(services.last_file_run());
            });
            let Ok(last) = receiver.recv().await else {
                return;
            };
            let Some(this) = weak.upgrade() else { return };
            this.last.replace(last.ok().flatten());
            this.update_undo();
            this.preview();
        });
    }

    /// "Undo the last run" (or redo it after an undo).
    fn step(self: &Rc<Self>) {
        let Some(last) = self.last.borrow().clone() else {
            return;
        };
        let redo = !last.can_undo() && last.can_redo();
        self.set_busy(true);
        let services = self.services();
        let weak = Rc::downgrade(self);
        let weak_done = Rc::downgrade(self);
        let run = last.run;
        run_in_background(
            move |progress| {
                let report = |d: usize, t: usize| {
                    let _ = progress.send_blocking((d, t));
                };
                services.step_file_run(run, redo, &report)
            },
            move |done, total| {
                if let Some(this) = weak.upgrade() {
                    this.work.show(done, total);
                }
            },
            move |result| {
                let Some(this) = weak_done.upgrade() else {
                    return;
                };
                this.set_busy(false);
                match result {
                    Ok(report) => {
                        this.show_results(&report);
                        if let Some(main) = this.main_window() {
                            let refresh = this.refresher();
                            toast_run(
                                &main,
                                this.services(),
                                run,
                                &run_summary(&report, Some(!redo)),
                                !redo,
                                refresh,
                            );
                        }
                    }
                    Err(error) => this.summary.set_text(&error.0),
                }
                this.after_change();
            },
        );
    }

    fn play(&self, position: u32) {
        let Some(track) = self
            .planned
            .borrow()
            .get(position as usize)
            .map(|p| p.track)
        else {
            return;
        };
        self.backend.send(PlayerRequest::PlayTracks {
            tracks: vec![TrackKey(track.get())],
            start: 0,
            title: tr("Organize files"),
            shuffle: false,
        });
    }

    fn set_ticked(&self, file: FileId, on: bool) {
        if on {
            self.ticked.borrow_mut().insert(file);
        } else {
            self.ticked.borrow_mut().remove(&file);
        }
        self.update_footer();
    }

    /// Space ticks or clears the selected row.
    fn toggle_selected(&self) -> bool {
        let position = self.selection.selected();
        let Some(planned) = self.planned.borrow().get(position as usize).cloned() else {
            return false;
        };
        if planned.change.moves() {
            let on = !self.ticked.borrow().contains(&planned.file);
            self.set_ticked(planned.file, on);
            self.store.items_changed(position, 1, 1);
        }
        true
    }

    fn bind(&self, row: &gtk::Box, item: &gtk::ListItem, index: usize) {
        let Some(planned) = self.planned.borrow().get(index).cloned() else {
            return;
        };
        let roots = self.roots.borrow();
        let now = relative(&planned.from, &roots);
        let after = relative(&planned.to, &roots);
        let cells = children(row);
        let [check_cell, paths, change] = cells.as_slice() else {
            return;
        };
        let movable = planned.change.moves();
        let ticked = movable && self.ticked.borrow().contains(&planned.file);
        if let Some(check) = check_cell.first_child().and_downcast::<gtk::CheckButton>() {
            self.filling.set(true);
            check.set_widget_name(&format!("move-{}", planned.file));
            check.set_sensitive(movable);
            check.set_active(ticked);
            check.update_property(&[gtk::accessible::Property::Label(&fill(
                &tr("Move {}"),
                &[&now],
            ))]);
            self.filling.set(false);
        }
        let lines = children(paths);
        if let [top, bottom] = lines.as_slice() {
            set_label(top, &now);
            set_label(bottom, &after);
            top.set_tooltip_text(Some(&now));
            bottom.set_tooltip_text(Some(&after));
        }
        let text = change_text(&planned.change);
        let parts = children(change);
        if let [icon, words] = parts.as_slice() {
            let taken = planned.change == Change::NameTaken;
            icon.set_visible(taken || planned.change == Change::Missing);
            set_label(words, &text);
            for class in ["hk-t3", "hk-strong", "hk-t2"] {
                words.remove_css_class(class);
            }
            match planned.change {
                Change::NameTaken => words.add_css_class("hk-strong"),
                Change::InPlace | Change::Missing | Change::CueSheet => {
                    words.add_css_class("hk-t3");
                }
                _ => words.add_css_class("hk-t2"),
            }
            change.set_tooltip_text(change_tooltip(&planned.change).as_deref());
        }
        item.set_accessible_label(&fill(
            &tr("{}, after the move {}, {}"),
            &[&now, &after, &text],
        ));
    }
}

fn children(widget: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut child = widget.as_ref().first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        out.push(c);
    }
    out
}

fn set_label(widget: &gtk::Widget, text: &str) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        label.set_text(text);
    }
}

/// The "New pattern" row: a plus and the words.
fn new_pattern_row() -> (gtk::ListBoxRow, gtk::Label) {
    let row = gtk::ListBoxRow::new();
    row.set_widget_name("new-pattern");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let plus = gtk::Image::from_icon_name("list-add-symbolic");
    plus.add_css_class("hk-t3");
    content.append(&plus);
    let text = label(&tr("New pattern"), &["hk-t2"]);
    content.append(&text);
    row.set_child(Some(&content));
    row.update_property(&[gtk::accessible::Property::Label(&tr("New pattern"))]);
    (row, text)
}

/// The row widget: a check box, the two paths and the change.
fn row_widget(organizer: &std::rc::Weak<Organizer>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
    row.add_css_class("hk-organize-row");
    let check_cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    check_cell.set_size_request(CHECK - GAP, -1);
    let check = gtk::CheckButton::new();
    check.set_valign(gtk::Align::Center);
    let weak = organizer.clone();
    check.connect_toggled(move |check| {
        let Some(organizer) = weak.upgrade() else {
            return;
        };
        if organizer.filling.get() {
            return;
        }
        let Some(id) = check
            .widget_name()
            .strip_prefix("move-")
            .and_then(|n| n.parse::<i64>().ok())
            .map(FileId::from_raw)
        else {
            return;
        };
        organizer.set_ticked(id, check.is_active());
    });
    check_cell.append(&check);
    row.append(&check_cell);
    let paths = gtk::Box::new(gtk::Orientation::Vertical, 2);
    paths.set_hexpand(true);
    paths.set_valign(gtk::Align::Center);
    for classes in [
        &["hk-mono", "hk-tool-path", "hk-t3"][..],
        &["hk-mono", "hk-tool-path", "hk-medium"][..],
    ] {
        let line = label("", classes);
        line.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        line.set_max_width_chars(1);
        paths.append(&line);
    }
    row.append(&paths);
    let change = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    change.set_size_request(CHANGE, -1);
    change.set_valign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name("helikodu-warning-symbolic");
    icon.add_css_class("hk-t1");
    change.append(&icon);
    change.append(&label("", &["hk-t2"]));
    row.append(&change);
    row
}

/// The options: Pattern with Insert, the note under it, and Into.
struct Options {
    root: gtk::Grid,
    pattern: gtk::Entry,
    insert: gtk::MenuButton,
    note: gtk::Label,
    into: gtk::DropDown,
    characters: gtk::DropDown,
    companions: gtk::CheckButton,
}

fn options() -> Options {
    let grid = gtk::Grid::new();
    grid.add_css_class("hk-organize-options");
    grid.set_column_spacing(12);
    grid.set_row_spacing(6);
    let pattern_label = label(&tr("Pattern"), &["hk-medium"]);
    pattern_label.set_ellipsize(gtk::pango::EllipsizeMode::None);
    grid.attach(&pattern_label, 0, 0, 1, 1);
    let entry = crate::widgets::pattern_entry::PatternEntry::new("organize-pattern");
    entry.labelled_by(&pattern_label);
    let pattern = entry.entry.clone();
    let insert = entry.insert.clone();
    let line = entry.root;
    grid.attach(&line, 1, 0, 3, 1);
    let note = label("", &["hk-t3", "hk-secondary"]);
    note.set_widget_name("organize-pattern-note");
    grid.attach(&note, 1, 1, 3, 1);
    let into_label = label(&tr("Into"), &["hk-medium"]);
    into_label.set_ellipsize(gtk::pango::EllipsizeMode::None);
    grid.attach(&into_label, 0, 2, 1, 1);
    let into = dropdown(&[tr("The library folder each file is in")], 0, 236);
    into.set_widget_name("organize-into");
    into.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        into_label.upcast_ref()
    ])]);
    let sets: Vec<String> = SETS.iter().map(|s| set_name(*s)).collect();
    let characters = dropdown(&sets, 0, 246);
    characters.set_widget_name("organize-characters");
    characters.update_property(&[gtk::accessible::Property::Label(&tr("Characters in names"))]);
    let companions = gtk::CheckButton::with_label(&tr("Move covers and booklets too"));
    companions.set_active(true);
    companions.set_hexpand(true);
    companions.set_halign(gtk::Align::End);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.append(&into);
    row.append(&characters);
    row.append(&companions);
    grid.attach(&row, 1, 2, 3, 1);
    Options {
        root: grid,
        pattern,
        insert,
        note,
        into,
        characters,
        companions,
    }
}

/// Opens Organize files over `main` for `tracks` (the whole library when
/// `None`), or brings it to the front.
pub(crate) fn open(main: &MainWindow, tracks: Option<Vec<TrackKey>>) -> Option<gtk::Window> {
    let parent = main.upcast_ref::<gtk::Window>();
    if let Some(window) = find_open(parent, NAME) {
        window.present();
        return Some(window);
    }
    let backend = main.backend()?;
    let Frame {
        window,
        list,
        side_note,
        title,
        summary: header_summary,
        main: column,
    } = frame(parent, NAME, &tr("Organize files"), &tr("Patterns"));
    side_note.set_text(&tr(
        "Every move is written down first, so a run can be undone later, even after a restart.",
    ));

    let opts = options();
    column.append(&opts.root);
    column.append(&table_header(&[
        (String::new(), CHECK - GAP, false),
        (tr("Now, and after the move"), -1, false),
        (tr("Change"), CHANGE, false),
    ]));

    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    let factory = gtk::SignalListItemFactory::new();
    let view = gtk::ListView::new(Some(selection.clone()), Some(factory.clone()));
    // Tab leaves the list after the focused row, instead of visiting
    // every row: Tab and Shift+Tab reach the rest of the window.
    view.set_tab_behavior(gtk::ListTabBehavior::Item);
    view.add_css_class("hk-dup-list");
    view.add_css_class("hk-organize-list");
    view.set_widget_name("organize-table");
    view.update_property(&[gtk::accessible::Property::Label(&tr("Files to organize"))]);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&view));
    let empty = label("", &["hk-t3", "hk-tool-empty"]);
    empty.set_xalign(0.5);
    empty.set_valign(gtk::Align::Start);
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.add_named(&scroller, Some("table"));
    stack.add_named(&empty, Some("empty"));
    column.append(&stack);
    let results = gtk::Box::new(gtk::Orientation::Vertical, 0);
    results.add_css_class("hk-tool-results");
    results.set_visible(false);
    column.append(&results);
    let (bar, summary) = footer();
    let undo = compact_button(&tr("Undo the last run"));
    undo.set_widget_name("organize-undo");
    undo.set_sensitive(false);
    bar.append(&undo);
    let run = primary_button(&fill(&ntr("Move {} file", "Move {} files", 0), &["0"]));
    run.set_widget_name("organize-run");
    run.set_sensitive(false);
    bar.append(&run);
    column.append(&bar);
    let work = WorkBar::new();
    column.append(&work.root);

    let organizer = Rc::new(Organizer {
        window: window.clone(),
        backend,
        tracks,
        roots: RefCell::new(Vec::new()),
        patterns: RefCell::new(Vec::new()),
        chosen: Cell::new(None),
        planned: RefCell::new(Vec::new()),
        ticked: RefCell::new(HashSet::new()),
        store,
        selection,
        list: list.clone(),
        pattern: opts.pattern.clone(),
        pattern_note: opts.note.clone(),
        into: opts.into.clone(),
        into_folders: RefCell::new(vec![None]),
        chosen_folder: RefCell::new(None),
        characters: opts.characters.clone(),
        companions: opts.companions.clone(),
        title,
        header_summary,
        summary,
        footer: bar,
        work,
        undo: undo.clone(),
        run: run.clone(),
        results,
        empty,
        stack,
        last: RefCell::new(None),
        generation: Cell::new(0),
        typing: Cell::new(0),
        filling: Cell::new(false),
        busy: Cell::new(false),
        cancel: RefCell::new(None),
    });

    let _ = opts.insert;

    let weak = Rc::downgrade(&organizer);
    factory.connect_setup(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&row_widget(&weak)));
        }
    });
    let weak = Rc::downgrade(&organizer);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(this), Some(row), Some(object)) = (
            weak.upgrade(),
            item.child().and_downcast::<gtk::Box>(),
            item.item().and_downcast::<glib::BoxedAnyObject>(),
        ) else {
            return;
        };
        let index = *object.borrow::<usize>();
        this.bind(&row, item, index);
    });

    let (weak_rename, weak_delete) = (Rc::downgrade(&organizer), Rc::downgrade(&organizer));
    crate::tools::edit_rows(
        &list,
        |row| row.widget_name().starts_with("pattern-"),
        move |row| {
            if let Some(this) = weak_rename.upgrade() {
                this.rename_pattern(row);
            }
        },
        move |row| {
            if let Some(this) = weak_delete.upgrade() {
                this.delete_pattern(row);
            }
        },
    );
    let weak = Rc::downgrade(&organizer);
    list.connect_row_selected(move |_, row| {
        let (Some(this), Some(row)) = (weak.upgrade(), row) else {
            return;
        };
        if let Ok(index) = usize::try_from(row.index()) {
            this.choose(index);
        }
    });
    let weak = Rc::downgrade(&organizer);
    opts.pattern.connect_changed(move |_| {
        if let Some(this) = weak.upgrade() {
            this.typed();
        }
    });
    let weak = Rc::downgrade(&organizer);
    on_selected(&opts.into, move |index| {
        let Some(this) = weak.upgrade() else { return };
        if this.filling.get() {
            return;
        }
        let last = this.into_folders.borrow().len().saturating_sub(1);
        if index == last {
            // "Choose a folder…"
            let weak = Rc::downgrade(&this);
            let start = this.roots.borrow().first().cloned();
            choose_folder(
                &this.window,
                &tr("Organize into a folder"),
                &tr("Choose"),
                start.as_deref(),
                move |folder| {
                    if let Some(this) = weak.upgrade() {
                        this.chosen_folder.replace(Some(folder));
                        this.fill_into();
                        this.preview();
                    }
                },
            );
            this.filling.set(true);
            this.into.set_selected(0);
            this.filling.set(false);
            return;
        }
        this.preview();
    });
    let weak = Rc::downgrade(&organizer);
    on_selected(&opts.characters, move |_| {
        if let Some(this) = weak.upgrade() {
            this.preview();
        }
    });
    let weak = Rc::downgrade(&organizer);
    run.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.start();
        }
    });
    let weak = Rc::downgrade(&organizer);
    undo.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.step();
        }
    });
    let weak = Rc::downgrade(&organizer);
    organizer.work.stop.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade()
            && let Some(cancel) = this.cancel.borrow().as_ref()
        {
            cancel.cancel();
        }
    });
    let weak = Rc::downgrade(&organizer);
    view.connect_activate(move |_, position| {
        if let Some(this) = weak.upgrade() {
            this.play(position);
        }
    });
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&organizer);
    keys.connect_key_pressed(move |_, key, _, state| {
        if key == gdk::Key::space
            && state.is_empty()
            && let Some(this) = weak.upgrade()
            && this.toggle_selected()
        {
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    view.add_controller(keys);

    // The window keeps its state for as long as it is open; a run goes on
    // after it closes.
    let keep_alive = RefCell::new(Some(organizer.clone()));
    window.connect_destroy(move |_| {
        keep_alive.borrow_mut().take();
    });
    organizer.update_footer();
    organizer.load_patterns(None);
    window.present();
    Some(window)
}
