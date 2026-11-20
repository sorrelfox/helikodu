// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sync and export: mirrors chosen playlists, searches, albums and artists to a
//! folder (a phone, a memory stick, another player), converting files with a
//! converter preset on the way.
//!
//! The destinations at the left; what is copied there, what happens to the
//! files and how they are named; five figures; the table of what a sync does;
//! and "Check again" and "Sync now". Helikodu removes only files it put there
//! itself. The sync runs in the background like a conversion.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::converter::preset_name;
use super::jobs::{self, JobKind, JobView, RowState};
use super::tool_window::{GAP, cell, footer, frame, primary_button, shown_path, table_header};
use crate::backend::export::{
    ExportServices, ExportState, NamingPattern, SyncAction, SyncDestination, SyncFiles, SyncPlan,
    SyncSource,
};
use crate::backend::{Backend, PlaylistKey, PlaylistType, SavedSearchRow};
use crate::dialogs::common::{compact_button, dropdown, find_open, on_selected};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::empty_state::empty_state;
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "sync-export";

/// The width of the Action column.
const ACTION_COLUMN: i32 = 80;
/// The width of the Format column.
const FORMAT_COLUMN: i32 = 100;
/// The width of the Size column.
const SIZE_COLUMN: i32 = 80;

/// Something that can be ticked in "Copy these".
#[derive(Debug, Clone, PartialEq)]
struct Choice {
    source: SyncSource,
    name: String,
    icon: &'static str,
    count: String,
}

struct Sync {
    window: gtk::Window,
    export: Arc<dyn ExportServices>,
    state: RefCell<ExportState>,
    chosen: Cell<usize>,
    choices: RefCell<Vec<Choice>>,
    plan: RefCell<SyncPlan>,
    list: gtk::ListBox,
    title: gtk::Label,
    header_summary: gtk::Label,
    sources: gtk::ListBox,
    files: gtk::DropDown,
    pattern: gtk::Entry,
    pattern_note: gtk::Label,
    playlists: gtk::CheckButton,
    remove: gtk::CheckButton,
    figures: Vec<(gtk::Label, gtk::Label)>,
    table: gtk::ListBox,
    summary: gtk::Label,
    meter: gtk::ProgressBar,
    check: gtk::Button,
    primary: gtk::Button,
    stop: gtk::Button,
    /// The options, figures, table and footer of a destination.
    content: gtk::Box,
    /// What the window shows while there is no destination.
    empty: gtk::Box,
    filling: Cell<bool>,
    generation: Cell<u64>,
}

/// The entries of the Files dropdown for the presets `presets`: convert
/// lossless files to each, copy as they are, convert everything to each.
fn file_choices(presets: &[crate::backend::export::Preset]) -> Vec<(String, SyncFiles, String)> {
    let mut out = Vec::new();
    for p in presets {
        out.push((
            fill(&tr("Convert lossless files to {}"), &[&preset_name(p)]),
            SyncFiles::ConvertLossless,
            p.id.clone(),
        ));
    }
    out.push((
        tr("Copy the files as they are"),
        SyncFiles::CopyAsIs,
        String::new(),
    ));
    for p in presets {
        out.push((
            fill(&tr("Convert everything to {}"), &[&preset_name(p)]),
            SyncFiles::ConvertAll,
            p.id.clone(),
        ));
    }
    out
}

impl Sync {
    /// The sync job of the chosen destination, running or finished.
    fn job(&self) -> Option<JobView> {
        let id = self.destination()?.id;
        jobs::current().filter(|j| j.kind == JobKind::Sync && j.target == id)
    }

    fn destination(&self) -> Option<SyncDestination> {
        self.state
            .borrow()
            .destinations
            .get(self.chosen.get())
            .cloned()
    }

