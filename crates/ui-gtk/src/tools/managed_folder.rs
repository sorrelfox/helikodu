// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The managed folder's window: the plan of adding music to the managed folder,
//! or of gathering the library's own files into it, and its run.
//!
//! At the left the places the files come from ("Add more…" adds folders) and
//! the managed folder with its pattern; at the right Copy or Move for this run,
//! a filter, five figures (to copy, get a number, already there, left where
//! they are, the space), a notice when the files do not fit, and every file
//! with what happens to it. The plan is worked out on a thread of its own, again
//! when the sources or Copy / Move change, and when the window gets the focus
//! back while the files do not fit. Nothing changes before the primary button.
//!
//! The run works in the background (`managed_runs`): the rows show what is
//! placed, the footer the progress and "Stop", which cuts the run short.
//! Closing the window does not stop it. Afterwards the figures say what was
//! done and "Show in the library" opens Tracks at the folders the music went
//! to; a finished run has no undo.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::file_tools::{WorkBar, relative};
use super::managed_runs::{self, Work};
use super::tool_window::{GAP, footer, frame, primary_button, table_header};
use crate::backend::services::{
    CancelToken, FolderState, ItemAction, ItemKind, ItemResult, LibraryFolder, ManagedItem,
    ManagedOperation, ManagedPlan, ManagedReport, ManagedSource, Services, SkipReason, Transfer,
};
use crate::dialogs::common::{
    bind_segmented, compact_button, dropdown, find_open, on_selected, segmented, tilde,
};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "managed-folder";

/// The least width of the "What happens" column; it grows with its longest
/// text.
const WHAT: i32 = 190;
/// The width of the Size column.
const SIZE: i32 = 60;
/// The width of the kind's icon.
const ICON: i32 = 16;

/// What the window plans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Music from outside the library.
    Add,
    /// The library's own files outside the managed folder.
    Gather,
}

/// Which rows the table shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    All,
    Music,
    Left,
}

/// Where a row stands in a run.
#[derive(Debug, Clone, PartialEq)]
enum RowState {
    /// The plan, before the run.
    Planned,
    /// Waiting for its turn.
    Waiting,
    /// Being placed.
    Working,
    /// Ended, as the report says.
    Ended(ItemResult),
}

thread_local! {
    static OPEN: RefCell<Weak<ManagedWindow>> = const { RefCell::new(Weak::new()) };
}

struct ManagedWindow {
    window: gtk::Window,
    main: glib::WeakRef<MainWindow>,
    services: Arc<dyn Services>,
    mode: Cell<Mode>,
    sources: RefCell<Vec<PathBuf>>,
    roots: RefCell<Vec<LibraryFolder>>,
    transfer: Cell<Transfer>,
    plan: RefCell<Option<Arc<ManagedPlan>>>,
    planning: RefCell<Option<CancelToken>>,
    generation: Cell<u64>,
    problem: RefCell<Option<String>>,
    /// The plan items the table shows, by index.
    shown: RefCell<Vec<usize>>,
    /// For each plan item, its turn among the files that move.
    turns: RefCell<Vec<Option<usize>>>,
    /// The plan items in the order the run takes them.
    in_turn: RefCell<Vec<usize>>,
    /// The results of the run shown, by source path.
    results: RefCell<HashMap<PathBuf, ItemResult>>,
    /// Whether the window shows the run of `managed_runs`.
    showing_run: Cell<bool>,
    last_done: Cell<usize>,
    /// The library folders the gathered files come from.
    gather_folders: Cell<usize>,
    filling: Cell<bool>,
    list: gtk::ListBox,
    into_folder: gtk::Label,
    into_pattern: gtk::Label,
    side_note: gtk::Label,
    title: gtk::Label,
    header_summary: gtk::Label,
    options: gtk::Box,
    modes: Vec<gtk::ToggleButton>,
    copy_note: gtk::Label,
    filter: gtk::SearchEntry,
    which: gtk::DropDown,
    figures: Vec<(gtk::Label, gtk::Label)>,
    space_icon: gtk::Image,
    notice: gtk::Box,
    notice_title: gtk::Label,
    notice_text: gtk::Label,
    store: gio::ListStore,
    stack: gtk::Stack,
    empty: gtk::Label,
    footer: gtk::Box,
    summary: gtk::Label,
    details: gtk::Button,
    show: gtk::Button,
    primary: gtk::Button,
    work: WorkBar,
}

fn kind_icon(kind: ItemKind) -> (&'static str, String) {
    match kind {
        ItemKind::Audio => ("audio-x-generic-symbolic", tr("Music")),
        ItemKind::Cover => ("image-x-generic-symbolic", tr("Cover")),
        ItemKind::Booklet => ("x-office-document-symbolic", tr("Booklet")),
        ItemKind::Log => ("text-x-generic-symbolic", tr("Rip log")),
        ItemKind::Lyrics => ("format-justify-left-symbolic", tr("Lyrics")),
        ItemKind::CueSheet => ("view-list-symbolic", tr("CUE sheet")),
    }
}

/// What happens to a file of the plan.
fn planned_text(item: &ManagedItem) -> String {
    match &item.action {
        ItemAction::Skip(_) => tr("Left where it is"),
        _ if item.kind == ItemKind::CueSheet && item.rewritten => tr("CUE sheet updated"),
        ItemAction::Move if item.kind == ItemKind::Lyrics => tr("Move, named like its track"),
        _ if item.kind == ItemKind::Lyrics => tr("Copy, named like its track"),
        _ if item.renamed => tr("Gets a number"),
        ItemAction::CopyKeepingSource => tr("Copy, the original stays"),
        ItemAction::Move => tr("Move"),
        _ => tr("Copy"),
    }
}

/// Why a file is left where it is, in a sentence.
fn reason_text(reason: &SkipReason) -> String {
    match reason {
        SkipReason::Identical => tr("An identical file is already there"),
        SkipReason::InLibrary => tr("It is in the library already: gather it instead"),
        SkipReason::InManagedFolder => tr("It is in the managed folder already"),
        SkipReason::Unreadable(why) => fill(&tr("It cannot be read: {}"), &[why]),
        SkipReason::Missing => tr("The file is missing"),
        SkipReason::Gone => tr("The file is not where it was"),
        SkipReason::Changed => tr("The library has the file elsewhere now"),
        SkipReason::Failed(why) => why.clone(),
        _ => tr("Left where it is"),
    }
}

