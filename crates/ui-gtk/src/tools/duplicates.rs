// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Find duplicates: the four ways of matching files at the left, each with
//! the number of groups it finds; a filter, "Keep" (the best quality, the
//! oldest in the library, the most played, I choose by hand) and "Mark", which
//! ticks every file of the shown groups but the one to keep; the table of
//! groups; and the footer with what is marked, the space it frees and "Move N
//! files to the trash". Files reached by two paths are listed but never
//! markable, and neither is the main copy of a group of identical files.
//!
//! The table is a list view over one row per group heading and file, so a
//! library with thousands of groups scrolls as fast as one with a few. A
//! single click selects a row, Space ticks it, and a double click or Enter
//! plays its track.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::tool_window::{
    ADDED, FORMAT, Frame, GAP, PLAYS, SIZE, cell, check_headers, check_width, date, entry_format,
    footer, frame, path_cell, primary_button, shown_path, side_row, table_header,
};
use super::trash;
use crate::backend::services::{
    DuplicateCounts, DuplicateEntry, DuplicateGroup, FileId, KeepRule, MatchBy,
};
use crate::backend::{Backend, PlayerRequest, TrackKey};
use crate::dialogs::common::{compact_button, dropdown, find_open, on_selected};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "find-duplicates";

/// The ways of matching, in the order of the left pane.
const MODES: [MatchBy; 4] = [
    MatchBy::SameRecording,
    MatchBy::SameAlbumTrack,
    MatchBy::IdenticalFiles,
    MatchBy::SameFile,
];

/// The keep rules, in the order of the dropdown.
const RULES: [KeepRule; 4] = [
    KeepRule::BestQuality,
    KeepRule::Oldest,
    KeepRule::MostPlayed,
    KeepRule::ByHand,
];

fn mode_name(by: MatchBy) -> String {
    match by {
        MatchBy::SameRecording => tr("Same recording"),
        MatchBy::SameAlbumTrack => tr("Same track, same album"),
        MatchBy::SameFile => tr("One file, two paths"),
        _ => tr("Identical files"),
    }
}

fn mode_line(by: MatchBy) -> String {
    match by {
        MatchBy::SameRecording => tr("Title, artist and length"),
        MatchBy::SameAlbumTrack => tr("Also album and track number"),
        MatchBy::SameFile => tr("Links and folders mounted twice"),
        _ => tr("Same size and content"),
    }
}

fn mode_count(by: MatchBy, counts: &DuplicateCounts) -> usize {
    match by {
        MatchBy::SameRecording => counts.same_recording,
        MatchBy::SameAlbumTrack => counts.same_album_track,
        MatchBy::SameFile => counts.same_file,
        _ => counts.identical_files,
    }
}

fn rule_name(rule: KeepRule) -> String {
    match rule {
        KeepRule::Oldest => tr("The oldest in the library"),
        KeepRule::MostPlayed => tr("The most played"),
        KeepRule::ByHand => tr("I choose by hand"),
        _ => tr("The best quality"),
    }
}

/// Why `entry` cannot be marked.
fn why_not(entry: &DuplicateEntry) -> String {
    if entry.same_file {
        tr("One file reached by two paths: it is never moved to the trash")
    } else if entry.missing {
        tr("The file is missing")
    } else if entry.is_main {
        tr("The main copy stays: it is the file that plays")
    } else {
        tr("Part of a file with several tracks: it is never moved on its own")
    }
}

/// One row of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    /// The heading of the group at this index.
    Head(usize),
    /// A file: the group and the entry.
    File(usize, usize),
}

struct Finder {
    window: gtk::Window,
    backend: Backend,
    by: Cell<MatchBy>,
    keep: Cell<KeepRule>,
    groups: RefCell<Vec<DuplicateGroup>>,
    marked: RefCell<HashSet<FileId>>,
    lines: RefCell<Vec<Line>>,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    filter: gtk::SearchEntry,
    mark: gtk::Button,
    counts: Vec<(MatchBy, gtk::Label)>,
    title: gtk::Label,
    header_summary: gtk::Label,
    side_note: gtk::Label,
    summary: gtk::Label,
    trash: gtk::Button,
    results: gtk::Box,
    empty: gtk::Label,
    stack: gtk::Stack,
    generation: Cell<u64>,
    filling: Cell<bool>,
}