    fn change(self: &Rc<Self>, f: impl FnOnce(&mut SyncDestination)) {
        {
            let mut state = self.state.borrow_mut();
            let Some(d) = state.destinations.get_mut(self.chosen.get()) else {
                return;
            };
            f(d);
        }
        let _ = self.export.save_state(&self.state.borrow());
        self.replan();
    }

    fn fill_destinations(&self) {
        crate::widgets::remove_rows(&self.list);
        for d in &self.state.borrow().destinations {
            let row = gtk::ListBoxRow::new();
            row.add_css_class("two-line");
            let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
            text.set_valign(gtk::Align::Center);
            let connected = d.is_connected();
            let name = label(&d.name, &["hk-side-name"]);
            text.append(&name);
            let line = if connected {
                label(&shown_path(&d.path), &["hk-mono", "hk-t3", "hk-side-line"])
            } else {
                name.add_css_class("hk-t3");
                label(&tr("Not connected"), &["hk-t3", "hk-side-line"])
            };
            line.set_ellipsize(gtk::pango::EllipsizeMode::Start);
            text.append(&line);
            row.set_child(Some(&text));
            row.update_property(&[gtk::accessible::Property::Label(&d.name)]);
            self.list.append(&row);
        }
        let has = !self.state.borrow().destinations.is_empty();
        self.content.set_visible(has);
        self.empty.set_visible(!has);
        self.filling.set(true);
        if has {
            let index = i32::try_from(self.chosen.get()).unwrap_or(0);
            self.list.select_row(self.list.row_at_index(index).as_ref());
        } else {
            self.list.unselect_all();
        }
        self.filling.set(false);
    }

    /// Shows the chosen destination's choices.
    fn show(self: &Rc<Self>) {
        let Some(d) = self.destination() else {
            self.title.set_text(&tr("No destination"));
            self.header_summary.set_text("");
            return;
        };
        self.title.set_text(&d.name);
        self.header_summary.set_text(&match d.last_synced {
            Some(seconds) => fill(
                &tr("Last synced on {}"),
                &[&super::tool_window::date(Some(
                    std::time::UNIX_EPOCH
                        + std::time::Duration::from_secs(u64::try_from(seconds).unwrap_or(0)),
                ))],
            ),
            None => tr("Not synced yet"),
        });
        self.filling.set(true);
        let presets = self.state.borrow().all_presets();
        let index = file_choices(&presets)
            .iter()
            .position(|(_, files, preset)| {
                *files == d.files && (d.files == SyncFiles::CopyAsIs || *preset == d.preset)
            })
            .unwrap_or(0);
        self.files.set_selected(index as u32);
        self.pattern.set_text(&d.pattern);
        self.playlists.set_active(d.write_playlists);
        self.remove.set_active(d.remove_unchosen);
        self.filling.set(false);
        self.fill_sources();
        self.replan();
    }