/// What became of a file in the run: the words, whether they carry a check
/// mark, and whether a warning.
fn result_text(result: &ItemResult, transfer: Transfer) -> (String, bool, bool) {
    match result {
        ItemResult::Copied => (tr("Copied"), true, false),
        ItemResult::Moved => (tr("Moved"), true, false),
        ItemResult::SourceKept(_) => (tr("Copied, the original stays"), true, false),
        ItemResult::Skipped(SkipReason::Failed(why)) => (
            match transfer {
                Transfer::Move => fill(&tr("Could not be moved: {}"), &[why]),
                Transfer::Copy => fill(&tr("Could not be copied: {}"), &[why]),
            },
            false,
            true,
        ),
        ItemResult::Skipped(_) => (tr("Left where it is"), false, false),
        ItemResult::Waiting => (tr("Waiting"), false, false),
        ItemResult::RolledBack => (tr("Rolled back"), false, false),
        ItemResult::NotRolledBack(why) => (why.clone(), false, true),
        _ => (String::new(), false, false),
    }
}

/// The width of the "What happens" column: 190 px, or its longest text.
fn what_width() -> i32 {
    thread_local! {
        static WIDTH: std::cell::OnceCell<i32> = const { std::cell::OnceCell::new() };
    }
    WIDTH.with(|width| {
        *width.get_or_init(|| {
            [
                tr("What happens"),
                tr("Copy, the original stays"),
                tr("Copy, named like its track"),
                tr("Move, named like its track"),
                tr("CUE sheet updated"),
                tr("Left where it is"),
                tr("Gets a number"),
                tr("Copied, the original stays"),
            ]
            .iter()
            .map(|text| {
                let cell = label(text, &[]);
                cell.set_ellipsize(gtk::pango::EllipsizeMode::None);
                // The check mark before a result.
                cell.measure(gtk::Orientation::Horizontal, -1).1 + 22
            })
            .fold(WHAT, i32::max)
        })
    })
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

impl ManagedWindow {
    fn source(&self) -> ManagedSource {
        match self.mode.get() {
            Mode::Add => ManagedSource::Add(self.sources.borrow().clone()),
            Mode::Gather => ManagedSource::Gather,
        }
    }

    fn main_window(&self) -> Option<MainWindow> {
        self.main.upgrade()
    }

    /// The run of `managed_runs` this window shows, if it shows one.
    fn run(&self) -> Option<managed_runs::RunView> {
        self.showing_run
            .get()
            .then(managed_runs::current)
            .flatten()
            .filter(|r| r.work == Work::Run || r.work == Work::Resume)
    }

    /// Works the plan out again.
    fn replan(self: &Rc<Self>) {
        if self.run().is_some_and(|r| r.is_working()) {
            return;
        }
        self.showing_run.set(false);
        self.results.borrow_mut().clear();
        if let Some(cancel) = self.planning.take() {
            cancel.cancel();
        }
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.plan.replace(None);
        self.problem.replace(None);
        self.update_all();
        let cancel = CancelToken::new();
        self.planning.replace(Some(cancel.clone()));
        let services = self.services.clone();
        let source = self.source();
        let transfer = self.transfer.get();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (sender, receiver) = async_channel::bounded(1);
            let spawned = std::thread::Builder::new()
                .name("helikodu-managed-plan".into())
                .spawn(move || {
                    let roots = services.library_folders().unwrap_or_default();
                    let plan = services.plan_managed(&source, transfer, &cancel);
                    let _ = sender.send_blocking((roots, plan));
                });
            if spawned.is_err() {
                return;
            }
            let Ok((roots, plan)) = receiver.recv().await else {
                return;
            };
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            this.planning.replace(None);
            this.roots.replace(roots);
            match plan {
                Ok(plan) => {
                    this.plan.replace(Some(Arc::new(plan)));
                }
                Err(error) => {
                    this.problem.replace(Some(error.0));
                }
            }
            this.update_all();
        });
    }

    /// Shows the run that works in `managed_runs`, started here or elsewhere.
    fn adopt_run(self: &Rc<Self>) -> bool {
        let Some(run) = managed_runs::current().filter(managed_runs::RunView::is_working) else {
            return false;
        };
        let Some(plan) = run.plan.clone() else {
            return false;
        };
        self.mode.set(match plan.operation {
            ManagedOperation::Consolidate => Mode::Gather,
            _ => Mode::Add,
        });
        self.transfer.set(plan.options.transfer);
        if self.mode.get() == Mode::Add && self.sources.borrow().is_empty() {
            // The places the files come from, as near as the plan tells.
            let mut sources: Vec<PathBuf> = plan
                .items
                .iter()
                .filter_map(|i| i.from.parent().map(Path::to_path_buf))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            sources.dedup();
            self.sources.replace(sources);
        }
        self.plan.replace(Some(plan));
        self.showing_run.set(true);
        self.update_all();
        true
    }

    fn rebuild_turns(&self) {
        let plan = self.plan.borrow();
        let mut turns = Vec::new();
        let mut in_turn = Vec::new();
        if let Some(plan) = plan.as_ref() {
            for (index, item) in plan.items.iter().enumerate() {
                if item.transfers() {
                    turns.push(Some(in_turn.len()));
                    in_turn.push(index);
                } else {
                    turns.push(None);
                }
            }
        }
        self.turns.replace(turns);
        self.in_turn.replace(in_turn);
    }

    /// Everything from the plan and the run.
    fn update_all(self: &Rc<Self>) {
        self.rebuild_turns();
        self.fill_side();
        self.fill_header();
        self.fill_options();
        self.fill_figures();
        self.filter_rows();
        self.fill_footer();
    }

    fn fill_side(&self) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let plan = self.plan.borrow();
        let items: &[ManagedItem] = plan.as_ref().map_or(&[], |p| p.items.as_slice());
        let files_line = |paths: &[&Path]| -> String {
            let folders: BTreeSet<&Path> = paths.iter().filter_map(|p| p.parent()).collect();
            if plan.is_none() {
                return String::new();
            }
            let files = fill(
                &ntr("{} file", "{} files", paths.len()),
                &[&format::count(paths.len())],
            );
            if folders.len() > 1 {
                fill(
                    &ntr("{} in {} folder", "{} in {} folders", folders.len()),
                    &[&files, &format::count(folders.len())],
                )
            } else {
                files
            }
        };
        match self.mode.get() {
            Mode::Add => {
                for source in self.sources.borrow().iter() {
                    let from: Vec<&Path> = items
                        .iter()
                        .map(|i| i.from.as_path())
                        .filter(|p| p.starts_with(source))
                        .collect();
                    self.list
                        .append(&side_row(&tilde(source), &files_line(&from), false));
                }
                let row = gtk::ListBoxRow::new();
                row.set_widget_name("add-more");
                let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                let plus = gtk::Image::from_icon_name("list-add-symbolic");
                plus.add_css_class("hk-t3");
                content.append(&plus);
                content.append(&label(&tr("Add more…"), &["hk-t2"]));
                row.set_child(Some(&content));
                row.update_property(&[gtk::accessible::Property::Label(&tr("Add more…"))]);
                row.set_sensitive(self.run().is_none_or(|r| !r.is_working()));
                self.list.append(&row);
            }
            Mode::Gather => {
                let roots = self.roots.borrow();
                let mut by_root: Vec<(PathBuf, Vec<&Path>)> = Vec::new();
                for item in items {
                    let root = roots
                        .iter()
                        .map(|r| r.path.as_path())
                        .filter(|r| item.from.starts_with(r))
                        .max_by_key(|r| r.as_os_str().len())
                        .map_or_else(
                            || item.from.parent().unwrap_or(&item.from).to_path_buf(),
                            Path::to_path_buf,
                        );
                    match by_root.iter_mut().find(|(r, _)| *r == root) {
                        Some((_, paths)) => paths.push(&item.from),
                        None => by_root.push((root, vec![&item.from])),
                    }
                }
                self.gather_folders.set(by_root.len());
                for (root, paths) in &by_root {
                    self.list
                        .append(&side_row(&tilde(root), &files_line(paths), false));
                }
                for root in roots
                    .iter()
                    .filter(|r| r.state == FolderState::NotConnected)
                {
                    let tracks = root.tracks.unwrap_or(0);
                    if tracks == 0 {
                        continue;
                    }
                    let line = fill(
                        &ntr(
                            "Not connected · its {} file stays",
                            "Not connected · its {} files stay",
                            tracks,
                        ),
                        &[&format::count(tracks)],
                    );
                    self.list.append(&side_row(&tilde(&root.path), &line, true));
                }
            }
        }
        let settings = self.services.settings();
        let managed = &settings.library.managed;
        self.into_folder
            .set_text(&managed.folder.as_deref().map(tilde).unwrap_or_default());
        self.into_pattern.set_text(&managed.pattern);
    }

    fn fill_header(&self) {
        let plan = self.plan.borrow();
        let n = plan.as_ref().map_or(0, |p| p.to_transfer());
        let count = format::count(n);
        let title = match (self.mode.get(), plan.is_some()) {
            (Mode::Add, true) => fill(
                &ntr(
                    "Add {} file to the managed folder",
                    "Add {} files to the managed folder",
                    n,
                ),
                &[&count],
            ),
            (Mode::Gather, true) => fill(
                &ntr(
                    "Gather {} file into the managed folder",
                    "Gather {} files into the managed folder",
                    n,
                ),
                &[&count],
            ),
            (Mode::Add, false) => tr("Add music to the managed folder"),
            (Mode::Gather, false) => tr("Gather the library into the managed folder"),
        };
        self.title.set_text(&title);
        let from = match self.mode.get() {
            Mode::Add => {
                let places = self.sources.borrow().len();
                fill(
                    &ntr("From {} place", "From {} places", places),
                    &[&format::count(places)],
                )
            }
            Mode::Gather => {
                let folders = self.gather_folders.get();
                fill(
                    &ntr("From {} library folder", "From {} library folders", folders),
                    &[&format::count(folders)],
                )
            }
        };
        let how = match self.transfer.get() {
            Transfer::Copy => tr("copied"),
            Transfer::Move => tr("moved"),
        };
        self.header_summary
            .set_text(&format!("{from} · {how} · {}", tr("named by the pattern")));
    }

    fn fill_options(&self) {
        let working = self.run().is_some_and(|r| r.is_working());
        self.options.set_sensitive(!working);
        self.filling.set(true);
        let index = usize::from(self.transfer.get() == Transfer::Move);
        if let Some(button) = self.modes.get(index) {
            button.set_active(true);
        }
        self.filling.set(false);
        let gather = self.mode.get() == Mode::Gather;
        self.filter.set_visible(!gather);
        self.copy_note
            .set_visible(gather && self.transfer.get() == Transfer::Copy);
    }

    fn fill_figures(&self) {
        let plan = self.plan.borrow();
        let set = |i: usize, value: &str, caption: &str| {
            if let Some((v, c)) = self.figures.get(i) {
                v.set_text(value);
                c.set_text(caption);
            }
        };
        let moving = self.transfer.get() == Transfer::Move;
        let Some(plan) = plan.as_ref() else {
            let first = if moving { tr("to move") } else { tr("to copy") };
            for (i, caption) in [
                first,
                tr("get a number"),
                tr("already there"),
                tr("left where they are"),
                tr("needed"),
            ]
            .iter()
            .enumerate()
            {
                set(i, "…", caption);
            }
            self.space_icon.set_visible(false);
            self.notice.set_visible(false);
            return;
        };
        let identical = plan
            .items
            .iter()
            .filter(|i| i.action == ItemAction::Skip(SkipReason::Identical))
            .count();
        let left = plan.skipped() - identical;
        let finished = self.run().and_then(|r| r.result).and_then(Result::ok);
        match finished {
            Some(report) => set(
                0,
                &format::count(report.transferred()),
                &if moving { tr("moved") } else { tr("copied") },
            ),
            None => set(
                0,
                &format::count(plan.to_transfer()),
                &if moving { tr("to move") } else { tr("to copy") },
            ),
        }
        set(1, &format::count(plan.renamed()), &tr("get a number"));
        set(2, &format::count(identical), &tr("already there"));
        set(3, &format::count(left), &tr("left where they are"));
        let needed = format::size(plan.bytes_needed);
        let caption = match plan.free_bytes {
            Some(free) => fill(&tr("needed · {} free"), &[&format::size(free)]),
            None => tr("needed"),
        };
        set(4, &needed, &caption);
        let fits = plan.fits();
        self.space_icon.set_visible(!fits);
        self.notice.set_visible(!fits && !self.showing_run.get());
        if !fits {
            self.notice_title.set_text(&fill(
                &tr("These files need {}. The drive of the managed folder has {} free."),
                &[&needed, &format::size(plan.free_bytes.unwrap_or(0))],
            ));
            self.notice_text.set_text(&tr(
                "Choose fewer files, or free some space. The run cannot start until the files fit.",
            ));
        }
    }

    /// The rows that the filter and the dropdown let through.
    fn filter_rows(&self) {
        let plan = self.plan.borrow();
        let text = self.filter.text().to_lowercase();
        let which = match self.which.selected() {
            1 => Shown::Music,
            2 => Shown::Left,
            _ => Shown::All,
        };
        let shown: Vec<usize> = plan.as_ref().map_or_else(Vec::new, |plan| {
            plan.items
                .iter()
                .enumerate()
                .filter(|(_, item)| match which {
                    Shown::All => true,
                    Shown::Music => item.kind == ItemKind::Audio,
                    Shown::Left => !item.transfers(),
                })
                .filter(|(_, item)| {
                    text.is_empty()
                        || item.from.to_string_lossy().to_lowercase().contains(&text)
                        || item.to.to_string_lossy().to_lowercase().contains(&text)
                })
                .map(|(i, _)| i)
                .collect()
        });
        let count = shown.len();
        self.shown.replace(shown);
        let items: Vec<glib::BoxedAnyObject> = (0..count).map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &items);
        let empty = if let Some(problem) = self.problem.borrow().as_ref() {
            problem.clone()
        } else if plan.is_none() {
            tr("Looking at the files…")
        } else if plan.as_ref().is_some_and(|p| p.items.is_empty()) {
            match self.mode.get() {
                Mode::Add => tr("There is no music in the chosen places."),
                Mode::Gather => tr("Every library file is in the managed folder."),
            }
        } else {
            tr("No file matches the filter.")
        };
        self.empty.set_text(&empty);
        self.stack
            .set_visible_child_name(if count == 0 { "empty" } else { "table" });
    }

    fn fill_footer(&self) {
        let run = self.run();
        let working = run.as_ref().is_some_and(managed_runs::RunView::is_working);
        self.footer.set_visible(!working);
        self.work.root.set_visible(working);
        if let Some(run) = run.as_ref().filter(|_| working) {
            self.show_progress(run);
            return;
        }
        self.details.set_visible(false);
        self.show.set_visible(false);
        self.primary.set_visible(true);
        let plan = self.plan.borrow();
        let moving = self.transfer.get() == Transfer::Move;
        let n = plan.as_ref().map_or(0, |p| p.to_transfer());
        self.primary.set_label(&fill(
            &if moving {
                ntr("Move {} file", "Move {} files", n)
            } else {
                ntr("Copy {} file", "Copy {} files", n)
            },
            &[&format::count(n)],
        ));
        if let Some(Some(Ok(report))) = run.as_ref().map(|r| r.result.clone()) {
            // After the run.
            self.primary.set_visible(false);
            let failed = report.problems().count();
            let mut text = tr("Done.");
            if failed > 0 {
                let line = if moving {
                    ntr(
                        "{} file could not be moved",
                        "{} files could not be moved",
                        failed,
                    )
                } else {
                    ntr(
                        "{} file could not be copied",
                        "{} files could not be copied",
                        failed,
                    )
                };
                text = format!("{text} {}", fill(&line, &[&format::count(failed)]));
            }
            if report.finished {
                self.summary.set_text(&text);
                self.show
                    .set_visible(managed_runs::tracks_query(&report).is_some());
                return;
            }
        }
        if managed_runs::interrupted().is_some() {
            self.summary.set_text(&tr(
                "A run was cut short. Nothing can be added until it is settled.",
            ));
            self.details.set_visible(true);
            self.primary.set_sensitive(false);
            return;
        }
        let Some(plan) = plan.as_ref() else {
            self.summary.set_text(&tr(
                "Nothing is replaced: a name that is taken gets a number.",
            ));
            self.primary.set_sensitive(false);
            return;
        };
        if n == 0 {
            self.summary.set_text(&match self.mode.get() {
                Mode::Add => tr("Nothing to add: every file is already there or cannot be read."),
                Mode::Gather => {
                    tr("Nothing to gather: every file is already there or cannot be read.")
                }
            });
            self.primary.set_sensitive(false);
            return;
        }
        self.summary.set_text(&tr(
            "Nothing is replaced: a name that is taken gets a number.",
        ));
        self.primary
            .set_sensitive(plan.fits() && !managed_runs::is_working());
    }

    /// The footer at work: "412 of 1,098 · 14.2 GB written".
    fn show_progress(&self, run: &managed_runs::RunView) {
        self.work.meter.set_fraction(run.fraction());
        let mut text = fill(
            &tr("{} of {}"),
            &[&format::count(run.done), &format::count(run.total)],
        );
        if run.plan.is_some() {
            text = fill(&tr("{} · {} written"), &[&text, &format::size(run.bytes)]);
        }
        self.work.text.set_text(&text);
        self.work.stop.set_sensitive(true);
    }

    /// The run made progress or ended.
    fn run_changed(self: &Rc<Self>) {
        let Some(run) = self.run() else {
            // A run cut short may have been settled: the footer says so.
            self.fill_footer();
            return;
        };
        if let Some(Ok(report)) = run.result.as_ref() {
            let results: HashMap<PathBuf, ItemResult> = report
                .outcomes
                .iter()
                .map(|o| (o.from.clone(), o.result.clone()))
                .collect();
            self.results.replace(results);
            self.update_all();
            return;
        }
        if run.result.is_some() {
            self.update_all();
            return;
        }
        // Rows from the last one shown to the one in progress.
        let from = self.last_done.get().min(run.done);
        self.last_done.set(run.done);
        let in_turn = self.in_turn.borrow().clone();
        let shown = self.shown.borrow().clone();
        for turn in from.saturating_sub(1)..=run.done {
            if let Some(index) = in_turn.get(turn)
                && let Some(position) = shown.iter().position(|i| i == index)
            {
                // A new item, so that the view binds the row again.
                let position_u32 = u32::try_from(position).unwrap_or(0);
                self.store
                    .splice(position_u32, 1, &[glib::BoxedAnyObject::new(position)]);
            }
        }
        self.fill_footer();
    }

    fn row_state(&self, index: usize) -> RowState {
        let Some(run) = self.run() else {
            return RowState::Planned;
        };
        let plan = self.plan.borrow();
        let Some(item) = plan.as_ref().and_then(|p| p.items.get(index)) else {
            return RowState::Planned;
        };
        if run.result.is_some() {
            return match self.results.borrow().get(&item.from) {
                Some(result) => RowState::Ended(result.clone()),
                None => RowState::Planned,
            };
        }
        match self.turns.borrow().get(index).copied().flatten() {
            Some(turn) if turn < run.done => RowState::Ended(match run.transfer {
                Transfer::Move => ItemResult::Moved,
                Transfer::Copy => ItemResult::Copied,
            }),
            Some(turn) if turn == run.done => RowState::Working,
            Some(_) => RowState::Waiting,
            None => RowState::Planned,
        }
    }

    fn bind(&self, row: &gtk::Box, item: &gtk::ListItem, position: usize) {
        let Some(index) = self.shown.borrow().get(position).copied() else {
            return;
        };
        let plan = self.plan.borrow();
        let Some(planned) = plan.as_ref().and_then(|p| p.items.get(index)).cloned() else {
            return;
        };
        let state = self.row_state(index);
        let cells = children(row);
        let [icon, paths, what, size] = cells.as_slice() else {
            return;
        };
        let (icon_name, kind_name) = kind_icon(planned.kind);
        if let Some(image) = icon.downcast_ref::<gtk::Image>() {
            image.set_icon_name(Some(icon_name));
            image.update_property(&[gtk::accessible::Property::Label(&kind_name)]);
            image.set_tooltip_text(Some(&kind_name));
        }
        let sources = self.sources.borrow();
        let roots: Vec<PathBuf> = match self.mode.get() {
            Mode::Add => sources.clone(),
            Mode::Gather => self
                .roots
                .borrow()
                .iter()
                .map(|r| r.path.parent().unwrap_or(&r.path).to_path_buf())
                .collect(),
        };
        let now = relative(&planned.from, &roots);
        let folder = plan
            .as_ref()
            .map(|p| p.options.folder.clone())
            .unwrap_or_default();
        let (second, mono) = match &planned.action {
            ItemAction::Skip(reason) => (reason_text(reason), false),
            _ => (
                planned
                    .to
                    .strip_prefix(&folder)
                    .map_or_else(|_| tilde(&planned.to), |p| p.to_string_lossy().into_owned()),
                true,
            ),
        };
        let lines = children(paths);
        if let [top, bottom] = lines.as_slice()
            && let (Some(top), Some(bottom)) = (
                top.downcast_ref::<gtk::Label>(),
                bottom.downcast_ref::<gtk::Label>(),
            )
        {
            top.set_text(&now);
            top.set_tooltip_text(Some(&now));
            bottom.set_text(&second);
            bottom.set_tooltip_text(Some(&second));
            for class in [
                "hk-mono",
                "hk-tool-path",
                "hk-medium",
                "hk-t3",
                "hk-managed-reason",
            ] {
                bottom.remove_css_class(class);
            }
            if mono {
                bottom.add_css_class("hk-mono");
                bottom.add_css_class("hk-tool-path");
                bottom.add_css_class("hk-medium");
                bottom.set_ellipsize(gtk::pango::EllipsizeMode::Start);
            } else {
                bottom.add_css_class("hk-t3");
                bottom.add_css_class("hk-managed-reason");
                bottom.set_ellipsize(gtk::pango::EllipsizeMode::End);
            }
        }
        let parts = children(what);
        let [mark, warning, meter, words] = parts.as_slice() else {
            return;
        };
        let (text, check, warn, working) = match &state {
            RowState::Planned => (planned_text(&planned), false, false, false),
            RowState::Waiting => (tr("Waiting"), false, false, false),
            RowState::Working => (
                match self.transfer.get() {
                    Transfer::Move => tr("Moving…"),
                    Transfer::Copy => tr("Copying…"),
                },
                false,
                false,
                true,
            ),
            RowState::Ended(result) => {
                let (text, check, warn) = result_text(result, self.transfer.get());
                if text.is_empty() {
                    (planned_text(&planned), false, false, false)
                } else {
                    (text, check, warn, false)
                }
            }
        };
        mark.set_visible(check);
        warning.set_visible(warn);
        meter.set_visible(working);
        if let Some(words) = words.downcast_ref::<gtk::Label>() {
            words.set_text(&text);
            words.set_tooltip_text(Some(&text));
            for class in ["hk-t1", "hk-t2", "hk-t3", "hk-strong"] {
                words.remove_css_class(class);
            }
            let left = !planned.transfers() || matches!(state, RowState::Waiting);
            words.add_css_class(if warn {
                "hk-t1"
            } else if left {
                "hk-t3"
            } else {
                "hk-t2"
            });
        }
        if let Some(size) = size.downcast_ref::<gtk::Label>() {
            size.set_text(&format::size(planned.size));
        }
        item.set_accessible_label(&fill(&tr("{}, {}, {}"), &[&now, &second, &text]));
    }

    /// The primary button: the run starts.
    fn start(self: &Rc<Self>) {
        let Some(plan) = self.plan.borrow().clone() else {
            return;
        };
        let Some(main) = self.main_window() else {
            return;
        };
        if !plan.fits() || plan.to_transfer() == 0 {
            return;
        }
        self.last_done.set(0);
        self.results.borrow_mut().clear();
        if managed_runs::start(&main, plan) {
            self.showing_run.set(true);
            self.update_all();
        }
    }

    /// Adds `paths` to the places the files come from.
    fn add_sources(self: &Rc<Self>, paths: Vec<PathBuf>) {
        {
            let mut sources = self.sources.borrow_mut();
            for path in paths {
                if !sources.contains(&path) {
                    sources.push(path);
                }
            }
        }
        self.replan();
    }

    /// Removes the place of the left pane's row `row`.
    fn remove_source(self: &Rc<Self>, row: &gtk::ListBoxRow) {
        if self.mode.get() != Mode::Add || self.run().is_some_and(|r| r.is_working()) {
            return;
        }
        let Ok(index) = usize::try_from(row.index()) else {
            return;
        };
        let removed = {
            let mut sources = self.sources.borrow_mut();
            (index < sources.len()).then(|| sources.remove(index))
        };
        if removed.is_some() {
            self.replan();
        }
    }

    fn ask_for_more(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title(tr("Add music to the managed folder"))
            .accept_label(tr("Add"))
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.select_multiple_folders(Some(&self.window), gio::Cancellable::NONE, move |files| {
            let Ok(files) = files else { return };
            let paths: Vec<PathBuf> = (0..files.n_items())
                .filter_map(|i| files.item(i).and_downcast::<gio::File>())
                .filter_map(|f| f.path())
                .collect();
            if let Some(this) = weak.upgrade()
                && !paths.is_empty()
            {
                this.add_sources(paths);
            }
        });
    }
}