impl Finder {
    fn line(&self, position: u32) -> Option<Line> {
        self.lines.borrow().get(position as usize).copied()
    }

    fn entry(&self, group: usize, entry: usize) -> Option<DuplicateEntry> {
        self.groups
            .borrow()
            .get(group)
            .and_then(|g| g.entries.get(entry))
            .cloned()
    }

    /// Whether the shown groups' `group` matches the filter text.
    fn shown(group: &DuplicateGroup, text: &str) -> bool {
        if text.is_empty() {
            return true;
        }
        let has = |value: Option<&String>| value.is_some_and(|v| v.to_lowercase().contains(text));
        has(group.title.as_ref())
            || has(group.artist.as_ref())
            || group
                .entries
                .iter()
                .any(|e| e.display_path.to_lowercase().contains(text))
    }

    /// The indices of the groups the filter shows.
    fn visible_groups(&self) -> Vec<usize> {
        let text = self.filter.text().trim().to_lowercase();
        self.groups
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, g)| Self::shown(g, &text))
            .map(|(i, _)| i)
            .collect()
    }

    /// Builds the table's rows from the groups and the filter.
    fn rebuild(&self) {
        let groups = self.groups.borrow();
        let visible = self.visible_groups();
        let mut lines = Vec::new();
        let mut files = 0;
        for &index in &visible {
            lines.push(Line::Head(index));
            for entry in 0..groups[index].entries.len() {
                lines.push(Line::File(index, entry));
                files += 1;
            }
        }
        drop(groups);
        let items: Vec<glib::BoxedAnyObject> = lines
            .iter()
            .map(|l| glib::BoxedAnyObject::new(*l))
            .collect();
        self.lines.replace(lines);
        self.store.splice(0, self.store.n_items(), &items);
        self.header_summary.set_text(&fill(
            &tr("{} · {}"),
            &[
                &fill(
                    &ntr("{} group", "{} groups", visible.len()),
                    &[&format::count(visible.len())],
                ),
                &fill(&ntr("{} file", "{} files", files), &[&format::count(files)]),
            ],
        ));
        let none = visible.is_empty();
        self.stack
            .set_visible_child_name(if none { "empty" } else { "table" });
        if none {
            self.empty.set_text(&if self.filter.text().is_empty() {
                tr("No duplicates found this way.")
            } else {
                tr("No group matches the filter.")
            });
        }
        self.update_footer();
    }

    fn update_footer(&self) {
        let by = self.by.get();
        let marked = self.marked.borrow();
        let count = marked.len();
        let freed: u64 = self
            .groups
            .borrow()
            .iter()
            .flat_map(|g| g.entries.iter())
            .filter(|e| marked.contains(&e.file))
            .map(|e| e.size)
            .sum();
        let text = if by == MatchBy::SameFile {
            tr(
                "One file reached by two paths takes its space once: nothing here is moved to the trash.",
            )
        } else if count == 0 {
            tr(
                "Nothing is marked. Choose which file to keep and press Mark, or tick files yourself.",
            )
        } else {
            fill(
                &tr("{} · {} will be freed · they go to the trash, so this can be undone"),
                &[
                    &fill(
                        &ntr("{} file marked", "{} files marked", count),
                        &[&format::count(count)],
                    ),
                    &format::size(freed),
                ],
            )
        };
        self.summary.set_text(&text);
        self.trash.set_label(&trash::move_label(count));
        self.trash
            .set_sensitive(count > 0 && by != MatchBy::SameFile);
        // This way only reports: it has no action.
        self.trash.set_visible(by != MatchBy::SameFile);
        self.mark
            .set_sensitive(by != MatchBy::SameFile && self.keep.get() != KeepRule::ByHand);
    }

    /// Ticks or clears `file`, when it may be marked.
    fn set_marked(&self, file: FileId, on: bool) {
        if on {
            self.marked.borrow_mut().insert(file);
        } else {
            self.marked.borrow_mut().remove(&file);
        }
        self.update_footer();
    }

    /// Loads the groups of the chosen way of matching.
    fn load(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let by = self.by.get();
        let keep = self.keep.get();
        self.title.set_text(&mode_name(by));
        self.header_summary.set_text("");
        self.marked.borrow_mut().clear();
        self.side_note.set_text(&tr("Looking for duplicates…"));
        let services = self.backend.services();
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let started = Instant::now();
            let found = backend
                .work(move || services.find_duplicates(by, keep))
                .await;
            let took = started.elapsed();
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            let groups = match found {
                Some(Ok(groups)) => groups,
                Some(Err(error)) => {
                    this.side_note.set_text(&error.0);
                    Vec::new()
                }
                None => Vec::new(),
            };
            let files: usize = groups.iter().map(|g| g.entries.len()).sum();
            this.side_note.set_text(&fill(
                &tr("Compared {} in {} s. Nothing is removed until you say so."),
                &[
                    &fill(&ntr("{} file", "{} files", files), &[&format::count(files)]),
                    &format::decimal(took.as_secs_f64(), 1),
                ],
            ));
            this.groups.replace(groups);
            this.rebuild();
        });
    }

    fn load_counts(self: &Rc<Self>) {
        let services = self.backend.services();
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let counts = backend.work(move || services.duplicate_counts()).await;
            let Some(this) = weak.upgrade() else { return };
            if let Some(Ok(counts)) = counts {
                for (by, label) in &this.counts {
                    label.set_text(&format::count(mode_count(*by, &counts)));
                }
            }
        });
    }

    /// Marks every file of the shown groups but the one the keep rule keeps.
    fn mark(self: &Rc<Self>) {
        let by = self.by.get();
        let keep = self.keep.get();
        if keep == KeepRule::ByHand || by == MatchBy::SameFile {
            return;
        }
        let services = self.backend.services();
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        let generation = self.generation.get();
        glib::spawn_future_local(async move {
            let found = backend
                .work(move || services.find_duplicates(by, keep))
                .await;
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            let Some(Ok(groups)) = found else { return };
            let before = this.marked.borrow().clone();
            this.groups.replace(groups);
            let visible = this.visible_groups();
            let groups = this.groups.borrow();
            let mut marked: HashSet<FileId> = before
                .into_iter()
                .filter(|file| {
                    groups
                        .iter()
                        .flat_map(|g| g.entries.iter())
                        .any(|e| e.file == *file && e.removable)
                })
                .collect();
            for index in visible {
                for entry in &groups[index].entries {
                    marked.remove(&entry.file);
                    if entry.marked && entry.removable {
                        marked.insert(entry.file);
                    }
                }
            }
            drop(groups);
            this.marked.replace(marked);
            this.rebuild();
        });
    }

    fn ask_trash(self: &Rc<Self>) {
        let entries: Vec<DuplicateEntry> = {
            let marked = self.marked.borrow();
            self.groups
                .borrow()
                .iter()
                .flat_map(|g| g.entries.iter())
                .filter(|e| marked.contains(&e.file) && e.removable)
                .cloned()
                .collect()
        };
        if entries.is_empty() {
            return;
        }
        let paths: Vec<String> = entries.iter().map(|e| shown_path(&e.path)).collect();
        let freed = entries.iter().map(|e| e.size).sum();
        let ids: Vec<FileId> = entries.iter().map(|e| e.file).collect();
        let copies = self.by.get() == MatchBy::IdenticalFiles;
        let weak = Rc::downgrade(self);
        trash::confirm(&self.window, &paths, freed, move |yes| {
            if yes && let Some(this) = weak.upgrade() {
                this.move_to_trash(ids, copies);
            }
        });
    }

    fn move_to_trash(self: &Rc<Self>, ids: Vec<FileId>, copies: bool) {
        let services = self.backend.services();
        let backend = self.backend.clone();
        let weak = Rc::downgrade(self);
        self.trash.set_sensitive(false);
        glib::spawn_future_local(async move {
            let result = backend
                .work(move || services.trash_files(&ids, copies))
                .await;
            let Some(this) = weak.upgrade() else { return };
            while let Some(child) = this.results.first_child() {
                this.results.remove(&child);
            }
            match result {
                Some(Ok(results)) => {
                    this.results.append(&trash::results_box(&results, true));
                    if let Some(main) = this.window.transient_for().and_downcast::<MainWindow>() {
                        main.toast(&trash::summary(&results), false);
                    }
                }
                Some(Err(error)) => this.results.append(&label(
                    &fill(&tr("Nothing was moved to the trash: {}"), &[&error.0]),
                    &["hk-t2"],
                )),
                None => this
                    .results
                    .append(&label(&tr("Nothing was moved to the trash."), &["hk-t2"])),
            }
            this.results.set_visible(true);
            this.load_counts();
            this.load();
        });
    }

    fn play(&self, position: u32) {
        let Some(Line::File(group, entry)) = self.line(position) else {
            return;
        };
        let Some(track) = self.entry(group, entry).and_then(|e| e.track) else {
            return;
        };
        self.backend.send(PlayerRequest::PlayTracks {
            tracks: vec![TrackKey(track.get())],
            start: 0,
            title: tr("Duplicates"),
            shuffle: false,
        });
    }

    /// Space ticks or clears the selected file.
    fn toggle_selected(&self) -> bool {
        let position = self.selection.selected();
        let Some(Line::File(group, entry)) = self.line(position) else {
            return false;
        };
        let Some(entry) = self.entry(group, entry) else {
            return false;
        };
        if !entry.removable || self.by.get() == MatchBy::SameFile {
            return true;
        }
        let on = !self.marked.borrow().contains(&entry.file);
        self.set_marked(entry.file, on);
        self.store.items_changed(position, 1, 1);
        true
    }

    /// Shows `line` in the row widget `row`.
    fn bind(&self, row: &gtk::Box, item: &gtk::ListItem, line: Line) {
        let (Some(head), Some(file)) = (row.first_child(), row.last_child()) else {
            return;
        };
        let groups = self.groups.borrow();
        match line {
            Line::Head(index) => {
                head.set_visible(true);
                file.set_visible(false);
                row.remove_css_class("hk-dup-file");
                row.remove_css_class("hk-alt");
                row.remove_css_class("hk-marked");
                row.add_css_class("hk-dup-head");
                item.set_selectable(false);
                item.set_activatable(false);
                item.set_focusable(false);
                let Some(group) = groups.get(index) else {
                    return;
                };
                let title = group.title.clone().unwrap_or_default();
                let mut by = Vec::new();
                if let Some(artist) = group.artist.as_ref().filter(|a| !a.is_empty()) {
                    by.push(artist.clone());
                }
                if let Some(length) = group.duration {
                    by.push(format::duration(length));
                }
                let by = by.join(" · ");
                let n = group.entries.len();
                let count = if self.by.get() == MatchBy::SameFile {
                    fill(&ntr("{} path", "{} paths", n), &[&format::count(n)])
                } else {
                    fill(&ntr("{} copy", "{} copies", n), &[&format::count(n)])
                };
                let labels = children(&head);
                if let [t, b, c] = labels.as_slice() {
                    set_label(t, &title);
                    set_label(b, &by);
                    set_label(c, &count);
                }
                item.set_accessible_label(&fill(&tr("{}, {}, {}"), &[&title, &by, &count]));
            }
            Line::File(index, entry_index) => {
                head.set_visible(false);
                file.set_visible(true);
                row.remove_css_class("hk-dup-head");
                row.add_css_class("hk-dup-file");
                if entry_index.is_multiple_of(2) {
                    row.add_css_class("hk-alt");
                } else {
                    row.remove_css_class("hk-alt");
                }
                item.set_selectable(true);
                item.set_activatable(true);
                item.set_focusable(true);
                let Some(entry) = groups.get(index).and_then(|g| g.entries.get(entry_index)) else {
                    return;
                };
                let markable = entry.removable && self.by.get() != MatchBy::SameFile;
                let marked = markable && self.marked.borrow().contains(&entry.file);
                if marked {
                    row.add_css_class("hk-marked");
                } else {
                    row.remove_css_class("hk-marked");
                }
                let cells = children(&file);
                let path = shown_path(&entry.path);
                let format_text = entry_format(entry);
                let added = date(entry.added);
                if let [check_cell, p, f, s, plays, a] = cells.as_slice() {
                    if let Some(check) = check_cell.first_child().and_downcast::<gtk::CheckButton>()
                    {
                        self.filling.set(true);
                        check.set_widget_name(&format!("file-{}", entry.file));
                        check.set_sensitive(markable);
                        check.set_active(marked);
                        check.set_tooltip_text(Some(&if markable {
                            tr("Move this file to the trash")
                        } else {
                            why_not(entry)
                        }));
                        check.update_property(&[gtk::accessible::Property::Label(&fill(
                            &tr("Move to the trash: {}"),
                            &[&path],
                        ))]);
                        self.filling.set(false);
                    }
                    set_label(p, &path);
                    p.set_tooltip_text(Some(&path));
                    set_label(f, &format_text);
                    set_label(s, &format::size(entry.size));
                    set_label(plays, &entry.plays.to_string());
                    set_label(a, &added);
                }
                let mut name = vec![path, format_text, format::size(entry.size)];
                name.push(fill(
                    &ntr("{} play", "{} plays", entry.plays as usize),
                    &[&entry.plays.to_string()],
                ));
                if !added.is_empty() {
                    name.push(fill(&tr("added {}"), &[&added]));
                }
                if marked {
                    name.push(tr("marked for the trash"));
                } else if !markable {
                    name.push(why_not(entry));
                }
                item.set_accessible_label(&name.join(", "));
            }
        }
    }
}