    fn fill_sources(self: &Rc<Self>) {
        crate::widgets::remove_rows(&self.sources);
        let Some(d) = self.destination() else { return };
        let mut choices = self.choices.borrow().clone();
        // Albums, artists and searches the destination has besides.
        for source in &d.sources {
            if !choices.iter().any(|c| &c.source == source) {
                let (name, icon) = match source {
                    SyncSource::Query { text } => (text.clone(), "edit-find-symbolic"),
                    SyncSource::Album { .. } => (tr("An album"), "media-optical-symbolic"),
                    SyncSource::Artist { .. } => (tr("An artist"), "avatar-default-symbolic"),
                    _ => (tr("Gone from the library"), "helikodu-playlist-symbolic"),
                };
                choices.push(Choice {
                    source: source.clone(),
                    name,
                    icon,
                    count: String::new(),
                });
            }
        }
        for choice in &choices {
            let row = gtk::ListBoxRow::new();
            row.set_activatable(false);
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            line.add_css_class("hk-sync-source");
            let check = gtk::CheckButton::new();
            check.set_active(d.sources.contains(&choice.source));
            check.update_property(&[gtk::accessible::Property::Label(&fill(
                &tr("Copy {}"),
                &[&choice.name],
            ))]);
            line.append(&check);
            line.append(&gtk::Image::from_icon_name(choice.icon));
            let name = label(&choice.name, &[]);
            name.set_hexpand(true);
            line.append(&name);
            line.append(&label(&choice.count, &["hk-t3", "hk-count"]));
            row.set_child(Some(&line));
            let source = choice.source.clone();
            let weak = Rc::downgrade(self);
            check.connect_toggled(move |check| {
                let Some(this) = weak.upgrade() else { return };
                let on = check.is_active();
                let source = source.clone();
                this.change(move |d| {
                    d.sources.retain(|s| *s != source);
                    if on {
                        d.sources.push(source);
                    }
                });
            });
            self.sources.append(&row);
        }
        let add = gtk::ListBoxRow::new();
        add.set_widget_name("add-source");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.add_css_class("hk-sync-source");
        line.append(&gtk::Image::from_icon_name("list-add-symbolic"));
        line.append(&label(
            &tr("Add an album, an artist or a search…"),
            &["hk-t2"],
        ));
        add.set_child(Some(&line));
        add.update_property(&[gtk::accessible::Property::Label(&tr(
            "Add an album, an artist or a search…",
        ))]);
        self.sources.append(&add);
    }

    /// Asks for a search in the query language and adds it.
    fn add_search(self: &Rc<Self>, anchor: &gtk::Widget) {
        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.set_margin_top(10);
        content.set_margin_bottom(10);
        content.set_margin_start(10);
        content.set_margin_end(10);
        let entry = gtk::Entry::new();
        entry.add_css_class("hk-input");
        entry.add_css_class("hk-query");
        entry.set_width_chars(36);
        entry.set_placeholder_text(Some("album:\"Tidewater Rooms\"  artist:Okto"));
        entry.update_property(&[gtk::accessible::Property::Label(&tr(
            "An album, an artist or a search",
        ))]);
        content.append(&label(
            &tr("Type a search: album:, artist: and the other fields of the search work."),
            &["hk-t3"],
        ));
        content.append(&entry);
        popover.set_child(Some(&content));
        let weak = Rc::downgrade(self);
        let pop = popover.downgrade();
        entry.connect_activate(move |entry| {
            let text = entry.text().trim().to_owned();
            if let (Some(this), false) = (weak.upgrade(), text.is_empty()) {
                this.change(|d| d.sources.push(SyncSource::Query { text }));
                this.fill_sources();
            }
            if let Some(p) = pop.upgrade() {
                p.popdown();
            }
        });
        popover.connect_closed(|p| p.unparent());
        popover.popup();
    }

