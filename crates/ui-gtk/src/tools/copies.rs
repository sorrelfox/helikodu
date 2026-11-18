// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The copies of one track's file, in the table style of Find duplicates: a
//! group row (the title, "Artist · length", the number of copies) and a row
//! for each file with the main-copy radio, a tick box, the path, format, size
//! and when it entered the library. "Make main copy" makes the chosen file the
//! one that plays. Ticked copies (never the main copy, a missing file, or a
//! second path to one file) go to the desktop trash after a confirmation, and
//! each file's result is listed afterwards.
//!
//! Opened with the copies badge, the context menu's "Copies…" or Alt+C. A
//! single click selects a row; a double click or Enter plays that file's
//! track.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use super::tool_window::{
    ADDED, FORMAT, GAP, SIZE, cell, check_headers, check_width, date, footer, path_cell,
    primary_button, shown_path, table_header,
};
use super::trash;
use crate::backend::services::{CopyFile, CopyGroup, FileId};
use crate::backend::{Backend, PlayerRequest, TrackKey, TrackRow};
use crate::dialogs::common::{compact_button, dialog_window, find_open, header_strip};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "copies";

struct Copies {
    window: gtk::Window,
    backend: Backend,
    track: TrackKey,
    /// The track as the lists show it, for the group row.
    row: RefCell<Option<TrackRow>>,
    group: RefCell<Option<CopyGroup>>,
    /// The copies ticked for the trash.
    ticked: RefCell<HashSet<FileId>>,
    /// The file the main-copy radio points at.
    chosen: Cell<Option<FileId>>,
    list: gtk::ListBox,
    results: gtk::Box,
    summary: gtk::Label,
    make_main: gtk::Button,
    trash: gtk::Button,
    /// Rows being filled, whose toggles are not the user's.
    filling: Cell<bool>,
}

/// Whether `file` may be ticked for the trash.
fn removable(file: &CopyFile) -> bool {
    !file.is_main && !file.missing && file.same_file_as.is_none()
}

/// Why `file` cannot be ticked.
fn why_not(file: &CopyFile) -> String {
    if file.is_main {
        tr("The main copy stays: it is the file that plays")
    } else if file.missing {
        tr("The file is missing")
    } else {
        tr("One file reached by two paths: it is never moved to the trash")
    }
}

/// The note after a file's path, and its tooltip.
fn tag(file: &CopyFile, group: &CopyGroup) -> Option<(String, String)> {
    if file.missing {
        return Some((tr("missing"), tr("The file is missing")));
    }
    if let Some(other) = file.same_file_as {
        let other = group
            .files
            .iter()
            .find(|f| f.file == other)
            .map(|f| shown_path(&f.path))
            .unwrap_or_default();
        let main = if file.is_main {
            format!("{} · ", tr("main copy"))
        } else {
            String::new()
        };
        return Some((
            format!("{main}{}", tr("one file, two paths")),
            fill(&tr("The same file as {}"), &[&other]),
        ));
    }
    file.is_main
        .then(|| (tr("main copy"), tr("The main copy is the file that plays")))
}

impl Copies {
    fn file(&self, id: FileId) -> Option<CopyFile> {
        self.group
            .borrow()
            .as_ref()
            .and_then(|g| g.files.iter().find(|f| f.file == id).cloned())
    }

