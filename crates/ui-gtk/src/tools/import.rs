// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Import from another player: a tool window that shows what an import would
//! do before anything changes.
//!
//! The left pane lists the players whose library was found on this computer
//! (Flatpak folders included), iTunes or Music and a Helikodu data file, which
//! are chosen as files, and the earlier imports, among them statistics carried
//! to replacement files. A source is read on a thread of its own while the
//! window shows that it works; the figures, "What changes", the playlists, the
//! fields that differ and the entries left out follow. Options and folders
//! that moved plan again without reading the file again. "Import for N
//! tracks" counts the matched tracks that would change.
//!
//! An import is undone only by "Undo this import" (the newest import that is
//! not undone) or by the Undo of its toast; Ctrl+Z does not undo imports, as
//! they change the database only, often for thousands of tracks.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::file_tools::run_in_background;
use super::import_moved;
use super::import_view::{
    Figures, LISTED, change_rows, counted, favourites_row, field_box, list_popover, listed_head,
    notice, playlist_row,
};
use super::tool_window::{Frame, date, footer, frame, primary_button};
use crate::backend::interchange::{
    ImportFrom, ImportPreview, ImportProblem, ImportRunKind, ImportRunRow, InterchangeServices,
    MergeOptions, PlannedPlaylist, Player, PlayerSource,
};
use crate::dialogs::common::{compact_button, find_open, tilde};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "import";

/// What the window opens with chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Start {
    /// A player's library.
    Player(Player),
    /// A Helikodu data file, to be chosen.
    DataFile,
}

/// What is chosen in the left pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chosen {
    Nothing,
    /// A row of "From": a player by its index, or the data file after them.
    Source(usize),
    /// An earlier import.
    Run(i64),
}

/// What the right side shows for a source.
#[derive(Debug, Clone)]
enum Shown {
    Reading,
    Plan(Box<ImportPreview>),
    Problem(ImportProblem),
}

struct Import {
    window: gtk::Window,
    main: glib::WeakRef<MainWindow>,
    services: Arc<dyn InterchangeServices>,
    players: Vec<PlayerSource>,
    /// The files chosen for the rows of "From", by index.
    chosen_files: RefCell<HashMap<usize, PathBuf>>,
    runs: RefCell<Vec<ImportRunRow>>,
    chosen: Cell<Chosen>,
    options: RefCell<MergeOptions>,
    shown: RefCell<Option<Shown>>,
    /// The number of the newest plan asked for; older answers are dropped.
    asked: Cell<u64>,
    filling: Cell<bool>,
    applying: Cell<bool>,
    // Widgets.
    from_list: gtk::ListBox,
    from_rows: Vec<(gtk::ListBoxRow, gtk::Label)>,
    runs_list: gtk::ListBox,
    title: gtk::Label,
    header_summary: gtk::Label,
    options_box: gtk::Box,
    playlists_check: gtk::CheckButton,
    pairs: gtk::Box,
    add_pair: gtk::Button,
    pairs_hint: gtk::Label,
    moved_notice: gtk::Box,
    moved_message: gtk::Label,
    problem: gtk::Box,
    problem_message: gtk::Label,
    problem_line: gtk::Label,
    problem_choose: gtk::Button,
    figures: Figures,
    body: gtk::Stack,
    working_text: gtk::Label,
    empty_text: gtk::Label,
    changes_head: gtk::Label,
    changes: gtk::Box,
    playlists_note: gtk::Label,
    playlists: gtk::Box,
    fields: gtk::Box,
    fields_list: gtk::Box,
    carried_text: gtk::Label,
    summary: gtk::Label,
    import: gtk::Button,
    undo: gtk::Button,
}

/// A player's name as the window shows it.
pub(crate) fn player_name(player: Player) -> String {
    match player {
        Player::ITunes => tr("iTunes or Music"),
        other => other.name().to_owned(),
    }
}

/// The place of a library as the left pane shows it: its folder, and for a
/// Flatpak the application's folder.
fn place(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(at) = text.find("/.var/app/") {
        let rest = &text[at + "/.var/app/".len()..];
        let app = rest.split('/').next().unwrap_or(rest);
        return tilde(&PathBuf::from(&text[..at]).join(".var/app").join(app));
    }
    path.parent().map_or_else(|| tilde(path), tilde)
}

/// "rhythmdb.xml and playlists.xml".
fn files_text(files: &[String]) -> String {
    match files {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => fill(&tr("{} and {}"), &[first, second]),
        more => more.join(", "),
    }
}

/// "today at 14:32" or the date and time.
fn when(time: SystemTime) -> String {
    crate::dialogs::maintenance::when(time)
}

/// Whether `time` is today.
fn is_today(time: SystemTime) -> bool {
    let seconds = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (Ok(at), Ok(now)) = (
        glib::DateTime::from_unix_local(i64::try_from(seconds).unwrap_or(0)),
        glib::DateTime::now_local(),
    ) else {
        return false;
    };
    at.year() == now.year() && at.day_of_year() == now.day_of_year()
}

/// The name of a run's source: the player, "Helikodu data" or "Replaced files".
fn run_name(run: &ImportRunRow) -> String {
    match run.kind {
        ImportRunKind::Carry => tr("Replaced files"),
        ImportRunKind::UserData => tr("Helikodu data"),
        ImportRunKind::Player => run.player.map_or_else(|| tr("Another player"), player_name),
    }
}

/// A left-pane row: the name over a second line, mono for a path.
fn source_row(name: &str) -> (gtk::ListBoxRow, gtk::Label) {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("two-line");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_valign(gtk::Align::Center);
    text.append(&label(name, &["hk-side-name"]));
    let line = label("", &["hk-t3", "hk-side-line"]);
    line.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    text.append(&line);
    row.set_child(Some(&text));
    row.update_property(&[gtk::accessible::Property::Label(name)]);
    (row, line)
}