fn children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut child = widget.first_child();
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

/// The row widget: a group heading and a file line, one of them shown.
fn row_widget(finder: &std::rc::Weak<Finder>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.set_valign(gtk::Align::End);
    head.set_vexpand(true);
    head.append(&label("", &["hk-strong"]));
    let by = label("", &["hk-t3"]);
    by.set_hexpand(true);
    head.append(&by);
    head.append(&label("", &["hk-t3", "hk-secondary"]));
    row.append(&head);
    let file = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
    file.set_vexpand(true);
    let check_cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    check_cell.set_size_request(check_width(), -1);
    let check = gtk::CheckButton::new();
    check.set_valign(gtk::Align::Center);
    let weak = finder.clone();
    let weak_row = row.downgrade();
    check.connect_toggled(move |check| {
        let Some(finder) = weak.upgrade() else { return };
        if finder.filling.get() {
            return;
        }
        let Some(id) = check
            .widget_name()
            .strip_prefix("file-")
            .and_then(|n| n.parse::<i64>().ok())
            .map(FileId::from_raw)
        else {
            return;
        };
        finder.set_marked(id, check.is_active());
        if let Some(row) = weak_row.upgrade() {
            if check.is_active() {
                row.add_css_class("hk-marked");
            } else {
                row.remove_css_class("hk-marked");
            }
        }
    });
    check_cell.append(&check);
    file.append(&check_cell);
    let path = path_cell();
    path.add_css_class("hk-dup-path");
    file.append(&path);
    file.append(&cell(FORMAT, &["hk-mono-meta", "hk-t3"], false));
    file.append(&cell(SIZE, &["hk-numeric"], true));
    file.append(&cell(PLAYS, &["hk-numeric"], true));
    file.append(&cell(ADDED, &["hk-numeric"], true));
    row.append(&file);
    row
}