    fn replan(self: &Rc<Self>) {
        if self.filling.get() {
            return;
        }
        if self.job().is_some_and(|j| j.is_running()) {
            self.update_footer(self.job().as_ref());
            return;
        }
        let Some(d) = self.destination() else {
            self.plan.replace(SyncPlan::default());
            self.rebuild();
            return;
        };
        match NamingPattern::parse(&d.pattern) {
            Ok(_) => {
                self.pattern_note.set_text("");
                self.pattern_note.remove_css_class("hk-query-problem");
                crate::widgets::pattern_entry::underline(&self.pattern, None);
            }
            Err(e) => {
                crate::widgets::pattern_entry::underline(&self.pattern, Some((e.start, e.end)));
                self.pattern_note.set_text(&super::naming_problem(&e));
                self.pattern_note.add_css_class("hk-query-problem");
            }
        }
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let export = self.export.clone();
        let weak = Rc::downgrade(self);
        self.summary.set_text(&tr("Looking at the destination…"));
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || export.plan_sync(&d)).await;
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            match result {
                Ok(Ok(plan)) => {
                    this.plan.replace(plan);
                }
                Ok(Err(error)) => {
                    this.plan.replace(SyncPlan::default());
                    this.summary.set_text(&error.0);
                }
                Err(_) => {}
            }
            this.rebuild();
        });
    }

    fn rebuild(self: &Rc<Self>) {
        crate::widgets::remove_rows(&self.table);
        let plan = self.plan.borrow();
        let job = self.job();
        for (index, item) in plan.items.iter().enumerate() {
            let state = job.as_ref().and_then(|j| j.rows.get(index).cloned());
            self.table.append(&sync_row(item, state.as_ref()));
        }
        let values = [
            format::count(plan.to_copy),
            format::count(plan.to_convert),
            format::count(plan.already_there),
            format::count(plan.to_remove),
            format::size(plan.bytes_needed),
        ];
        for ((value, caption), text) in self.figures.iter().zip(values) {
            value.set_text(&text);
            let _ = caption;
        }
        if let Some((_, caption)) = self.figures.last() {
            caption.set_text(&match plan.bytes_free {
                Some(free) => fill(&tr("needed · {} free"), &[&format::size(free)]),
                None => tr("needed"),
            });
        }
        drop(plan);
        self.update_footer(job.as_ref());
    }

    fn update_footer(&self, job: Option<&JobView>) {
        let id = self.destination().map(|d| d.id).unwrap_or_default();
        let job = job.filter(|j| j.kind == JobKind::Sync && j.target == id);
        let connected = self.destination().is_some_and(|d| d.is_connected());
        match job {
            Some(job) if job.is_running() => {
                self.meter.set_visible(true);
                self.meter.set_fraction(job.fraction());
                self.summary.set_text(&fill(
                    &tr("{} of {} · {} written"),
                    &[
                        &format::count(job.done()),
                        &format::count(job.rows.len()),
                        &format::size(job.bytes),
                    ],
                ));
                self.primary.set_visible(false);
                self.check.set_visible(false);
                self.stop.set_visible(true);
            }
            _ => {
                self.meter.set_visible(false);
                self.stop.set_visible(false);
                self.check.set_visible(true);
                self.primary.set_visible(true);
                let foreign = self.plan.borrow().foreign;
                let mut text = tr(
                    "Nothing changes in the library. Files on the destination that Helikodu did not put there are left alone.",
                );
                if let Some(summary) = job.and_then(|j| j.finished.clone()) {
                    text = fill(
                        &ntr(
                            // Translators: after a sync: {0} is the number of
                            // files written, {1} their size ("1.2 GB").
                            "Synced: {} file · {}",
                            "Synced: {} files · {}",
                            summary.written,
                        ),
                        &[
                            &format::count(summary.written),
                            &format::size(summary.bytes),
                        ],
                    );
                    if summary.failed > 0 {
                        text = fill(
                            &tr("{} · {}"),
                            &[
                                &text,
                                &fill(
                                    &ntr("{} failed", "{} failed", summary.failed),
                                    &[&format::count(summary.failed)],
                                ),
                            ],
                        );
                    }
                } else if !connected {
                    text = tr("The destination is not connected. Connect it to sync.");
                } else if foreign > 0 {
                    text = fill(
                        &ntr(
                            "{} file Helikodu did not put there stands in the way and is left alone.",
                            "{} files Helikodu did not put there stand in the way and are left alone.",
                            foreign,
                        ),
                        &[&format::count(foreign)],
                    );
                }
                self.summary.set_text(&text);
                let work = !self.plan.borrow().items.is_empty();
                self.primary.set_sensitive(connected && work);
            }
        }
    }

    fn start(self: &Rc<Self>) {
        let Some(d) = self.destination() else { return };
        let rows = self.plan.borrow().items.len();
        let export = self.export.clone();
        let workers = self.state.borrow().converter.workers;
        let target = d.id.clone();
        if let Err(error) = jobs::start(JobKind::Sync, target, rows, move |events| {
            export.start_sync(&d, workers, events)
        }) {
            self.summary.set_text(&error.0);
        }
    }

    /// Asks for the folder of a new destination and adds it. Only a click on
    /// "New destination" or "Add destination…" (or Enter on one) asks; Cancel
    /// leaves everything as it was and asks nothing more.
    fn new_destination(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        crate::chooser::select_folder(
            &self.window,
            &tr("Choose the folder to sync to"),
            None,
            None,
            move |path| {
                let (Some(this), Some(path)) = (weak.upgrade(), path) else {
                    return;
                };
                this.add_destination(path);
            },
        );
    }

    fn add_destination(self: &Rc<Self>, path: std::path::PathBuf) {
        {
            let mut state = self.state.borrow_mut();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| tr("Destination"));
            let id = state.new_destination_id();
            let mut d = SyncDestination::new(id, name, path);
            d.preset = state.converter.preset.clone();
            state.destinations.push(d);
            self.chosen.set(state.destinations.len() - 1);
        }
        let _ = self.export.save_state(&self.state.borrow());
        self.fill_destinations();
        self.show();
    }
}