fn set_mono(line: &gtk::Label, mono: bool) {
    for class in ["hk-mono", "hk-side-mono"] {
        if mono {
            line.add_css_class(class);
        } else {
            line.remove_css_class(class);
        }
    }
}

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

impl Import {
    fn data_index(&self) -> usize {
        self.players.len()
    }

    /// The file of the row `index` of "From", found or chosen.
    fn file_of(&self, index: usize) -> Option<PathBuf> {
        if let Some(path) = self.chosen_files.borrow().get(&index) {
            return Some(path.clone());
        }
        self.players.get(index).and_then(|p| p.found.clone())
    }

    fn source_at(&self, index: usize) -> Option<ImportFrom> {
        let path = self.file_of(index)?;
        Some(match self.players.get(index) {
            Some(source) => ImportFrom::Player {
                player: source.player,
                path,
            },
            None => ImportFrom::DataFile(path),
        })
    }

    fn source_name(&self, index: usize) -> String {
        self.players
            .get(index)
            .map_or_else(|| tr("A Helikodu data file"), |p| player_name(p.player))
    }

    /// Fills in the second lines of "From".
    fn update_from_rows(&self) {
        for (index, (row, line)) in self.from_rows.iter().enumerate() {
            let chosen = self.chosen_files.borrow().get(&index).cloned();
            let player = self.players.get(index);
            let found = player.and_then(|p| p.found.clone());
            let (text, mono, missing) = match (chosen, found) {
                (Some(path), _) => (tilde(&path), true, false),
                (None, Some(path)) => (place(&path), true, false),
                (None, None) => match player.map(|p| p.player) {
                    Some(Player::ITunes) => (tr("Choose its Library.xml…"), false, false),
                    Some(_) => (tr("Not found on this computer"), false, true),
                    None => (tr("Choose a file…"), false, false),
                },
            };
            line.set_text(&text);
            line.set_tooltip_text(mono.then_some(text.as_str()));
            set_mono(line, mono);
            if missing {
                row.add_css_class("hk-import-missing");
            } else {
                row.remove_css_class("hk-import-missing");
            }
        }
    }

    /// Fills "Earlier imports" from `runs`, selecting the chosen run.
    fn fill_runs(&self) {
        self.filling.set(true);
        while let Some(row) = self.runs_list.first_child() {
            self.runs_list.remove(&row);
        }
        for run in self.runs.borrow().iter() {
            let day = if is_today(run.at) {
                let clock = glib::DateTime::from_unix_local(
                    i64::try_from(
                        run.at
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .map_or(0, |d| d.as_secs()),
                    )
                    .unwrap_or(0),
                )
                .ok()
                .and_then(|t| t.format("%H:%M").ok())
                .map(|t| t.to_string())
                .unwrap_or_default();
                fill(&tr("today, {}"), &[&clock])
            } else {
                date(Some(run.at))
            };
            let name = fill(&tr("{} · {}"), &[&run_name(run), &day]);
            let (row, line) = source_row(&name);
            row.set_widget_name(&format!("import-run-{}", run.id));
            if run.undone {
                line.set_text(&tr("Undone"));
                row.add_css_class("hk-import-undone");
            } else {
                line.set_text(&fill(
                    &ntr("{} track", "{} tracks", run.tracks),
                    &[&format::count(run.tracks)],
                ));
            }
            self.runs_list.append(&row);
            if self.chosen.get() == Chosen::Run(run.id) {
                self.runs_list.select_row(Some(&row));
            }
        }
        self.runs_list.set_visible(!self.runs.borrow().is_empty());
        if let Some(head) = self.runs_list.prev_sibling() {
            head.set_visible(!self.runs.borrow().is_empty());
        }
        self.filling.set(false);
    }

    /// Reads the earlier imports again, then runs `then`.
    fn load_runs(self: &Rc<Self>, then: impl FnOnce(&Rc<Self>) + 'static) {
        let services = self.services.clone();
        let weak = Rc::downgrade(self);
        run_in_background(
            move |_| services.import_runs(),
            |_, _| {},
            move |runs| {
                let Some(this) = weak.upgrade() else { return };
                match runs {
                    Ok(runs) => {
                        this.runs.replace(runs);
                    }
                    Err(error) => this.summary.set_text(&error.0),
                }
                this.fill_runs();
                then(&this);
            },
        );
    }

    /// Chooses the row `index` of "From": plans at once when its file is
    /// known, or asks for the file first.
    fn choose_source(self: &Rc<Self>, index: usize, ask: bool) {
        if !ask && self.file_of(index).is_some() {
            self.chosen.set(Chosen::Source(index));
            self.select_left();
            self.plan();
            return;
        }
        let name = self.source_name(index);
        let (title, kind, patterns): (String, String, &[&str]) = match self.players.get(index) {
            Some(source) => match source.player {
                Player::ITunes => (
                    tr("Choose the Library.xml of iTunes or Music"),
                    tr("iTunes or Music library"),
                    &["*.xml"],
                ),
                Player::Rhythmbox => (
                    fill(&tr("Choose the library of {}"), &[&name]),
                    fill(&tr("{} library"), &[&name]),
                    &["*.xml"],
                ),
                _ => (
                    fill(&tr("Choose the library of {}"), &[&name]),
                    fill(&tr("{} library"), &[&name]),
                    &["*.db"],
                ),
            },
            None => (
                tr("Choose a Helikodu data file"),
                tr("Helikodu data file"),
                &["*.json"],
            ),
        };
        let weak = Rc::downgrade(self);
        crate::chooser::open_file(&self.window, &title, &kind, patterns, move |path| {
            let Some(this) = weak.upgrade() else { return };
            match path {
                Some(path) => {
                    this.chosen_files.borrow_mut().insert(index, path);
                    this.update_from_rows();
                    this.chosen.set(Chosen::Source(index));
                    this.select_left();
                    this.plan();
                }
                None => this.select_left(),
            }
        });
    }