/// A row of the left pane: the path in mono over a line in 11 px `t3`.
fn side_row(path: &str, line: &str, dim: bool) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("two-line");
    if dim {
        row.add_css_class("hk-managed-dim");
    }
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_valign(gtk::Align::Center);
    let name = label(path, &["hk-side-name", "hk-mono", "hk-managed-path"]);
    name.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    name.set_tooltip_text(Some(path));
    text.append(&name);
    text.append(&label(line, &["hk-t3", "hk-side-line"]));
    row.set_child(Some(&text));
    row.update_property(&[gtk::accessible::Property::Label(path)]);
    row
}

/// A row of the table: the kind, the two lines, what happens and the size.
fn row_widget() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
    row.add_css_class("hk-managed-row");
    let icon = gtk::Image::new();
    icon.add_css_class("hk-t2");
    icon.set_size_request(ICON, -1);
    icon.set_valign(gtk::Align::Center);
    row.append(&icon);
    let paths = gtk::Box::new(gtk::Orientation::Vertical, 2);
    paths.set_hexpand(true);
    paths.set_valign(gtk::Align::Center);
    let top = label("", &["hk-mono", "hk-tool-path", "hk-t3", "hk-managed-from"]);
    top.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    top.set_max_width_chars(1);
    paths.append(&top);
    let bottom = label("", &["hk-mono", "hk-tool-path", "hk-medium"]);
    bottom.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    bottom.set_max_width_chars(1);
    paths.append(&bottom);
    row.append(&paths);
    let what = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    what.set_size_request(what_width(), -1);
    what.set_valign(gtk::Align::Center);
    let mark = gtk::Image::from_icon_name("helikodu-check-symbolic");
    mark.add_css_class("hk-t2");
    what.append(&mark);
    let warning = gtk::Image::from_icon_name("helikodu-warning-symbolic");
    warning.add_css_class("hk-t1");
    what.append(&warning);
    let meter = gtk::ProgressBar::new();
    meter.add_css_class("hk-tool-meter");
    meter.set_size_request(72, -1);
    meter.set_valign(gtk::Align::Center);
    meter.set_pulse_step(0.0);
    what.append(&meter);
    let words = label("", &["hk-t2"]);
    words.set_ellipsize(gtk::pango::EllipsizeMode::End);
    what.append(&words);
    what.set_hexpand(false);
    row.append(&what);
    let size = label("", &["hk-t2"]);
    size.set_size_request(SIZE, -1);
    size.set_xalign(1.0);
    size.set_valign(gtk::Align::Center);
    row.append(&size);
    row
}