/// A row of the table.
fn sync_row(item: &crate::backend::export::SyncItem, state: Option<&RowState>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
    row.add_css_class("hk-sync-row");
    let removal = item.action == SyncAction::Remove;
    let action = match item.action {
        SyncAction::Convert => tr("Convert"),
        SyncAction::Remove => tr("Remove"),
        _ => tr("Copy"),
    };
    let action_cell = cell(ACTION_COLUMN, &[], false);
    action_cell.set_text(&action);
    row.append(&action_cell);
    let path = cell(-1, &["hk-mono", "hk-tool-path"], false);
    path.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    path.set_max_width_chars(1);
    let shown = item.relative.to_string_lossy().into_owned();
    path.set_text(&shown);
    path.set_tooltip_text(Some(&shown));
    if removal {
        path.add_css_class("hk-t3");
    }
    row.append(&path);
    let from = item.from_codec.to_uppercase();
    let format_text = match item.action {
        SyncAction::Convert => format!("{from} → {}", item.to_format),
        SyncAction::Playlist => tr("Playlist"),
        _ => from,
    };
    let format_cell = cell(FORMAT_COLUMN, &["hk-mono-meta", "hk-t3"], false);
    format_cell.set_text(&format_text);
    row.append(&format_cell);
    let size = cell(SIZE_COLUMN, &["hk-numeric"], true);
    size.set_text(&match state {
        Some(RowState::Working(f)) => format!("{:.0} %", f * 100.0),
        Some(RowState::Written(_)) => format!("✓  {}", format::size(item.bytes)),
        Some(RowState::Failed(_)) => tr("Failed"),
        Some(RowState::Skipped) => tr("Left alone"),
        _ => format::size(item.bytes),
    });
    if let Some(RowState::Failed(why)) = state {
        size.set_tooltip_text(Some(why));
    }
    row.append(&size);
    row.update_property(&[gtk::accessible::Property::Label(&fill(
        &tr("{} {}, {}, {}"),
        &[&action, &shown, &format_text, &size.text()],
    ))]);
    row
}

/// Opens Sync and export over `main`, or brings it to the front.
pub(crate) fn open(main: &MainWindow) -> Option<gtk::Window> {
    open_with(main, None)
}