    /// Selects the left-pane row of what is chosen.
    fn select_left(&self) {
        self.filling.set(true);
        match self.chosen.get() {
            Chosen::Source(index) => {
                self.runs_list.unselect_all();
                if let Some((row, _)) = self.from_rows.get(index) {
                    self.from_list.select_row(Some(row));
                }
            }
            Chosen::Run(id) => {
                self.from_list.unselect_all();
                let position = self.runs.borrow().iter().position(|r| r.id == id);
                if let Some(row) = position
                    .and_then(|p| i32::try_from(p).ok())
                    .and_then(|p| self.runs_list.row_at_index(p))
                {
                    self.runs_list.select_row(Some(&row));
                }
            }
            Chosen::Nothing => {
                self.from_list.unselect_all();
                self.runs_list.unselect_all();
            }
        }
        self.filling.set(false);
    }

    /// Plans the chosen source with the options, on a thread of its own.
    fn plan(self: &Rc<Self>) {
        let Chosen::Source(index) = self.chosen.get() else {
            return;
        };
        let Some(from) = self.source_at(index) else {
            return;
        };
        let asked = self.asked.get() + 1;
        self.asked.set(asked);
        self.shown.replace(Some(Shown::Reading));
        self.render();
        let services = self.services.clone();
        let options = self.options.borrow().clone();
        let weak = Rc::downgrade(self);
        run_in_background(
            move |_| services.plan_import(&from, &options),
            |_, _| {},
            move |result| {
                let Some(this) = weak.upgrade() else { return };
                if this.asked.get() != asked {
                    return;
                }
                this.shown.replace(Some(match result {
                    Ok(preview) => Shown::Plan(Box::new(preview)),
                    Err(problem) => Shown::Problem(problem),
                }));
                this.render();
            },
        );
    }

    fn preview(&self) -> Option<ImportPreview> {
        match self.shown.borrow().as_ref() {
            Some(Shown::Plan(preview)) => Some((**preview).clone()),
            _ => None,
        }
    }

    /// Shows the chosen source or run.
    fn render(&self) {
        let chosen = self.chosen.get();
        self.body.remove_css_class("hk-import-dim");
        self.undo.set_visible(false);
        self.import.set_visible(false);
        self.moved_notice.set_visible(false);
        self.problem.set_visible(false);
        match chosen {
            Chosen::Nothing => {
                self.title.set_text(&tr("Import"));
                self.header_summary.set_text("");
                self.options_box.set_visible(false);
                self.figures.root.set_visible(false);
                self.empty_text
                    .set_text(&tr("Choose a player or a file at the left."));
                self.body.set_visible_child_name("empty");
                self.summary.set_text("");
            }
            Chosen::Source(index) => self.render_source(index),
            Chosen::Run(id) => {
                let run = self.runs.borrow().iter().find(|r| r.id == id).cloned();
                if let Some(run) = run {
                    self.render_run(&run);
                }
            }
        }
    }