/// The window, built and empty.
fn build(main: &MainWindow) -> Option<Rc<ManagedWindow>> {
    let backend = main.backend()?;
    let parent = main.upcast_ref::<gtk::Window>();
    let frame = frame(parent, NAME, &tr("Managed folder"), &tr("From"));
    let window = frame.window.clone();
    let list = frame.list.clone();
    list.set_vexpand(false);
    list.set_widget_name("managed-sources");
    // The places are not chosen: Delete removes the one with the focus.
    list.set_selection_mode(gtk::SelectionMode::None);
    // "Into": the managed folder and its pattern, under the places.
    let into = gtk::Box::new(gtk::Orientation::Vertical, 3);
    into.add_css_class("hk-managed-into");
    into.append(&label(&tr("Into"), &["hk-section-label"]));
    let into_folder = label("", &["hk-mono", "hk-managed-path", "hk-side-name"]);
    into_folder.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    into.append(&into_folder);
    let into_pattern = label("", &["hk-mono", "hk-t3", "hk-side-mono"]);
    into_pattern.set_wrap(true);
    into_pattern.set_wrap_mode(gtk::pango::WrapMode::Char);
    into_pattern.set_ellipsize(gtk::pango::EllipsizeMode::None);
    into_pattern.set_max_width_chars(24);
    into_pattern.set_single_line_mode(false);
    into.append(&into_pattern);
    into.set_vexpand(true);
    into.set_valign(gtk::Align::Start);
    if let Some(side) = list.parent().and_downcast::<gtk::Box>() {
        side.insert_child_after(&into, Some(&list));
    }

    let column = frame.main.clone();
    // The options: Copy or Move for this run, the filter, which rows.
    let options = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    options.add_css_class("hk-toolbar");
    options.add_css_class("hk-tool-toolbar");
    let (modes_box, modes) = segmented(&[tr("Copy"), tr("Move")]);
    modes_box.set_widget_name("managed-transfer");
    modes_box.update_property(&[gtk::accessible::Property::Label(&tr(
        "Copy or move the files",
    ))]);
    options.append(&modes_box);
    let copy_note = label(
        &tr("A copy leaves the original on the disk, and the library follows the copy."),
        &["hk-t3", "hk-secondary"],
    );
    copy_note.set_hexpand(true);
    copy_note.set_wrap(true);
    copy_note.set_ellipsize(gtk::pango::EllipsizeMode::None);
    options.append(&copy_note);
    let filter = gtk::SearchEntry::new();
    filter.add_css_class("hk-input");
    filter.set_hexpand(true);
    filter.set_valign(gtk::Align::Center);
    filter.set_placeholder_text(Some(&tr("Filter by name or folder")));
    filter.update_property(&[gtk::accessible::Property::Label(&tr("Filter the files"))]);
    filter.set_widget_name("managed-filter");
    options.append(&filter);
    let which = dropdown(
        &[
            tr("All files"),
            tr("Music only"),
            tr("Only files left where they are"),
        ],
        0,
        210,
    );
    which.set_widget_name("managed-shown");
    which.set_halign(gtk::Align::End);
    which.update_property(&[gtk::accessible::Property::Label(&tr("Files shown"))]);
    options.append(&which);
    column.append(&options);

    // The five figures.
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    strip.set_homogeneous(true);
    strip.add_css_class("hk-sync-figures");
    strip.set_widget_name("managed-figures");
    let mut figures = Vec::new();
    let space_icon = gtk::Image::from_icon_name("helikodu-warning-symbolic");
    space_icon.add_css_class("hk-t1");
    space_icon.update_property(&[gtk::accessible::Property::Label(&tr(
        "The files do not fit",
    ))]);
    for i in 0..5 {
        let figure = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let value = label("…", &["hk-sync-figure"]);
        if i == 4 {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            line.append(&space_icon);
            line.append(&value);
            figure.append(&line);
        } else {
            figure.append(&value);
        }
        let caption = label("", &["hk-t3", "hk-sync-caption"]);
        figure.append(&caption);
        strip.append(&figure);
        figures.push((value, caption));
    }
    column.append(&strip);
    // The notice of a plan that does not fit.
    let notice = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    notice.add_css_class("hk-notice");
    notice.add_css_class("hk-managed-notice");
    notice.set_widget_name("managed-notice");
    notice.set_visible(false);
    let warning = gtk::Image::from_icon_name("helikodu-warning-symbolic");
    warning.set_valign(gtk::Align::Start);
    notice.append(&warning);
    let notice_lines = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let notice_title = label("", &["hk-medium"]);
    notice_title.set_wrap(true);
    notice_title.set_ellipsize(gtk::pango::EllipsizeMode::None);
    notice_lines.append(&notice_title);
    let notice_text = label("", &["hk-t3", "hk-secondary"]);
    notice_text.set_wrap(true);
    notice_text.set_ellipsize(gtk::pango::EllipsizeMode::None);
    notice_lines.append(&notice_text);
    notice.append(&notice_lines);
    column.append(&notice);

    column.append(&table_header(&[
        (String::new(), ICON, false),
        (tr("Now, and in the managed folder"), -1, false),
        (tr("What happens"), what_width(), false),
        (tr("Size"), SIZE, true),
    ]));
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    let factory = gtk::SignalListItemFactory::new();
    let view = gtk::ListView::new(Some(selection), Some(factory.clone()));
    view.set_tab_behavior(gtk::ListTabBehavior::Item);
    view.add_css_class("hk-dup-list");
    view.add_css_class("hk-managed-list");
    view.set_widget_name("managed-table");
    view.update_property(&[gtk::accessible::Property::Label(&tr(
        "The files of the plan",
    ))]);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&view));
    let empty = label("", &["hk-t3", "hk-tool-empty"]);
    empty.set_widget_name("managed-empty");
    empty.set_xalign(0.5);
    empty.set_valign(gtk::Align::Start);
    empty.set_wrap(true);
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.add_named(&scroller, Some("table"));
    stack.add_named(&empty, Some("empty"));
    column.append(&stack);

    let (bar, summary) = footer();
    summary.set_widget_name("managed-summary");
    summary.set_wrap(true);
    summary.set_ellipsize(gtk::pango::EllipsizeMode::None);
    let details = compact_button(&tr("Details…"));
    details.set_widget_name("managed-details");
    details.set_visible(false);
    bar.append(&details);
    let show = compact_button(&tr("Show in the library"));
    show.set_widget_name("managed-show");
    show.set_visible(false);
    bar.append(&show);
    let primary = primary_button(&fill(&ntr("Copy {} file", "Copy {} files", 0), &["0"]));
    primary.set_widget_name("managed-run");
    primary.set_sensitive(false);
    bar.append(&primary);
    column.append(&bar);
    let work = WorkBar::new();
    work.root.set_widget_name("managed-work");
    work.meter
        .update_property(&[gtk::accessible::Property::Label(&tr("Placed so far"))]);
    column.append(&work.root);

    let transfer = match backend.services().settings().library.managed.mode {
        helikodu_model::settings::ManagedMode::Move => Transfer::Move,
        _ => Transfer::Copy,
    };
    let this = Rc::new(ManagedWindow {
        window: window.clone(),
        main: main.downgrade(),
        services: backend.services(),
        mode: Cell::new(Mode::Add),
        sources: RefCell::new(Vec::new()),
        roots: RefCell::new(Vec::new()),
        transfer: Cell::new(transfer),
        plan: RefCell::new(None),
        planning: RefCell::new(None),
        generation: Cell::new(0),
        problem: RefCell::new(None),
        shown: RefCell::new(Vec::new()),
        turns: RefCell::new(Vec::new()),
        in_turn: RefCell::new(Vec::new()),
        results: RefCell::new(HashMap::new()),
        showing_run: Cell::new(false),
        last_done: Cell::new(0),
        gather_folders: Cell::new(0),
        filling: Cell::new(false),
        list: list.clone(),
        into_folder,
        into_pattern,
        side_note: frame.side_note.clone(),
        title: frame.title.clone(),
        header_summary: frame.summary.clone(),
        options,
        modes: modes.clone(),
        copy_note,
        filter: filter.clone(),
        which: which.clone(),
        figures,
        space_icon,
        notice,
        notice_title,
        notice_text,
        store,
        stack,
        empty,
        footer: bar,
        summary,
        details: details.clone(),
        show: show.clone(),
        primary: primary.clone(),
        work,
    });

    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&row_widget()));
        }
    });
    let weak = Rc::downgrade(&this);
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
        let position = *object.borrow::<usize>();
        this.bind(&row, item, position);
    });

    let weak = Rc::downgrade(&this);
    bind_segmented(
        &modes,
        usize::from(transfer == Transfer::Move),
        move |index| {
            let Some(this) = weak.upgrade() else { return };
            if this.filling.get() {
                return;
            }
            let chosen = if index == 1 {
                Transfer::Move
            } else {
                Transfer::Copy
            };
            if this.transfer.get() != chosen {
                this.transfer.set(chosen);
                this.replan();
            }
        },
    );
    let weak = Rc::downgrade(&this);
    filter.connect_search_changed(move |_| {
        if let Some(this) = weak.upgrade() {
            this.filter_rows();
        }
    });
    let weak = Rc::downgrade(&this);
    on_selected(&which, move |_| {
        if let Some(this) = weak.upgrade() {
            this.filter_rows();
        }
    });
    let weak = Rc::downgrade(&this);
    list.connect_row_activated(move |_, row| {
        if row.widget_name() == "add-more"
            && let Some(this) = weak.upgrade()
        {
            this.ask_for_more();
        }
    });
    // A place leaves the run with Delete.
    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(&this);
    keys.connect_key_pressed(move |controller, key, _, state| {
        if key == gdk::Key::Delete
            && state.is_empty()
            && let Some(this) = weak.upgrade()
            && let Some(list) = controller.widget().and_downcast::<gtk::ListBox>()
            && let Some(row) = list.focus_child().and_downcast::<gtk::ListBoxRow>()
            && row.widget_name() != "add-more"
        {
            this.remove_source(&row);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    list.add_controller(keys);
    let weak = Rc::downgrade(&this);
    primary.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.start();
        }
    });
    let weak = Rc::downgrade(&this);
    details.connect_clicked(move |_| {
        if let Some(main) = weak.upgrade().and_then(|t| t.main_window()) {
            super::managed_cut_short::open(&main);
        }
    });
    let weak = Rc::downgrade(&this);
    show.connect_clicked(move |_| {
        let Some(this) = weak.upgrade() else { return };
        let Some(main) = this.main_window() else {
            return;
        };
        if let Some(Some(Ok(report))) = this.run().map(|r| r.result) {
            managed_runs::mark_told();
            show_report(&main, &report);
        }
    });
    this.work.stop.connect_clicked(|button| {
        button.set_sensitive(false);
        managed_runs::stop();
    });
    // The plan is worked out again when the window gets the focus back while
    // the files do not fit: space may have been freed meanwhile.
    let weak = Rc::downgrade(&this);
    window.connect_is_active_notify(move |window| {
        let Some(this) = weak.upgrade() else { return };
        let full = this.plan.borrow().as_ref().is_some_and(|p| !p.fits());
        if window.is_active() && full && !this.showing_run.get() {
            this.replan();
        }
    });
    let weak = Rc::downgrade(&this);
    let id = managed_runs::listen(move || {
        if let Some(this) = weak.upgrade() {
            this.run_changed();
        }
    });
    // The window keeps its state while it is open; a run goes on after it
    // closes, and its end is told by a toast then.
    let keep = Rc::new(RefCell::new(Some(this.clone())));
    let main_weak = main.downgrade();
    let forget = move || {
        managed_runs::unlisten(id);
        if let Some(this) = keep.borrow_mut().take()
            && let Some(cancel) = this.planning.take()
        {
            cancel.cancel();
        }
    };
    let on_destroy = forget.clone();
    window.connect_destroy(move |_| on_destroy());
    window.connect_close_request(move |_| {
        forget();
        if managed_runs::untold()
            && let Some(main) = main_weak.upgrade()
        {
            managed_runs::toast_done(&main);
        }
        glib::Propagation::Proceed
    });
    OPEN.with(|open| open.replace(Rc::downgrade(&this)));
    Some(this)
}