    /// Fetches the group and the track again, and shows them.
    fn load(self: &Rc<Self>) {
        let services = self.backend.services();
        let library = self.backend.library();
        let track = self.track;
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let found = backend
                .work(move || {
                    let group = services.copies_of(track);
                    let row = library
                        .track_rows(&[track])
                        .ok()
                        .and_then(|rows| rows.into_iter().next());
                    (group, row)
                })
                .await;
            let Some(this) = weak.upgrade() else { return };
            match found {
                Some((Ok(group), row)) => {
                    if row.is_some() {
                        this.row.replace(row);
                    }
                    this.show(group);
                }
                Some((Err(error), _)) => this.show_problem(&error.0),
                None => this.show_problem(&tr("The copies cannot be read.")),
            }
        });
    }

    fn show_problem(&self, text: &str) {
        self.results_line(text);
    }

    /// Replaces the results area with one line of text.
    fn results_line(&self, text: &str) {
        while let Some(child) = self.results.first_child() {
            self.results.remove(&child);
        }
        let line = label(text, &["hk-t2"]);
        line.set_wrap(true);
        line.set_single_line_mode(false);
        self.results.append(&line);
        self.results.set_visible(true);
    }

    fn show(self: &Rc<Self>, group: Option<CopyGroup>) {
        crate::widgets::remove_rows(&self.list);
        self.list.append(&self.group_row(group.as_ref()));
        {
            let mut ticked = self.ticked.borrow_mut();
            match &group {
                Some(group) => {
                    ticked.retain(|id| group.files.iter().any(|f| f.file == *id && removable(f)))
                }
                None => ticked.clear(),
            }
        }
        self.chosen.set(group.as_ref().map(|g| g.main));
        self.group.replace(group.clone());
        match &group {
            Some(group) => {
                let mut first_radio: Option<gtk::CheckButton> = None;
                self.filling.set(true);
                for (index, file) in group.files.iter().enumerate() {
                    let row = self.file_row(index, file, group, &mut first_radio);
                    self.list.append(&row);
                }
                self.filling.set(false);
            }
            None => {
                let row = gtk::ListBoxRow::new();
                row.set_activatable(false);
                row.set_selectable(false);
                row.add_css_class("hk-dup-file");
                row.set_child(Some(&label(
                    &tr("The library has no other copy of this file now."),
                    &["hk-t3"],
                )));
                self.list.append(&row);
            }
        }
        self.update_footer();
    }

    fn group_row(&self, group: Option<&CopyGroup>) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);
        row.set_selectable(false);
        row.set_focusable(false);
        row.add_css_class("hk-dup-head");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.set_valign(gtk::Align::End);
        let track = self.row.borrow();
        let title_text = track.as_ref().map(|t| t.title.clone()).unwrap_or_default();
        let title = label(&title_text, &["hk-strong"]);
        line.append(&title);
        let mut by = Vec::new();
        if let Some(track) = track.as_ref() {
            if !track.artist.is_empty() {
                by.push(track.artist.clone());
            }
            if let Some(length) = track.duration {
                by.push(format::duration(length));
            }
        }
        let by_text = by.join(" · ");
        let by_label = label(&by_text, &["hk-t3"]);
        by_label.set_hexpand(true);
        line.append(&by_label);
        let files = group.map_or(1, |g| g.files.len());
        let count_text = fill(
            &ntr("{} copy", "{} copies", files),
            &[&format::count(files)],
        );
        line.append(&label(&count_text, &["hk-t3", "hk-secondary"]));
        row.set_child(Some(&line));
        row.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{}, {}, {}"),
            &[&title_text, &by_text, &count_text],
        ))]);
        row
    }

    fn file_row(
        self: &Rc<Self>,
        index: usize,
        file: &CopyFile,
        group: &CopyGroup,
        first_radio: &mut Option<gtk::CheckButton>,
    ) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("hk-dup-file");
        if index.is_multiple_of(2) {
            row.add_css_class("hk-alt");
        }
        row.set_widget_name(&format!("copy-{}", file.file));
        let line = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
        let path = shown_path(&file.path);

        let main_cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        main_cell.set_size_request(check_width(), -1);
        let radio = gtk::CheckButton::new();
        if let Some(first) = first_radio.as_ref() {
            radio.set_group(Some(first));
        } else {
            *first_radio = Some(radio.clone());
        }
        radio.set_active(file.file == group.main);
        radio.set_sensitive(!file.missing && (file.same_file_as.is_none() || file.is_main));
        radio.set_widget_name("main-copy");
        radio.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("Main copy: {}"),
            &[&path],
        ))]);
        radio.set_tooltip_text(Some(&tr("The main copy is the file that plays")));
        let id = file.file;
        let weak = Rc::downgrade(self);
        radio.connect_toggled(move |radio| {
            if let Some(this) = weak.upgrade()
                && radio.is_active()
                && !this.filling.get()
            {
                this.chosen.set(Some(id));
                this.update_footer();
            }
        });
        main_cell.append(&radio);
        line.append(&main_cell);

        let tick_cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tick_cell.set_size_request(check_width(), -1);
        let tick = gtk::CheckButton::new();
        tick.set_widget_name("trash-tick");
        let can = removable(file);
        tick.set_sensitive(can);
        tick.set_active(can && self.ticked.borrow().contains(&file.file));
        tick.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("Move to the trash: {}"),
            &[&path],
        ))]);
        if can {
            tick.set_tooltip_text(Some(&tr("Move this copy to the trash")));
        } else {
            tick.set_tooltip_text(Some(&why_not(file)));
        }
        let weak = Rc::downgrade(self);
        let weak_row = row.downgrade();
        tick.connect_toggled(move |tick| {
            let Some(this) = weak.upgrade() else { return };
            if tick.is_active() {
                this.ticked.borrow_mut().insert(id);
            } else {
                this.ticked.borrow_mut().remove(&id);
            }
            if let Some(row) = weak_row.upgrade() {
                if tick.is_active() {
                    row.add_css_class("hk-marked");
                } else {
                    row.remove_css_class("hk-marked");
                }
            }
            this.update_footer();
        });
        if tick.is_active() {
            row.add_css_class("hk-marked");
        }
        tick_cell.append(&tick);
        line.append(&tick_cell);

        let file_cell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        file_cell.set_hexpand(true);
        let path_label = path_cell();
        path_label.set_text(&path);
        path_label.set_tooltip_text(Some(&path));
        file_cell.append(&path_label);
        let note = tag(file, group);
        if let Some((text, tooltip)) = &note {
            let note = label(text, &["hk-t3", "hk-secondary"]);
            note.set_hexpand(false);
            note.set_ellipsize(gtk::pango::EllipsizeMode::None);
            note.set_tooltip_text(Some(tooltip));
            file_cell.append(&note);
        }
        line.append(&file_cell);
        let format_text = file
            .format
            .as_ref()
            .map(|f| f.to_uppercase())
            .unwrap_or_default();
        let format_label = cell(FORMAT, &["hk-mono-meta", "hk-t3"], false);
        format_label.set_text(&format_text);
        line.append(&format_label);
        let size = cell(SIZE, &["hk-numeric"], true);
        size.set_text(&format::size(file.size));
        line.append(&size);
        let added = cell(ADDED, &["hk-numeric"], true);
        let added_text = date(file.added);
        added.set_text(&added_text);
        line.append(&added);
        row.set_child(Some(&line));
        let mut name = vec![path, format_text, format::size(file.size)];
        if !added_text.is_empty() {
            name.push(fill(&tr("added {}"), &[&added_text]));
        }
        if let Some((_, tooltip)) = note {
            name.push(tooltip);
        }
        row.update_property(&[gtk::accessible::Property::Label(&name.join(", "))]);
        row
    }

    fn update_footer(&self) {
        let group = self.group.borrow();
        let ticked = self.ticked.borrow();
        let count = ticked.len();
        let freed: u64 = group
            .as_ref()
            .map(|g| {
                g.files
                    .iter()
                    .filter(|f| ticked.contains(&f.file))
                    .map(|f| f.size)
                    .sum()
            })
            .unwrap_or(0);
        self.summary.set_text(&if count == 0 {
            tr("Tick the copies to move to the trash. The main copy stays.")
        } else {
            fill(
                &tr("{} · {} will be freed · they go to the trash, so this can be undone"),
                &[
                    &fill(
                        &ntr("{} file ticked", "{} files ticked", count),
                        &[&format::count(count)],
                    ),
                    &format::size(freed),
                ],
            )
        });
        self.trash
            .set_label(&format!("{}…", trash::move_label(count)));
        self.trash.set_sensitive(count > 0);
        let main = group.as_ref().map(|g| g.main);
        self.make_main
            .set_sensitive(self.chosen.get().is_some() && self.chosen.get() != main);
    }

    fn make_main(self: &Rc<Self>) {
        let Some(file) = self.chosen.get() else {
            return;
        };
        let services = self.backend.services();
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        let path = self
            .file(file)
            .map(|f| shown_path(&f.path))
            .unwrap_or_default();
        glib::spawn_future_local(async move {
            let result = backend.work(move || services.set_main_copy(file)).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Some(Ok(())) => {
                    this.results_line(&fill(&tr("{} is the main copy now."), &[&path]));
                    this.load();
                }
                Some(Err(error)) => {
                    this.results_line(&fill(&tr("The main copy was not changed: {}"), &[&error.0]))
                }
                None => this.results_line(&tr("The main copy was not changed.")),
            }
        });
    }

    fn ask_trash(self: &Rc<Self>) {
        let files: Vec<CopyFile> = {
            let group = self.group.borrow();
            let ticked = self.ticked.borrow();
            group
                .as_ref()
                .map(|g| {
                    g.files
                        .iter()
                        .filter(|f| ticked.contains(&f.file) && removable(f))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        if files.is_empty() {
            return;
        }
        let paths: Vec<String> = files.iter().map(|f| shown_path(&f.path)).collect();
        let freed = files.iter().map(|f| f.size).sum();
        let ids: Vec<FileId> = files.iter().map(|f| f.file).collect();
        let weak = Rc::downgrade(self);
        trash::confirm(&self.window, &paths, freed, move |yes| {
            if yes && let Some(this) = weak.upgrade() {
                this.move_to_trash(ids);
            }
        });
    }

    fn move_to_trash(self: &Rc<Self>, ids: Vec<FileId>) {
        let services = self.backend.services();
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        self.trash.set_sensitive(false);
        glib::spawn_future_local(async move {
            let result = backend.work(move || services.trash_files(&ids, true)).await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Some(Ok(results)) => {
                    while let Some(child) = this.results.first_child() {
                        this.results.remove(&child);
                    }
                    this.results.append(&trash::results_box(&results, false));
                    this.results.set_visible(true);
                    this.ticked.borrow_mut().clear();
                    if let Some(main) = this.window.transient_for().and_downcast::<MainWindow>() {
                        main.toast(&trash::summary(&results), false);
                    }
                }
                Some(Err(error)) => this.results_line(&fill(
                    &tr("Nothing was moved to the trash: {}"),
                    &[&error.0],
                )),
                None => this.results_line(&tr("Nothing was moved to the trash.")),
            }
            this.load();
        });
    }

    fn play(&self, row: &gtk::ListBoxRow) {
        let Some(id) = row
            .widget_name()
            .strip_prefix("copy-")
            .and_then(|n| n.parse::<i64>().ok())
            .map(FileId::from_raw)
        else {
            return;
        };
        let Some(track) = self.file(id).and_then(|f| f.track) else {
            return;
        };
        self.backend.send(PlayerRequest::PlayTracks {
            tracks: vec![TrackKey(track.get())],
            start: 0,
            title: tr("Copies"),
            shuffle: false,
        });
    }
}

/// Opens the copies of `track`'s file over `main`, replacing a copies window
/// already open.
pub(crate) fn open(main: &MainWindow, track: TrackKey) -> Option<gtk::Window> {
    let parent = main.upcast_ref::<gtk::Window>();
    if let Some(old) = find_open(parent, NAME) {
        old.destroy();
    }
    let backend = main.backend()?;
    let window = dialog_window(
        Some(parent),
        NAME,
        &tr("Identical copies"),
        (860, 460),
        false,
    );
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let (header, _) = header_strip(
        &window,
        &tr("Identical copies"),
        Some(&tr("Files with the same bytes, tags included")),
    );
    column.append(&header);
    let [main, remove] = check_headers();
    column.append(&table_header(&[
        (main, check_width(), false),
        (remove, check_width(), false),
        (tr("File"), -1, false),
        (tr("Format"), FORMAT, false),
        (tr("Size"), SIZE, true),
        (tr("Added"), ADDED, true),
    ]));
    let list = gtk::ListBox::new();
    list.add_css_class("hk-dup-table");
    list.set_activate_on_single_click(false);
    list.update_property(&[gtk::accessible::Property::Label(&tr("Copies"))]);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&list));
    column.append(&scroller);
    let results = gtk::Box::new(gtk::Orientation::Vertical, 0);
    results.add_css_class("hk-tool-results");
    results.set_visible(false);
    column.append(&results);
    let (bar, summary) = footer();
    let make_main = compact_button(&tr("Make main copy"));
    make_main.set_widget_name("make-main-copy");
    make_main.set_sensitive(false);
    bar.append(&make_main);
    let trash_button = primary_button(&format!("{}…", trash::move_label(0)));
    trash_button.set_widget_name("trash-copies");
    trash_button.set_sensitive(false);
    bar.append(&trash_button);
    column.append(&bar);
    window.set_child(Some(&column));

    let copies = Rc::new(Copies {
        window: window.clone(),
        backend,
        track,
        row: RefCell::new(None),
        group: RefCell::new(None),
        ticked: RefCell::new(HashSet::new()),
        chosen: Cell::new(None),
        list: list.clone(),
        results,
        summary,
        make_main: make_main.clone(),
        trash: trash_button.clone(),
        filling: Cell::new(false),
    });
    let weak = Rc::downgrade(&copies);
    make_main.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.make_main();
        }
    });
    let weak = Rc::downgrade(&copies);
    trash_button.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.ask_trash();
        }
    });
    let weak = Rc::downgrade(&copies);
    list.connect_row_activated(move |_, row| {
        if let Some(this) = weak.upgrade() {
            this.play(row);
        }
    });
    // The window keeps its state for as long as it is open.
    let keep = RefCell::new(Some(copies.clone()));
    window.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });
    copies.update_footer();
    copies.load();
    window.present();
    Some(window)
}