    fn render_source(&self, index: usize) {
        let name = self.source_name(index);
        let from = self.source_at(index);
        let file_name = from
            .as_ref()
            .and_then(|f| f.path().file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let data = index == self.data_index();
        self.title.set_text(if data { &file_name } else { &name });
        self.options_box.set_visible(true);
        self.import.set_visible(true);
        let shown = self.shown.borrow().clone();
        match shown {
            None | Some(Shown::Reading) => {
                self.header_summary.set_text(&file_name);
                self.figures.root.set_visible(true);
                self.figures.reading();
                self.working_text
                    .set_text(&fill(&tr("Reading {}…"), &[&file_name]));
                self.body.set_visible_child_name("working");
                self.import.set_label(&tr("Import"));
                self.import.set_sensitive(false);
                self.summary.set_text("");
            }
            Some(Shown::Problem(problem)) => {
                self.header_summary.set_text(&file_name);
                self.options_box.set_visible(false);
                self.figures.root.set_visible(false);
                self.problem.set_visible(true);
                let (message, line) = match &problem {
                    ImportProblem::Newer => (
                        tr("This file was saved by a newer Helikodu. Update Helikodu to read it."),
                        String::new(),
                    ),
                    ImportProblem::Unreadable(cause) => (
                        if data {
                            tr("This file cannot be read as a Helikodu data file.")
                        } else {
                            fill(&tr("This file cannot be read as a {} library."), &[&name])
                        },
                        cause.clone(),
                    ),
                    ImportProblem::Failed(cause) => {
                        (tr("The import could not be planned."), cause.clone())
                    }
                };
                self.problem_message.set_text(&message);
                self.problem_line.set_text(&line);
                self.problem_line.set_visible(!line.is_empty());
                self.problem_choose
                    .set_visible(!matches!(problem, ImportProblem::Failed(_)));
                self.empty_text.set_text("");
                self.body.set_visible_child_name("empty");
                self.import.set_visible(false);
                self.summary.set_text("");
            }
            Some(Shown::Plan(preview)) => self.render_plan(&preview, data),
        }
    }

    fn render_plan(&self, preview: &ImportPreview, data: bool) {
        let entries = preview.matching.entries;
        self.header_summary.set_text(&match &preview.saved {
            Some(saved) => {
                let saved_text = match (&saved.program, saved.at) {
                    (Some(program), Some(at)) => {
                        fill(&tr("Saved by {} on {}"), &[program, &date(Some(at))])
                    }
                    (None, Some(at)) => fill(&tr("Saved on {}"), &[&date(Some(at))]),
                    (Some(program), None) => fill(&tr("Saved by {}"), &[program]),
                    (None, None) => String::new(),
                };
                let tracks = fill(
                    &ntr("{} track", "{} tracks", saved.tracks),
                    &[&format::count(saved.tracks)],
                );
                if saved_text.is_empty() {
                    tracks
                } else {
                    fill(&tr("{} · {}"), &[&saved_text, &tracks])
                }
            }
            None => fill(
                &tr("{} · {}"),
                &[
                    &files_text(&preview.files),
                    &fill(
                        &ntr("{} entry", "{} entries", entries),
                        &[&format::count(entries)],
                    ),
                ],
            ),
        });
        let nothing_by_path = entries > 0
            && preview.matching.by_path == 0
            && self.options.borrow().mappings.is_empty();
        if nothing_by_path {
            self.moved_message.set_text(&fill(
                &ntr(
                    "The {} entry was not found by its path.",
                    "None of the {} entries were found by their path.",
                    entries,
                ),
                &[&format::count(entries)],
            ));
        }
        self.moved_notice.set_visible(nothing_by_path);
        self.figures.root.set_visible(true);
        self.figures
            .show(&preview.matching, preview.totals.unchanged, true);
        self.changes_head.set_text(&tr("What changes"));
        self.fill_changes(&preview.totals, data, Some(preview.notes.len()));
        let playlists_on = self.playlists_check.is_active();
        let in_file = preview.playlists.len() + usize::from(preview.totals.favourites > 0);
        self.playlists_note.set_text(&if playlists_on {
            fill(
                &ntr("{} in the file", "{} in the file", in_file),
                &[&format::count(in_file)],
            )
        } else {
            String::new()
        });
        self.fill_playlists(
            &preview.playlists,
            preview.earlier,
            false,
            data,
            preview.totals.favourites,
            playlists_on,
        );
        clear(&self.fields_list);
        for conflict in &preview.field_conflicts {
            self.fields_list.append(&field_box(conflict));
        }
        self.fields.set_visible(!preview.field_conflicts.is_empty());
        self.body.set_visible_child_name("columns");
        let tracks = preview.totals.tracks;
        self.import.set_label(&if tracks > 0 {
            fill(
                &ntr("Import for {} track", "Import for {} tracks", tracks),
                &[&format::count(tracks)],
            )
        } else {
            tr("Import")
        });
        self.import
            .set_sensitive(!preview.empty && !self.applying.get());
        self.summary.set_text(&if preview.empty {
            if data {
                tr("Nothing to import: everything in this file is in your library already.")
            } else {
                tr("Nothing to import: this file was imported before and has no new plays.")
            }
        } else if data {
            tr("Counts take the higher of the two values, so importing this file twice changes nothing.")
        } else {
            tr("Plays are added to yours. Importing this file again later adds only the plays made since.")
        });
    }

    /// Fills "What changes" with the rows of `totals`, and the notes of the
    /// reader when `notes` is given.
    fn fill_changes(
        &self,
        totals: &crate::backend::interchange::PlanTotals,
        data: bool,
        notes: Option<usize>,
    ) {
        clear(&self.changes);
        let rows = change_rows(totals, data);
        let none = rows.is_empty();
        for row in rows {
            self.changes.append(&row);
        }
        if none {
            let row = label(&tr("Nothing"), &["hk-t3"]);
            row.add_css_class("hk-import-change");
            self.changes.append(&row);
        }
        if let Some(notes) = notes.filter(|n| *n > 0) {
            let figure = counted(&ntr("{} note", "{} notes", notes), notes);
            if let Some(number) = figure.first_child().and_downcast::<gtk::Label>() {
                number.remove_css_class("hk-medium");
                number.add_css_class("hk-t3");
            }
            figure.set_spacing(0);
            let show = gtk::Button::with_label(&tr("Show"));
            show.add_css_class("hk-import-link");
            show.set_widget_name("notes-show");
            show.set_margin_start(10);
            let lines = self.preview().map(|p| p.notes).unwrap_or_default();
            show.connect_clicked(move |button| {
                list_popover(button, &tr("What the reader left out"), &lines);
            });
            figure.append(&show);
            let row = super::import_view::change_row(&tr("Left out by the reader"), &figure);
            if let Some(name) = row.first_child() {
                name.add_css_class("hk-t3");
            }
            self.changes.append(&row);
        }
    }

    fn fill_playlists(
        &self,
        playlists: &[PlannedPlaylist],
        earlier: Option<SystemTime>,
        past: bool,
        data: bool,
        favourites: usize,
        on: bool,
    ) {
        clear(&self.playlists);
        if !on {
            self.playlists
                .append(&label(&tr("Playlists are left out."), &["hk-t3"]));
            return;
        }
        for playlist in playlists {
            self.playlists
                .append(&playlist_row(playlist, earlier, past, data));
        }
        if favourites > 0 {
            self.playlists.append(&favourites_row(favourites));
        }
        if playlists.is_empty() && favourites == 0 {
            self.playlists.append(&label(
                &if past {
                    tr("It made no playlists.")
                } else {
                    tr("The file has no playlists.")
                },
                &["hk-t3"],
            ));
        }
    }

    fn render_run(&self, run: &ImportRunRow) {
        self.options_box.set_visible(false);
        self.undo.set_visible(!run.undone);
        let newest = self
            .runs
            .borrow()
            .iter()
            .filter(|r| !r.undone)
            .map(|r| r.id)
            .max();
        let can_undo = newest == Some(run.id) && !self.applying.get();
        self.undo.set_sensitive(can_undo);
        let carry = run.kind == ImportRunKind::Carry;
        self.title.set_text(&run_name(run));
        let files = run
            .summary
            .as_ref()
            .map(|s| files_text(&s.files))
            .unwrap_or_default();
        let made = if carry {
            fill(&tr("Carried {}"), &[&when(run.at)])
        } else {
            fill(&tr("Imported {}"), &[&when(run.at)])
        };
        self.header_summary.set_text(&if files.is_empty() {
            made
        } else {
            fill(&tr("{} · {}"), &[&made, &files])
        });
        if run.undone {
            self.body.add_css_class("hk-import-dim");
        }
        match (&run.summary, carry) {
            (Some(summary), false) => {
                self.figures.root.set_visible(true);
                self.figures
                    .show(&summary.matching, summary.totals.unchanged, false);
                self.changes_head.set_text(&tr("What changed"));
                let data = run.kind == ImportRunKind::UserData;
                self.fill_changes(&summary.totals, data, None);
                self.playlists_note.set_text(&tr("made beside yours"));
                self.fill_playlists(
                    &summary.playlists,
                    None,
                    true,
                    data,
                    summary.totals.favourites,
                    true,
                );
                self.fields.set_visible(false);
                self.body.set_visible_child_name("columns");
            }
            _ => {
                self.figures.root.set_visible(false);
                self.carried_text.set_text(&if carry {
                    fill(
                        &ntr(
                            "The plays, skips, history and places in playlists of {} missing track were moved to its new file.",
                            "The plays, skips, history and places in playlists of {} missing tracks were moved to their new files.",
                            run.tracks,
                        ),
                        &[&format::count(run.tracks)],
                    )
                } else {
                    fill(
                        &ntr(
                            "It changed {} track. No more was kept of what it did.",
                            "It changed {} tracks. No more was kept of what it did.",
                            run.tracks,
                        ),
                        &[&format::count(run.tracks)],
                    )
                });
                self.body.set_visible_child_name("carried");
            }
        }
        self.summary.set_text(&if run.undone {
            match run.undone_at {
                Some(at) => fill(&tr("Undone on {}"), &[&date(Some(at))]),
                None => tr("Undone"),
            }
        } else if !can_undo && newest != Some(run.id) {
            tr("Undo the newer imports first.")
        } else if carry {
            tr("Undo moves the plays, history and places in playlists back to the missing tracks.")
        } else {
            tr("Undo takes back what this import added. Plays made since stay, and its playlists are deleted.")
        });
    }

    /// "Import for N tracks": applies the plan shown, then shows the import
    /// under "Earlier imports" and the toast in the main window.
    fn apply(self: &Rc<Self>) {
        let Some(preview) = self.preview() else {
            return;
        };
        if preview.empty || self.applying.get() {
            return;
        }
        self.applying.set(true);
        self.import.set_sensitive(false);
        let services = self.services.clone();
        let weak = Rc::downgrade(self);
        let tracks = preview.totals.tracks;
        run_in_background(
            move |_| services.apply_import(preview.plan),
            |_, _| {},
            move |result| {
                let Some(this) = weak.upgrade() else { return };
                this.applying.set(false);
                match result {
                    Ok(run) => {
                        this.chosen.set(Chosen::Run(run));
                        this.shown.replace(None);
                        this.load_runs(|this| {
                            this.select_left();
                            this.render();
                        });
                        if let Some(main) = this.main.upgrade() {
                            main.playlists_changed(None);
                            toast_import(&main, this.services.clone(), run, tracks);
                        }
                    }
                    Err(error) => {
                        this.summary.set_text(&error.0);
                        this.render();
                    }
                }
            },
        );
    }

    /// "Undo this import".
    fn undo(self: &Rc<Self>) {
        let Chosen::Run(run) = self.chosen.get() else {
            return;
        };
        self.applying.set(true);
        self.undo.set_sensitive(false);
        let services = self.services.clone();
        let weak = Rc::downgrade(self);
        run_in_background(
            move |_| services.undo_import(run),
            |_, _| {},
            move |result| {
                let Some(this) = weak.upgrade() else { return };
                this.applying.set(false);
                if let Err(error) = &result
                    && let Some(main) = this.main.upgrade()
                {
                    main.toast(&error.0, true);
                }
                if let Some(main) = this.main.upgrade() {
                    main.playlists_changed(None);
                }
                this.load_runs(|this| this.render());
            },
        );
    }

    fn set_options(self: &Rc<Self>, change: impl FnOnce(&mut MergeOptions)) {
        change(&mut self.options.borrow_mut());
        self.fill_pairs();
        self.plan();
    }

    /// The chips of the folders that moved.
    fn fill_pairs(self: &Rc<Self>) {
        while let Some(child) = self.pairs.first_child() {
            self.pairs.remove(&child);
        }
        let mappings = self.options.borrow().mappings.clone();
        for (i, mapping) in mappings.iter().enumerate() {
            let weak = Rc::downgrade(self);
            self.pairs.append(&import_moved::chip(mapping, move || {
                if let Some(this) = weak.upgrade() {
                    this.set_options(|o| {
                        if i < o.mappings.len() {
                            o.mappings.remove(i);
                        }
                    });
                }
            }));
        }
        self.pairs.set_visible(!mappings.is_empty());
        self.pairs_hint.set_visible(mappings.is_empty());
    }

    fn add_pair(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        import_moved::popover(&self.add_pair, &self.window, move |mapping| {
            if let Some(this) = weak.upgrade() {
                this.set_options(|o| o.mappings.push(mapping));
            }
        });
    }
}

/// The toast after an import: its Undo does what "Undo this import" does, but
/// Ctrl+Z does not.
fn toast_import(
    main: &MainWindow,
    services: Arc<dyn InterchangeServices>,
    run: i64,
    tracks: usize,
) {
    let weak = main.downgrade();
    main.toast_led(
        &fill(
            &ntr(
                "Imported plays and ratings for {} track",
                "Imported plays and ratings for {} tracks",
                tracks,
            ),
            &[&format::count(tracks)],
        ),
        "helikodu-check-symbolic",
        &tr("Undo"),
        true,
        move || {
            undo_run(
                &weak,
                services.clone(),
                run,
                &tr("The import was undone."),
                || {},
            );
        },
    );
}

/// Undoes `run` from a toast, then refreshes the Import window if it is open.
pub(crate) fn undo_run(
    main: &glib::WeakRef<MainWindow>,
    services: Arc<dyn InterchangeServices>,
    run: i64,
    done: &str,
    then: impl FnOnce() + 'static,
) {
    let weak = main.clone();
    let done = done.to_owned();
    run_in_background(
        move |_| services.undo_import(run),
        |_, _| {},
        move |result| {
            let Some(main) = weak.upgrade() else { return };
            match result {
                Ok(()) => main.toast(&done, false),
                Err(error) => main.toast(&error.0, true),
            }
            main.playlists_changed(None);
            refresh();
            then();
        },
    );
}

thread_local! {
    /// The open Import window's state, so that toasts and Missing files can
    /// refresh it.
    static OPEN: RefCell<std::rc::Weak<Import>> = const { RefCell::new(std::rc::Weak::new()) };
}

/// Reads the earlier imports again in the open Import window, if any.
pub(crate) fn refresh() {
    if let Some(this) = OPEN.with(|o| o.borrow().upgrade()) {
        this.load_runs(|this| this.render());
    }
}

/// Opens the Import window over `main`, or brings it to the front, with
/// `start` chosen.
pub(crate) fn open(main: &MainWindow, start: Option<Start>) -> Option<gtk::Window> {
    let parent = main.upcast_ref::<gtk::Window>();
    if let Some(window) = find_open(parent, NAME) {
        window.present();
        if let (Some(start), Some(this)) = (start, OPEN.with(|o| o.borrow().upgrade())) {
            this.start(start);
        }
        return Some(window);
    }
    let backend = main.backend()?;
    let services = backend.services().interchange();
    let Frame {
        window,
        list: from_list,
        side_note,
        title,
        summary: header_summary,
        main: column,
    } = frame(parent, NAME, &tr("Import"), &tr("From"));
    side_note.set_text(&tr(
        "The other player’s files are only read, never changed.",
    ));

    // The left pane: the sources, then the earlier imports.
    from_list.set_vexpand(false);
    from_list.set_widget_name("import-from");
    let players = services.players();
    let mut from_rows = Vec::new();
    for source in &players {
        let (row, line) = source_row(&player_name(source.player));
        row.set_widget_name(&format!("import-from-{}", source.player.id()));
        from_list.append(&row);
        from_rows.push((row, line));
    }
    let (row, line) = source_row(&tr("A Helikodu data file"));
    row.set_widget_name("import-from-data");
    from_list.append(&row);
    from_rows.push((row, line));
    let runs_head = label(
        &tr("Earlier imports"),
        &["hk-section-label", "hk-list-section"],
    );
    let runs_list = gtk::ListBox::new();
    runs_list.add_css_class("hk-list");
    runs_list.set_widget_name("import-runs");
    runs_list.set_margin_start(8);
    runs_list.set_margin_end(8);
    runs_list.set_valign(gtk::Align::Start);
    runs_list.update_property(&[gtk::accessible::Property::Label(&tr("Earlier imports"))]);
    let runs_scroller = gtk::ScrolledWindow::new();
    runs_scroller.add_css_class("hk-import-runs-scroller");
    runs_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    runs_scroller.set_vexpand(true);
    runs_scroller.set_child(Some(&runs_list));
    if let Some(side) = from_list.parent().and_downcast::<gtk::Box>() {
        side.insert_child_after(&runs_head, Some(&from_list));
        side.insert_child_after(&runs_scroller, Some(&runs_head));
    }

    // The options.
    let options_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    options_box.add_css_class("hk-import-options");
    let checks = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let playlists_check = gtk::CheckButton::with_label(&tr("Import playlists"));
    playlists_check.set_widget_name("import-playlists");
    playlists_check.set_active(true);
    checks.append(&playlists_check);
    let ratings_check =
        gtk::CheckButton::with_label(&tr("Use its ratings also where you rated the track here"));
    ratings_check.set_widget_name("import-replace-ratings");
    checks.append(&ratings_check);
    options_box.append(&checks);
    let moved_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    moved_line.append(&label(
        &tr("Folders that moved"),
        &["hk-import-pairs-label"],
    ));
    let pairs = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    pairs.set_widget_name("import-pairs");
    pairs.set_visible(false);
    moved_line.append(&pairs);
    let add_pair = compact_button(&tr("Add…"));
    add_pair.set_widget_name("import-add-pair");
    moved_line.append(&add_pair);
    let pairs_hint = label(
        &tr("For a library whose music was in another folder, or on Windows or a Mac."),
        &["hk-t3", "hk-secondary"],
    );
    pairs_hint.set_hexpand(true);
    pairs_hint.set_margin_start(6);
    moved_line.append(&pairs_hint);
    options_box.append(&moved_line);
    column.append(&options_box);

    // The notice of an import that found nothing by path.
    let (moved_notice, moved_message, moved_line_text, room) = notice();
    moved_notice.add_css_class("hk-import-notice");
    moved_notice.set_widget_name("import-moved-notice");
    moved_line_text.set_text(&tr(
        "If the music was in another folder there, tell Helikodu where it is now. Until then entries are matched by their tags only.",
    ));
    let add_moved = gtk::Button::with_label(&tr("Add a folder that moved…"));
    add_moved.add_css_class("hk-import-notice-action");
    add_moved.set_widget_name("import-add-moved");
    room.append(&add_moved);
    moved_notice.set_visible(false);
    column.append(&moved_notice);

    // The notice of a file that cannot be read.
    let (problem, problem_message, problem_line, problem_room) = notice();
    problem.add_css_class("hk-import-notice");
    problem.set_widget_name("import-problem");
    problem.set_margin_top(16);
    let problem_choose = compact_button(&tr("Choose another file…"));
    problem_choose.set_widget_name("import-choose-another");
    problem_room.append(&problem_choose);
    problem.set_visible(false);
    column.append(&problem);

    let figures = Figures::new();
    column.append(&figures.root);

    // The two columns, the working state and the other bodies.
    let columns = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    columns.add_css_class("hk-import-columns");
    columns.set_homogeneous(true);
    let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
    left.add_css_class("hk-import-column");
    let changes_head = label(
        &tr("What changes"),
        &["hk-section-label", "hk-import-column-head"],
    );
    left.append(&changes_head);
    let changes = gtk::Box::new(gtk::Orientation::Vertical, 0);
    changes.set_widget_name("import-changes");
    left.append(&changes);
    let left_scroller = gtk::ScrolledWindow::new();
    left_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    left_scroller.set_child(Some(&left));
    columns.append(&left_scroller);
    let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right.add_css_class("hk-import-column");
    let playlists_headline = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    playlists_headline.add_css_class("hk-import-column-head");
    let playlists_head = label(&tr("Playlists"), &["hk-section-label"]);
    playlists_head.set_hexpand(true);
    playlists_headline.append(&playlists_head);
    let playlists_note = label("", &["hk-t3", "hk-secondary"]);
    playlists_headline.append(&playlists_note);
    right.append(&playlists_headline);
    let playlists = gtk::Box::new(gtk::Orientation::Vertical, 0);
    playlists.set_widget_name("import-playlists-list");
    right.append(&playlists);
    let fields = gtk::Box::new(gtk::Orientation::Vertical, 6);
    fields.add_css_class("hk-import-fields");
    fields.set_widget_name("import-fields");
    let fields_headline = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let fields_head = label(&tr("Fields that differ"), &["hk-section-label"]);
    fields_head.set_hexpand(true);
    fields_headline.append(&fields_head);
    fields_headline.append(&label(
        &tr("the definition here is kept"),
        &["hk-t3", "hk-secondary"],
    ));
    fields.append(&fields_headline);
    let fields_list = gtk::Box::new(gtk::Orientation::Vertical, 8);
    fields.append(&fields_list);
    fields.set_visible(false);
    right.append(&fields);
    let right_scroller = gtk::ScrolledWindow::new();
    right_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    right_scroller.set_child(Some(&right));
    let divider = gtk::Separator::new(gtk::Orientation::Vertical);
    let right_side = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    right_side.append(&divider);
    right_scroller.set_hexpand(true);
    right_side.append(&right_scroller);
    columns.append(&right_side);

    let working = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    working.add_css_class("hk-import-working");
    working.set_halign(gtk::Align::Center);
    working.set_valign(gtk::Align::Start);
    let spinner = gtk::Spinner::new();
    working.append(&spinner);
    let working_text = label("", &["hk-t3"]);
    working.append(&working_text);
    let empty_text = label("", &["hk-t3", "hk-tool-empty"]);
    empty_text.set_xalign(0.5);
    empty_text.set_valign(gtk::Align::Start);
    let carried_text = label("", &["hk-t3", "hk-tool-empty"]);
    carried_text.set_wrap(true);
    carried_text.set_single_line_mode(false);
    carried_text.set_ellipsize(gtk::pango::EllipsizeMode::None);
    carried_text.set_valign(gtk::Align::Start);
    let body = gtk::Stack::new();
    body.set_vexpand(true);
    body.set_widget_name("import-body");
    body.add_named(&columns, Some("columns"));
    body.add_named(&working, Some("working"));
    body.add_named(&empty_text, Some("empty"));
    body.add_named(&carried_text, Some("carried"));
    column.append(&body);
    // The spinner turns only while a library is read.
    {
        let spinner = spinner.clone();
        body.connect_visible_child_name_notify(move |body| {
            spinner.set_spinning(body.visible_child_name().as_deref() == Some("working"));
        });
    }

    let (bar, summary) = footer();
    summary.set_widget_name("import-footer");
    summary.set_wrap(true);
    summary.set_single_line_mode(false);
    summary.set_ellipsize(gtk::pango::EllipsizeMode::None);
    let undo = compact_button(&tr("Undo this import"));
    undo.set_widget_name("import-undo");
    undo.set_visible(false);
    bar.append(&undo);
    let import = primary_button(&tr("Import"));
    import.set_widget_name("import-apply");
    bar.append(&import);
    column.append(&bar);

    let this = Rc::new(Import {
        window: window.clone(),
        main: main.downgrade(),
        services: services.clone(),
        players,
        chosen_files: RefCell::new(HashMap::new()),
        runs: RefCell::new(Vec::new()),
        chosen: Cell::new(Chosen::Nothing),
        options: RefCell::new(MergeOptions::default()),
        shown: RefCell::new(None),
        asked: Cell::new(0),
        filling: Cell::new(false),
        applying: Cell::new(false),
        from_list: from_list.clone(),
        from_rows,
        runs_list: runs_list.clone(),
        title,
        header_summary,
        options_box,
        playlists_check: playlists_check.clone(),
        pairs,
        add_pair: add_pair.clone(),
        pairs_hint,
        moved_notice,
        moved_message,
        problem,
        problem_message,
        problem_line,
        problem_choose: problem_choose.clone(),
        figures,
        body,
        working_text,
        empty_text,
        changes_head,
        changes,
        playlists_note,
        playlists,
        fields,
        fields_list,
        carried_text,
        summary,
        import: import.clone(),
        undo: undo.clone(),
    });
    OPEN.with(|o| o.replace(Rc::downgrade(&this)));
    this.update_from_rows();

    let weak = Rc::downgrade(&this);
    from_list.connect_row_selected(move |_, row| {
        let (Some(this), Some(row)) = (weak.upgrade(), row) else {
            return;
        };
        if this.filling.get() {
            return;
        }
        let index = usize::try_from(row.index()).unwrap_or(0);
        if this.chosen.get() == Chosen::Source(index) {
            return;
        }
        this.choose_source(index, false);
    });
    // Enter or a double click on a source asks for another file.
    let weak = Rc::downgrade(&this);
    from_list.connect_row_activated(move |_, row| {
        let Some(this) = weak.upgrade() else { return };
        let index = usize::try_from(row.index()).unwrap_or(0);
        if this.chosen.get() == Chosen::Source(index)
            && this.players.get(index).is_none_or(|p| p.found.is_none())
        {
            this.choose_source(index, true);
        }
    });
    from_list.set_activate_on_single_click(false);
    let weak = Rc::downgrade(&this);
    runs_list.connect_row_selected(move |_, row| {
        let (Some(this), Some(row)) = (weak.upgrade(), row) else {
            return;
        };
        if this.filling.get() {
            return;
        }
        let index = usize::try_from(row.index()).unwrap_or(0);
        let Some(id) = this.runs.borrow().get(index).map(|r| r.id) else {
            return;
        };
        this.chosen.set(Chosen::Run(id));
        this.select_left();
        this.render();
    });
    let weak = Rc::downgrade(&this);
    playlists_check.connect_toggled(move |check| {
        if let Some(this) = weak.upgrade() {
            let skip = !check.is_active();
            this.set_options(|o| o.skip_playlists = skip);
        }
    });
    let weak = Rc::downgrade(&this);
    ratings_check.connect_toggled(move |check| {
        if let Some(this) = weak.upgrade() {
            let replace = check.is_active();
            this.set_options(|o| o.replace_ratings = replace);
        }
    });
    let weak = Rc::downgrade(&this);
    add_pair.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.add_pair();
        }
    });
    let weak = Rc::downgrade(&this);
    add_moved.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.add_pair();
        }
    });
    let weak = Rc::downgrade(&this);
    problem_choose.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade()
            && let Chosen::Source(index) = this.chosen.get()
        {
            this.choose_source(index, true);
        }
    });
    let weak = Rc::downgrade(&this);
    this.figures.unmatched.link.connect_clicked(move |button| {
        if let Some(preview) = weak.upgrade().and_then(|t| t.preview()) {
            list_popover(
                button,
                &listed_head(
                    preview.unmatched.len().min(LISTED),
                    preview.matching.unmatched,
                ),
                &preview.unmatched,
            );
        }
    });
    let weak = Rc::downgrade(&this);
    this.figures.ambiguous.link.connect_clicked(move |button| {
        if let Some(preview) = weak.upgrade().and_then(|t| t.preview()) {
            list_popover(
                button,
                &listed_head(
                    preview.ambiguous.len().min(LISTED),
                    preview.matching.ambiguous,
                ),
                &preview.ambiguous,
            );
        }
    });
    let weak = Rc::downgrade(&this);
    import.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.apply();
        }
    });
    let weak = Rc::downgrade(&this);
    undo.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.undo();
        }
    });

    let keep_alive = RefCell::new(Some(this.clone()));
    window.connect_destroy(move |_| {
        if let Some(this) = keep_alive.borrow_mut().take() {
            this.services.forget();
        }
    });
    this.render();
    let first = start.or_else(|| {
        this.players
            .iter()
            .find(|p| p.found.is_some())
            .map(|p| Start::Player(p.player))
    });
    this.load_runs(move |this| {
        if let Some(start) = first {
            this.start(start);
        }
    });
    window.present();
    Some(window)
}