/// Opens Sync and export with the playlist `playlist` ticked for the chosen
/// destination (a playlist's "Copy the files to a folder…").
pub(crate) fn open_with(main: &MainWindow, playlist: Option<PlaylistKey>) -> Option<gtk::Window> {
    let parent = main.upcast_ref::<gtk::Window>();
    if let Some(window) = find_open(parent, NAME) {
        window.present();
        return Some(window);
    }
    let backend: Backend = main.backend()?;
    let export = backend.services().export();
    let mut state = export.state();
    let frame = frame(parent, NAME, &tr("Sync and export"), &tr("Destinations"));
    frame.side_note.set_text(&tr(
        "Copies go one way: from the library to the destination.",
    ));
    // "New destination" is a button under the destinations, not a row of
    // their list: selecting rows, the arrow keys and the focus coming back
    // to the window never ask for a folder.
    let new_destination = gtk::Button::new();
    new_destination.set_widget_name("new-destination");
    new_destination.add_css_class("hk-side-add");
    let new_line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    new_line.append(&gtk::Image::from_icon_name("list-add-symbolic"));
    new_line.append(&gtk::Label::new(Some(&tr("New destination"))));
    new_destination.set_child(Some(&new_line));
    new_destination.update_property(&[gtk::accessible::Property::Label(&tr("New destination"))]);
    new_destination.set_margin_start(8);
    new_destination.set_margin_end(8);
    new_destination.set_vexpand(true);
    new_destination.set_valign(gtk::Align::Start);
    frame.list.set_vexpand(false);
    if let Some(side) = frame.list.parent().and_downcast::<gtk::Box>() {
        side.insert_child_after(&new_destination, Some(&frame.list));
    }

    let add_destination = primary_button(&tr("Add destination…"));
    add_destination.set_widget_name("add-destination");
    let empty = empty_state(
        "folder-symbolic",
        &tr("No destinations yet"),
        &tr("Add a folder, a phone or a memory stick to keep in step with your playlists."),
        std::slice::from_ref(&add_destination),
    );
    empty.set_visible(false);
    frame.main.append(&empty);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.set_vexpand(true);
    frame.main.append(&column);

    // What goes there, and how.
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    top.add_css_class("hk-tool-options");
    let left = gtk::Box::new(gtk::Orientation::Vertical, 8);
    left.set_hexpand(true);
    left.append(&label(&tr("Copy these"), &["hk-section-label"]));
    let sources = gtk::ListBox::new();
    sources.add_css_class("hk-sync-sources");
    sources.set_selection_mode(gtk::SelectionMode::None);
    sources.set_widget_name("sync-sources");
    sources.update_property(&[gtk::accessible::Property::Label(&tr("Copy these"))]);
    let sources_scroll = gtk::ScrolledWindow::new();
    sources_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    sources_scroll.set_propagate_natural_height(true);
    sources_scroll.set_max_content_height(186);
    sources_scroll.set_child(Some(&sources));
    left.append(&sources_scroll);
    top.append(&left);
    let right = gtk::Box::new(gtk::Orientation::Vertical, 8);
    right.set_hexpand(true);
    right.append(&label(&tr("Files"), &["hk-section-label"]));
    let presets = state.all_presets();
    let names: Vec<String> = file_choices(&presets)
        .into_iter()
        .map(|(n, _, _)| n)
        .collect();
    let files = dropdown(&names, 0, -1);
    files.set_halign(gtk::Align::Fill);
    files.set_widget_name("sync-files");
    files.update_property(&[gtk::accessible::Property::Label(&tr("Files"))]);
    right.append(&files);
    let caption = label(&tr("Folders and names"), &["hk-field-label"]);
    caption.set_margin_top(6);
    right.append(&caption);
    let pattern_entry = crate::widgets::pattern_entry::PatternEntry::new("sync-pattern");
    pattern_entry.labelled_by(&caption);
    let pattern = pattern_entry.entry.clone();
    pattern.add_css_class("hk-query");
    right.append(&pattern_entry.root);
    let pattern_note = label("", &["hk-t3"]);
    right.append(&pattern_note);
    let playlists = gtk::CheckButton::with_label(&tr("Write each playlist as an .m3u8 file"));
    playlists.set_widget_name("write-playlists");
    right.append(&playlists);
    let remove = gtk::CheckButton::with_label(&tr("Remove files that are no longer chosen"));
    remove.set_widget_name("remove-unchosen");
    right.append(&remove);
    top.append(&right);
    column.append(&top);

    // The five figures.
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    strip.set_homogeneous(true);
    strip.add_css_class("hk-sync-figures");
    let mut figures = Vec::new();
    for caption in [
        tr("to copy"),
        tr("to convert"),
        tr("already there"),
        tr("to remove"),
        tr("needed"),
    ] {
        let figure = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let value = label("0", &["hk-sync-figure"]);
        let text = label(&caption, &["hk-t3", "hk-sync-caption"]);
        figure.append(&value);
        figure.append(&text);
        strip.append(&figure);
        figures.push((value, text));
    }
    column.append(&strip);

    column.append(&table_header(&[
        (tr("Action"), ACTION_COLUMN, false),
        (tr("On the destination"), -1, false),
        (tr("Format"), FORMAT_COLUMN, false),
        (tr("Size"), SIZE_COLUMN, true),
    ]));
    let table = gtk::ListBox::new();
    table.add_css_class("hk-dup-table");
    table.set_widget_name("sync-table");
    table.update_property(&[gtk::accessible::Property::Label(&tr("What a sync does"))]);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&table));
    column.append(&scroller);

    let (bar, summary) = footer();
    summary.set_wrap(true);
    summary.set_single_line_mode(false);
    let meter = gtk::ProgressBar::new();
    meter.add_css_class("hk-tool-meter");
    meter.set_size_request(180, -1);
    meter.set_valign(gtk::Align::Center);
    meter.set_visible(false);
    bar.prepend(&meter);
    let check = compact_button(&tr("Check again"));
    check.set_widget_name("check-again");
    bar.append(&check);
    let stop = compact_button(&tr("Stop"));
    stop.set_widget_name("stop");
    stop.set_visible(false);
    bar.append(&stop);
    let primary = primary_button(&tr("Sync now"));
    primary.set_widget_name("sync-now");
    bar.append(&primary);
    column.append(&bar);

    // A playlist's "Copy the files to a folder…" ticks it.
    if let Some(key) = playlist {
        let source = SyncSource::Playlist {
            id: i64::try_from(key.0).unwrap_or(0),
        };
        if state.destinations.is_empty() {
            let id = state.new_destination_id();
            let mut d = SyncDestination::new(
                id,
                tr("Folder"),
                glib::home_dir().join("Music").join("Export"),
            );
            d.preset = state.converter.preset.clone();
            state.destinations.push(d);
        }
        if let Some(d) = state.destinations.first_mut()
            && !d.sources.contains(&source)
        {
            d.sources.push(source);
        }
        let _ = export.save_state(&state);
    }

    let sync = Rc::new(Sync {
        window: frame.window.clone(),
        export,
        state: RefCell::new(state),
        chosen: Cell::new(0),
        choices: RefCell::new(Vec::new()),
        plan: RefCell::new(SyncPlan::default()),
        list: frame.list.clone(),
        title: frame.title.clone(),
        header_summary: frame.summary.clone(),
        sources: sources.clone(),
        files: files.clone(),
        pattern: pattern.clone(),
        pattern_note,
        playlists: playlists.clone(),
        remove: remove.clone(),
        figures,
        table,
        summary,
        meter,
        check: check.clone(),
        primary: primary.clone(),
        stop: stop.clone(),
        content: column,
        empty,
        filling: Cell::new(false),
        generation: Cell::new(0),
    });

    let weak = Rc::downgrade(&sync);
    frame.list.connect_row_selected(move |_, row| {
        let (Some(this), Some(row)) = (weak.upgrade(), row) else {
            return;
        };
        if this.filling.get() {
            return;
        }
        this.chosen.set(usize::try_from(row.index()).unwrap_or(0));
        this.show();
    });
    for button in [&new_destination, &add_destination] {
        let weak = Rc::downgrade(&sync);
        button.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.new_destination();
            }
        });
    }
    let weak = Rc::downgrade(&sync);
    sources.connect_row_activated(move |_, row| {
        if let Some(this) = weak.upgrade()
            && row.widget_name() == "add-source"
        {
            this.add_search(row.upcast_ref());
        }
    });
    sources.connect_row_selected(|_, _| {});
    let weak = Rc::downgrade(&sync);
    on_selected(&files, move |index| {
        let Some(this) = weak.upgrade() else { return };
        if this.filling.get() {
            return;
        }
        let presets = this.state.borrow().all_presets();
        if let Some((_, files, preset)) = file_choices(&presets).get(index).cloned() {
            this.change(move |d| {
                d.files = files;
                if !preset.is_empty() {
                    d.preset = preset;
                }
            });
        }
    });
    let weak = Rc::downgrade(&sync);
    pattern.connect_changed(move |entry| {
        let Some(this) = weak.upgrade() else { return };
        if this.filling.get() {
            return;
        }
        let text = entry.text().to_string();
        this.change(move |d| d.pattern = text);
    });
    let weak = Rc::downgrade(&sync);
    playlists.connect_toggled(move |c| {
        let Some(this) = weak.upgrade() else { return };
        if !this.filling.get() {
            let on = c.is_active();
            this.change(move |d| d.write_playlists = on);
        }
    });
    let weak = Rc::downgrade(&sync);
    remove.connect_toggled(move |c| {
        let Some(this) = weak.upgrade() else { return };
        if !this.filling.get() {
            let on = c.is_active();
            this.change(move |d| d.remove_unchosen = on);
        }
    });
    let weak = Rc::downgrade(&sync);
    check.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            jobs::clear();
            this.replan();
        }
    });
    let weak = Rc::downgrade(&sync);
    primary.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.start();
        }
    });
    let weak = Rc::downgrade(&sync);
    stop.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.export.stop();
        }
    });
    let weak = Rc::downgrade(&sync);
    let listener = jobs::listen(move |job, _| {
        let Some(this) = weak.upgrade() else { return };
        if job.kind != JobKind::Sync {
            return;
        }
        if job.is_running() {
            this.update_footer(Some(job));
        } else {
            this.rebuild();
        }
    });
    let keep_alive = RefCell::new(Some(sync.clone()));
    frame.window.connect_destroy(move |_| {
        jobs::unlisten(listener);
        keep_alive.borrow_mut().take();
    });

    // "Copy these": the playlists, folders and smart playlists, then the saved
    // searches.
    let rows = main.playlist_rows();
    let mut choices: Vec<Choice> = rows
        .iter()
        .map(|p| {
            let (icon, count) = match p.kind {
                PlaylistType::Folder => {
                    let n = rows.iter().filter(|c| c.parent == Some(p.key)).count();
                    (
                        "folder-symbolic",
                        fill(&ntr("{} playlist", "{} playlists", n), &[&format::count(n)]),
                    )
                }
                PlaylistType::Smart => ("helikodu-funnel-symbolic", format::count(p.items)),
                PlaylistType::Static => ("helikodu-playlist-symbolic", format::count(p.items)),
            };
            Choice {
                source: SyncSource::Playlist {
                    id: i64::try_from(p.key.0).unwrap_or(0),
                },
                name: p.name.clone(),
                icon,
                count,
            }
        })
        .collect();
    let weak = Rc::downgrade(&sync);
    glib::spawn_future_local(async move {
        let searches: Vec<SavedSearchRow> = backend
            .query(|l| l.saved_searches())
            .await
            .unwrap_or_default();
        let Some(this) = weak.upgrade() else { return };
        choices.extend(searches.into_iter().map(|s| Choice {
            source: SyncSource::SavedSearch {
                id: i64::try_from(s.key.0).unwrap_or(0),
            },
            name: s.name,
            icon: "edit-find-symbolic",
            count: format::count(s.count),
        }));
        this.choices.replace(choices);
        this.fill_sources();
    });
    sync.fill_destinations();
    sync.show();
    frame.window.present();
    Some(frame.window)
}