fn show_report(main: &MainWindow, report: &ManagedReport) {
    managed_runs::show_in_library(main, report);
}

/// The window open over `main`, if one is.
fn open_window(main: &MainWindow) -> Option<Rc<ManagedWindow>> {
    find_open(main.upcast_ref(), NAME)?;
    OPEN.with(|open| open.borrow().upgrade())
}

/// Whether the managed folder's window is open over `main`.
pub(crate) fn is_open(main: &MainWindow) -> bool {
    open_window(main).is_some()
}

fn finish_opening(this: &Rc<ManagedWindow>, mode: Mode) {
    this.side_note.set_text(&match mode {
        Mode::Add => {
            tr("Nothing changes until you start. Your choices here are for this run only.")
        }
        Mode::Gather => tr(
            "Tracks stay the same tracks: ratings, play counts, labels and playlists follow the files.",
        ),
    });
    this.window.present();
}

/// Opens the plan of adding `paths` to the managed folder; an open window of
/// the plan takes them as more places.
pub(crate) fn open_add(main: &MainWindow, paths: Vec<PathBuf>) -> Option<gtk::Window> {
    if let Some(this) = open_window(main) {
        if this.run().is_some_and(|r| r.is_working()) {
            this.window.present();
            return Some(this.window.clone());
        }
        if this.mode.get() == Mode::Gather {
            this.mode.set(Mode::Add);
            this.sources.borrow_mut().clear();
        }
        finish_opening(&this, Mode::Add);
        this.add_sources(paths);
        return Some(this.window.clone());
    }
    let this = build(main)?;
    if this.adopt_run() {
        finish_opening(&this, this.mode.get());
        return Some(this.window.clone());
    }
    this.mode.set(Mode::Add);
    this.sources.replace(paths);
    this.replan();
    finish_opening(&this, Mode::Add);
    Some(this.window.clone())
}