impl Import {
    /// Chooses what `start` names.
    fn start(self: &Rc<Self>, start: Start) {
        let index = match start {
            Start::Player(player) => self
                .players
                .iter()
                .position(|p| p.player == player)
                .unwrap_or(0),
            Start::DataFile => self.data_index(),
        };
        self.choose_source(index, false);
    }
}

/// Asks where to save the user data, then saves it and says so in a toast with
/// "Show".
pub(crate) fn export_user_data(main: &MainWindow) {
    export_user_data_over(main, main.upcast_ref());
}

/// [`export_user_data`] with the save dialog over `parent`.
pub(crate) fn export_user_data_over(main: &MainWindow, parent: &gtk::Window) {
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(&tr("Helikodu data file")));
    filter.add_suffix("json");
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title(tr("Export your data"))
        .initial_name("helikodu-data.json")
        .filters(&filters)
        .modal(true)
        .build();
    let weak = main.downgrade();
    dialog.save(Some(parent), gio::Cancellable::NONE, move |file| {
        let (Some(main), Ok(Some(path))) = (weak.upgrade(), file.map(|f| f.path())) else {
            return;
        };
        export_user_data_to(&main, path);
    });
}

/// Saves the user data to `path` and says so in a toast with "Show".
pub(crate) fn export_user_data_to(main: &MainWindow, path: PathBuf) {
    let Some(backend) = main.backend() else {
        return;
    };
    let services = backend.services().interchange();
    let weak = main.downgrade();
    let target = path.clone();
    run_in_background(
        move |_| services.export_user_data(&target),
        |_, _| {},
        move |result| {
            let Some(main) = weak.upgrade() else { return };
            match result {
                Ok(()) => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let window = main.downgrade();
                    main.toast_led(
                        &fill(&tr("Your data was saved to {}"), &[&name]),
                        "helikodu-check-symbolic",
                        &tr("Show"),
                        false,
                        move || {
                            if let Some(main) = window.upgrade() {
                                crate::views::actions::open_folder(&main, Some(path.clone()));
                            }
                        },
                    );
                }
                Err(error) => main.toast(&error.0, true),
            }
        },
    );
}