/// Opens Find duplicates over `main`, or brings it to the front.
pub(crate) fn open(main: &MainWindow) -> Option<gtk::Window> {
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
    } = frame(parent, NAME, &tr("Find duplicates"), &tr("Match by"));

    let mut counts = Vec::new();
    for by in MODES {
        let (row, count) = side_row(&mode_name(by), &mode_line(by));
        list.append(&row);
        counts.push((by, count));
    }

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    toolbar.add_css_class("hk-toolbar");
    toolbar.add_css_class("hk-tool-toolbar");
    let filter = gtk::SearchEntry::new();
    filter.add_css_class("hk-input");
    filter.set_hexpand(true);
    filter.set_valign(gtk::Align::Center);
    filter.set_placeholder_text(Some(&tr("Filter by title, artist or folder")));
    filter.update_property(&[gtk::accessible::Property::Label(&tr(
        "Filter the duplicates",
    ))]);
    toolbar.append(&filter);
    let keep_label = label(&tr("Keep"), &["hk-t2", "hk-keep-label"]);
    toolbar.append(&keep_label);
    let rules: Vec<String> = RULES.iter().map(|r| rule_name(*r)).collect();
    let keep = dropdown(&rules, 0, 200);
    keep.set_widget_name("keep-rule");
    keep.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        keep_label.upcast_ref()
    ])]);
    toolbar.append(&keep);
    let mark = compact_button(&tr("Mark"));
    mark.set_widget_name("mark");
    mark.set_tooltip_text(Some(&tr(
        "Tick every file of the shown groups but the one to keep",
    )));
    toolbar.append(&mark);
    column.append(&toolbar);
    let [_, remove] = check_headers();
    column.append(&table_header(&[
        (remove, check_width(), false),
        (tr("File"), -1, false),
        (tr("Format"), FORMAT, false),
        (tr("Size"), SIZE, true),
        (tr("Plays"), PLAYS, true),
        (tr("Added"), ADDED, true),
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
    view.set_widget_name("duplicates-table");
    view.update_property(&[gtk::accessible::Property::Label(&tr("Duplicates"))]);
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
    let trash_button = primary_button(&trash::move_label(0));
    trash_button.set_widget_name("trash-duplicates");
    trash_button.set_sensitive(false);
    bar.append(&trash_button);
    column.append(&bar);

    let finder = Rc::new(Finder {
        window: window.clone(),
        backend,
        by: Cell::new(MatchBy::IdenticalFiles),
        keep: Cell::new(KeepRule::BestQuality),
        groups: RefCell::new(Vec::new()),
        marked: RefCell::new(HashSet::new()),
        lines: RefCell::new(Vec::new()),
        store,
        selection,
        filter: filter.clone(),
        mark: mark.clone(),
        counts,
        title,
        header_summary,
        side_note,
        summary,
        trash: trash_button.clone(),
        results,
        empty,
        stack,
        generation: Cell::new(0),
        filling: Cell::new(false),
    });

    let weak = Rc::downgrade(&finder);
    factory.connect_setup(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&row_widget(&weak)));
        }
    });
    let weak = Rc::downgrade(&finder);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(finder), Some(row), Some(object)) = (
            weak.upgrade(),
            item.child().and_downcast::<gtk::Box>(),
            item.item().and_downcast::<glib::BoxedAnyObject>(),
        ) else {
            return;
        };
        let line = *object.borrow::<Line>();
        finder.bind(&row, item, line);
    });

    let weak = Rc::downgrade(&finder);
    list.connect_row_selected(move |_, row| {
        let (Some(finder), Some(row)) = (weak.upgrade(), row) else {
            return;
        };
        let Some(by) = usize::try_from(row.index())
            .ok()
            .and_then(|i| MODES.get(i))
            .copied()
        else {
            return;
        };
        if finder.by.get() != by || finder.groups.borrow().is_empty() {
            finder.by.set(by);
            finder.results.set_visible(false);
            finder.load();
        }
    });
    let weak = Rc::downgrade(&finder);
    filter.connect_search_changed(move |_| {
        if let Some(finder) = weak.upgrade() {
            finder.rebuild();
        }
    });
    let weak = Rc::downgrade(&finder);
    on_selected(&keep, move |index| {
        if let (Some(finder), Some(rule)) = (weak.upgrade(), RULES.get(index)) {
            finder.keep.set(*rule);
            finder.update_footer();
        }
    });
    let weak = Rc::downgrade(&finder);
    mark.connect_clicked(move |_| {
        if let Some(finder) = weak.upgrade() {
            finder.mark();
        }
    });
    let weak = Rc::downgrade(&finder);
    trash_button.connect_clicked(move |_| {
        if let Some(finder) = weak.upgrade() {
            finder.ask_trash();
        }
    });
    let weak = Rc::downgrade(&finder);
    view.connect_activate(move |_, position| {
        if let Some(finder) = weak.upgrade() {
            finder.play(position);
        }
    });
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&finder);
    keys.connect_key_pressed(move |_, key, _, state| {
        if key == gdk::Key::space
            && state.is_empty()
            && let Some(finder) = weak.upgrade()
            && finder.toggle_selected()
        {
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    view.add_controller(keys);

    // The window keeps its state for as long as it is open.
    let keep_alive = RefCell::new(Some(finder.clone()));
    window.connect_destroy(move |_| {
        keep_alive.borrow_mut().take();
    });
    let start = MODES
        .iter()
        .position(|m| *m == MatchBy::IdenticalFiles)
        .and_then(|i| list.row_at_index(i32::try_from(i).unwrap_or(0)));
    list.select_row(start.as_ref());
    finder.update_footer();
    finder.load_counts();
    window.present();
    Some(window)
}

/// Selects the way of matching `by` in the open window (for tests and
/// screenshots).
pub(crate) fn choose(window: &gtk::Window, by: &str) {
    let index = match by {
        "same-recording" => 0,
        "same-album-track" => 1,
        "same-file" => 3,
        _ => 2,
    };
    let list = super::tool_window::side_list(window);
    if let Some(list) = list
        && let Some(row) = list.row_at_index(index)
    {
        list.select_row(Some(&row));
    }
}