/// Opens the plan of gathering the library into the managed folder.
pub(crate) fn open_gather(main: &MainWindow) -> Option<gtk::Window> {
    if let Some(this) = open_window(main) {
        if !this.run().is_some_and(|r| r.is_working()) && this.mode.get() != Mode::Gather {
            this.mode.set(Mode::Gather);
            this.sources.borrow_mut().clear();
            this.replan();
        }
        finish_opening(&this, this.mode.get());
        return Some(this.window.clone());
    }
    let this = build(main)?;
    if this.adopt_run() {
        finish_opening(&this, this.mode.get());
        return Some(this.window.clone());
    }
    this.mode.set(Mode::Gather);
    this.replan();
    finish_opening(&this, Mode::Gather);
    Some(this.window.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: ItemKind, action: ItemAction) -> ManagedItem {
        ManagedItem {
            kind,
            file: None,
            from: PathBuf::from("/in/a.flac"),
            to: PathBuf::from("/m/a.flac"),
            size: 1,
            action,
            renamed: false,
            rewritten: false,
        }
    }

    #[test]
    fn each_file_says_what_happens_to_it() {
        assert_eq!(
            planned_text(&item(ItemKind::Audio, ItemAction::Copy)),
            "Copy"
        );
        assert_eq!(
            planned_text(&item(ItemKind::Audio, ItemAction::Move)),
            "Move"
        );
        assert_eq!(
            planned_text(&item(ItemKind::Audio, ItemAction::CopyKeepingSource)),
            "Copy, the original stays"
        );
        assert_eq!(
            planned_text(&item(ItemKind::Lyrics, ItemAction::Copy)),
            "Copy, named like its track"
        );
        let mut numbered = item(ItemKind::Audio, ItemAction::Copy);
        numbered.renamed = true;
        assert_eq!(planned_text(&numbered), "Gets a number");
        let mut sheet = item(ItemKind::CueSheet, ItemAction::Copy);
        sheet.rewritten = true;
        assert_eq!(planned_text(&sheet), "CUE sheet updated");
        assert_eq!(
            planned_text(&item(
                ItemKind::Audio,
                ItemAction::Skip(SkipReason::Identical)
            )),
            "Left where it is"
        );
        assert_eq!(
            reason_text(&SkipReason::Unreadable("the file is empty".into())),
            "It cannot be read: the file is empty"
        );
        assert_eq!(
            result_text(
                &ItemResult::Skipped(SkipReason::Failed("disk full".into())),
                Transfer::Copy
            ),
            ("Could not be copied: disk full".into(), false, true)
        );
    }
}
