// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify files: decodes files from start to end and lists those that do not
//! decode cleanly. The left pane counts the results (all files, no problems,
//! problems, could not be read) and says what the last check read and when;
//! the toolbar has a filter, what to check, and "Verify again"; each row names
//! the file, says in one plain sentence what was found and what it means, its
//! format and "Show in folder". The footer has the totals, "Copy as text" and
//! "Save report…": there is no primary button, because the tool changes
//! nothing. At work the footer shows a meter and "Stop", and rows appear as
//! problems are found.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};

use super::file_tools::{WorkBar, choose_folder, count_row, relative};
use super::tool_window::{Frame, GAP, footer, frame, table_header};
use crate::backend::Backend;
use crate::backend::TrackKey;
use crate::backend::services::{
    Services, VerifiedFormat, VerifyProblem, VerifyReport, VerifyScope,
};
use crate::dialogs::common::{compact_button, copy_text, dropdown, find_open, on_selected, tilde};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the window is known by among the toplevels.
pub(crate) const NAME: &str = "verify-files";

/// The width of the Format column.
const FORMAT: i32 = 104;

/// The groups of the left pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    All,
    Clean,
    Problems,
    Unreadable,
}

const GROUPS: [Group; 4] = [Group::All, Group::Clean, Group::Problems, Group::Unreadable];

fn group_name(group: Group) -> String {
    match group {
        Group::All => tr("All files"),
        Group::Clean => tr("No problems"),
        Group::Problems => tr("Problems"),
        Group::Unreadable => tr("Could not be read"),
    }
}

/// `d` as minutes and seconds: `3:41`.
fn clock(d: Duration) -> String {
    format::duration(d)
}

/// The format of a checked file: `FLAC 16/44.1`, `DSD64`.
fn format_text(format: Option<&VerifiedFormat>) -> String {
    let Some(f) = format else {
        return String::new();
    };
    let codec = match f.codec.as_str() {
        "flac" => "FLAC".to_owned(),
        "alac" => "ALAC".to_owned(),
        "wavpack" => "WavPack".to_owned(),
        "ape" => "APE".to_owned(),
        "mp3" | "mp3float" => "MP3".to_owned(),
        "aac" => "AAC".to_owned(),
        "vorbis" => "Vorbis".to_owned(),
        "opus" => "Opus".to_owned(),
        other if other.starts_with("dsd") => {
            let rate = f.rate.unwrap_or(2_822_400);
            return format!("DSD{}", rate / 44_100);
        }
        other if other.starts_with("pcm_") => "PCM".to_owned(),
        other => other.to_uppercase(),
    };
    match (f.bits, f.rate) {
        (Some(bits), Some(rate)) => {
            let khz = f64::from(rate) / 1000.0;
            let khz = if (khz - khz.round()).abs() < 0.05 {
                format!("{}", khz.round())
            } else {
                format!("{khz:.1}")
            };
            format!("{codec} {bits}/{khz}")
        }
        _ => codec,
    }
}

/// What was found, in one plain sentence.
fn sentence(report: &VerifyReport) -> String {
    if let Some(reason) = &report.unreadable {
        return fill(&tr("The file could not be opened: {}."), &[reason]);
    }
    let Some(problem) = report.problems.first() else {
        return tr("Decoded cleanly from start to end.");
    };
    match problem {
        VerifyProblem::Skipped { count, at } => {
            let n = usize::try_from(*count).unwrap_or(usize::MAX);
            match at {
                Some(at) => fill(
                    &ntr(
                        "{} frame could not be decoded, around {}. A click or a short gap is likely there.",
                        "{} frames could not be decoded, around {}. A click or a short gap is likely there.",
                        n,
                    ),
                    &[&format::count(n), &clock(*at)],
                ),
                None => fill(
                    &ntr(
                        "{} frame could not be decoded. A click or a short gap is likely there.",
                        "{} frames could not be decoded. A click or a short gap is likely there.",
                        n,
                    ),
                    &[&format::count(n)],
                ),
            }
        }
        VerifyProblem::Stopped { at, .. } => fill(
            &tr("The file stops in the middle at {}; the rest cannot be played."),
            &[&clock(*at)],
        ),
        VerifyProblem::Shorter { stated, decoded }
            if stated.saturating_sub(*decoded) < Duration::from_secs(10) =>
        {
            fill(
                &tr(
                    "Ends {} s before the length written in the file. The end of the track may be missing.",
                ),
                &[&format!(
                    "{:.1}",
                    stated.saturating_sub(*decoded).as_secs_f64()
                )],
            )
        }
        VerifyProblem::Shorter { stated, decoded } | VerifyProblem::Longer { stated, decoded } => {
            fill(
                &tr("The header gives {}, the audio is {} long."),
                &[&clock(*stated), &clock(*decoded)],
            )
        }
        VerifyProblem::ChecksumMismatch => {
            tr("The audio does not match the checksum stored in the file: some bytes have changed.")
        }
        _ => tr("The file does not decode cleanly."),
    }
}

struct Verify {
    window: gtk::Window,
    backend: Backend,
    tracks: Option<Vec<TrackKey>>,
    roots: RefCell<Vec<PathBuf>>,
    scopes: RefCell<Vec<Option<VerifyScope>>>,
    folder: RefCell<Option<PathBuf>>,
    reports: RefCell<Vec<VerifyReport>>,
    total: Cell<usize>,
    group: Cell<Group>,
    shown: RefCell<Vec<usize>>,
    store: gio::ListStore,
    counts: Vec<(Group, gtk::Label)>,
    filter: gtk::SearchEntry,
    check: gtk::DropDown,
    again: gtk::Button,
    title: gtk::Label,
    header_summary: gtk::Label,
    last_place: gtk::Label,
    last_when: gtk::Label,
    summary: gtk::Label,
    footer: gtk::Box,
    copy: gtk::Button,
    save: gtk::Button,
    work: WorkBar,
    empty: gtk::Label,
    stack: gtk::Stack,
    running: RefCell<Option<Arc<AtomicBool>>>,
    filling: Cell<bool>,
    checked: Cell<bool>,
}

impl Verify {
    fn services(&self) -> Arc<dyn Services> {
        self.backend.services()
    }

    fn fill_check(&self) {
        let mut items = vec![tr("The whole library")];
        let mut scopes = vec![Some(VerifyScope::Library)];
        for root in self.roots.borrow().iter() {
            items.push(tilde(root));
            scopes.push(Some(VerifyScope::Folder(root.clone())));
        }
        if let Some(tracks) = &self.tracks {
            items.push(tr("The selected tracks"));
            scopes.push(Some(VerifyScope::Tracks(tracks.clone())));
        }
        if let Some(folder) = self.folder.borrow().as_ref()
            && !self.roots.borrow().contains(folder)
        {
            items.push(tilde(folder));
            scopes.push(Some(VerifyScope::Folder(folder.clone())));
        }
        items.push(tr("Choose a folder…"));
        scopes.push(None);
        let selected = if self.tracks.is_some() && self.folder.borrow().is_none() {
            scopes
                .iter()
                .position(|s| matches!(s, Some(VerifyScope::Tracks(_))))
        } else {
            self.folder.borrow().as_ref().and_then(|f| {
                scopes
                    .iter()
                    .position(|s| s.as_ref() == Some(&VerifyScope::Folder(f.clone())))
            })
        }
        .unwrap_or(0);
        self.filling.set(true);
        let strings: Vec<&str> = items.iter().map(String::as_str).collect();
        self.check.set_model(Some(&gtk::StringList::new(&strings)));
        self.check
            .set_selected(u32::try_from(selected).unwrap_or(0));
        self.filling.set(false);
        self.scopes.replace(scopes);
    }

    fn scope(&self) -> VerifyScope {
        self.scopes
            .borrow()
            .get(self.check.selected() as usize)
            .cloned()
            .flatten()
            .unwrap_or(VerifyScope::Library)
    }

    fn scope_text(scope: &VerifyScope) -> String {
        match scope {
            VerifyScope::Library => tr("The whole library"),
            VerifyScope::Folder(folder) => tilde(folder),
            VerifyScope::Tracks(_) => tr("The selected tracks"),
        }
    }

    fn load_roots(self: &Rc<Self>, then_check: bool) {
        let services = self.services();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (sender, receiver) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = sender.send_blocking(services.library_folders());
            });
            let Ok(folders) = receiver.recv().await else {
                return;
            };
            let Some(this) = weak.upgrade() else { return };
            this.roots.replace(
                folders
                    .map(|f| f.into_iter().map(|f| f.path).collect())
                    .unwrap_or_default(),
            );
            this.fill_check();
            if then_check {
                this.start();
            }
        });
    }

    /// Starts a check of the chosen files.
    fn start(self: &Rc<Self>) {
        if self.running.borrow().is_some() {
            return;
        }
        let scope = self.scope();
        let stop = Arc::new(AtomicBool::new(false));
        self.running.replace(Some(stop.clone()));
        self.reports.borrow_mut().clear();
        self.total.set(0);
        self.checked.set(true);
        self.set_busy(true);
        self.rebuild();
        let place = Self::scope_text(&scope);
        let place_is_folder = matches!(scope, VerifyScope::Folder(_));
        let started = Instant::now();
        let services = self.services();
        let (sender, receiver) = async_channel::unbounded::<Message>();
        let spawned = std::thread::Builder::new()
            .name("helikodu-verify".into())
            .spawn(move || {
                let paths = services.verify_targets(&scope).unwrap_or_default();
                let _ = sender.send_blocking(Message::Total(paths.len()));
                let done = std::sync::atomic::AtomicUsize::new(0);
                services.verify_files(&paths, &|| stop.load(Ordering::Relaxed), &|_, report| {
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    let _ = sender.send_blocking(Message::Report(n, report));
                });
                let _ = sender.send_blocking(Message::Done);
            });
        if spawned.is_err() {
            self.running.replace(None);
            self.set_busy(false);
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(message) = receiver.recv().await {
                let Some(this) = weak.upgrade() else { return };
                match message {
                    Message::Total(n) => {
                        this.total.set(n);
                        this.work.show(0, n);
                    }
                    Message::Report(done, report) => {
                        this.work.show(done, this.total.get());
                        let interesting = !report.is_clean();
                        this.reports.borrow_mut().push(report);
                        if interesting || this.group.get() == Group::All {
                            this.rebuild();
                        } else {
                            this.update_counts();
                        }
                    }
                    Message::Done => {
                        this.running.replace(None);
                        this.set_busy(false);
                        this.last_place.set_text(&place);
                        if place_is_folder {
                            this.last_place.add_css_class("hk-mono");
                        } else {
                            this.last_place.remove_css_class("hk-mono");
                        }
                        let now = glib::DateTime::now_local()
                            .ok()
                            .and_then(|t| t.format("%H:%M").ok())
                            .map(|t| t.to_string())
                            .unwrap_or_default();
                        let took = started.elapsed().as_secs();
                        let took = if took >= 60 {
                            fill(
                                &ntr("{} min", "{} min", (took / 60) as usize),
                                &[&(took / 60).to_string()],
                            )
                        } else {
                            fill(&ntr("{} s", "{} s", took as usize), &[&took.to_string()])
                        };
                        this.last_when
                            .set_text(&fill(&tr("Today at {} · took {}"), &[&now, &took]));
                        this.rebuild();
                        return;
                    }
                }
            }
        });
    }

    fn set_busy(&self, busy: bool) {
        self.footer.set_visible(!busy);
        self.work.root.set_visible(busy);
        self.again.set_sensitive(!busy);
        self.check.set_sensitive(!busy);
        self.again.set_label(&if self.checked.get() {
            tr("Verify again")
        } else {
            tr("Verify")
        });
    }

    fn in_group(report: &VerifyReport, group: Group) -> bool {
        match group {
            Group::All => true,
            Group::Clean => report.is_clean(),
            Group::Problems => report.unreadable.is_none() && !report.problems.is_empty(),
            Group::Unreadable => report.unreadable.is_some(),
        }
    }

    fn update_counts(&self) {
        let reports = self.reports.borrow();
        for (group, count) in &self.counts {
            let n = reports.iter().filter(|r| Self::in_group(r, *group)).count();
            count.set_text(&if self.checked.get() {
                format::count(n)
            } else {
                String::new()
            });
        }
        let clean = reports.iter().filter(|r| r.is_clean()).count();
        let unreadable = reports.iter().filter(|r| r.unreadable.is_some()).count();
        let problems = reports.len() - clean - unreadable;
        self.summary.set_text(&if !self.checked.get() {
            tr("Choose what to check and press Verify. A check changes nothing.")
        } else {
            fill(
                &tr("Every file was decoded from start to end. {} fine, {} and {}."),
                &[
                    &fill(&ntr("{} is", "{} are", clean), &[&format::count(clean)]),
                    &fill(
                        &ntr("{} has problems", "{} have problems", problems),
                        &[&format::count(problems)],
                    ),
                    &fill(
                        &ntr("{} could not be read", "{} could not be read", unreadable),
                        &[&format::count(unreadable)],
                    ),
                ],
            )
        });
        let any = problems + unreadable > 0;
        self.copy.set_sensitive(any);
        self.save.set_sensitive(any);
    }

    fn rebuild(&self) {
        let group = self.group.get();
        let text = self.filter.text().trim().to_lowercase();
        let shown: Vec<usize> = self
            .reports
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, r)| Self::in_group(r, group))
            .filter(|(_, r)| {
                text.is_empty() || r.path.to_string_lossy().to_lowercase().contains(&text)
            })
            .map(|(i, _)| i)
            .collect();
        let count = shown.len();
        self.shown.replace(shown);
        let items: Vec<glib::BoxedAnyObject> = (0..count).map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &items);
        self.title.set_text(&group_name(group));
        let total = self.reports.borrow().len().max(self.total.get());
        self.header_summary.set_text(&fill(
            &tr("{} of {}"),
            &[
                &format::count(count),
                &fill(&ntr("{} file", "{} files", total), &[&format::count(total)]),
            ],
        ));
        self.stack
            .set_visible_child_name(if count == 0 { "empty" } else { "table" });
        self.empty.set_text(&if !self.checked.get() {
            tr("Nothing has been checked yet.")
        } else if self.running.borrow().is_some() {
            tr("Checking… files with problems appear here as they are found.")
        } else {
            tr("No file here.")
        });
        self.update_counts();
    }

    fn report_text(&self) -> String {
        let roots = self.roots.borrow();
        let mut out = String::new();
        for report in self.reports.borrow().iter().filter(|r| !r.is_clean()) {
            out.push_str(&relative(&report.path, &roots));
            out.push('\t');
            out.push_str(&format_text(report.format.as_ref()));
            out.push('\t');
            out.push_str(&sentence(report));
            out.push('\n');
        }
        out
    }

    fn save_report(self: &Rc<Self>) {
        let text = self.report_text();
        let dialog = gtk::FileDialog::builder()
            .title(tr("Save the report"))
            .accept_label(tr("Save"))
            .initial_name(tr("verify-report.txt"))
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |result| {
            if let Ok(Some(path)) = result.map(|f| f.path())
                && let Err(error) = std::fs::write(&path, text)
                && let Some(this) = weak.upgrade()
            {
                this.summary.set_text(&fill(
                    &tr("The report could not be saved: {}"),
                    &[&error.to_string()],
                ));
            }
        });
    }

    fn report(&self, position: u32) -> Option<VerifyReport> {
        let index = *self.shown.borrow().get(position as usize)?;
        self.reports.borrow().get(index).cloned()
    }

    fn bind(&self, row: &gtk::Box, item: &gtk::ListItem, position: u32) {
        let Some(report) = self.report(position) else {
            return;
        };
        let cells = children(row);
        let [icon, text, format_cell, show] = cells.as_slice() else {
            return;
        };
        icon.set_opacity(if report.is_clean() { 0.0 } else { 1.0 });
        let path = relative(&report.path, &self.roots.borrow());
        let what = sentence(&report);
        let lines = children(text);
        if let [p, w] = lines.as_slice() {
            set_label(p, &path);
            p.set_tooltip_text(Some(&tilde(&report.path)));
            set_label(w, &what);
        }
        let format = format_text(report.format.as_ref());
        set_label(format_cell, &format);
        show.set_widget_name(&format!("show-{position}"));
        item.set_accessible_label(&fill(&tr("{}, {}, {}"), &[&path, &what, &format]));
    }

    fn show_in_folder(&self, position: u32) {
        let (Some(report), Some(main)) = (
            self.report(position),
            self.window.transient_for().and_downcast::<MainWindow>(),
        ) else {
            return;
        };
        crate::views::actions::open_folder(&main, Some(report.path));
    }
}

enum Message {
    Total(usize),
    Report(usize, VerifyReport),
    Done,
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

fn row_widget(verify: &std::rc::Weak<Verify>, item: &gtk::ListItem) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
    row.add_css_class("hk-verify-row");
    let icon = gtk::Image::from_icon_name("helikodu-warning-symbolic");
    icon.add_css_class("hk-t1");
    icon.set_valign(gtk::Align::Center);
    row.append(&icon);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let path = label("", &["hk-mono", "hk-tool-path"]);
    path.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    path.set_max_width_chars(1);
    text.append(&path);
    let what = label("", &["hk-t2", "hk-secondary"]);
    what.set_max_width_chars(1);
    text.append(&what);
    row.append(&text);
    let format = label("", &["hk-mono-meta", "hk-t3"]);
    format.set_size_request(FORMAT, -1);
    row.append(&format);
    let show = compact_button(&tr("Show in folder"));
    let weak = verify.clone();
    let item = item.downgrade();
    show.connect_clicked(move |_| {
        if let (Some(this), Some(item)) = (weak.upgrade(), item.upgrade()) {
            this.show_in_folder(item.position());
        }
    });
    row.append(&show);
    row
}

/// Opens Verify files over `main` for `tracks` (the whole library when
/// `None`), or brings it to the front. Opened for tracks, or with `check`, it
/// starts checking at once.
pub(crate) fn open(
    main: &MainWindow,
    tracks: Option<Vec<TrackKey>>,
    check: bool,
) -> Option<gtk::Window> {
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
    } = frame(parent, NAME, &tr("Verify files"), &tr("Result"));
    side_note.set_text(&tr("A check reads every file once. It changes nothing."));
    let mut counts = Vec::new();
    for group in GROUPS {
        let (row, count) = count_row(&group_name(group));
        list.append(&row);
        counts.push((group, count));
    }
    list.set_vexpand(false);
    let last_place = label("", &["hk-side-mono", "hk-t2"]);
    let last_when = label("", &["hk-t3", "hk-secondary"]);
    if let Some(side) = list.parent().and_downcast::<gtk::Box>() {
        let last = gtk::Box::new(gtk::Orientation::Vertical, 2);
        last.add_css_class("hk-verify-last");
        last.set_vexpand(true);
        last.set_valign(gtk::Align::Start);
        last.append(&label(
            &tr("Last check"),
            &["hk-section-label", "hk-list-section"],
        ));
        last_place.set_margin_start(18);
        last_when.set_margin_start(18);
        last.append(&last_place);
        last.append(&last_when);
        side.insert_child_after(&last, Some(&list));
    }

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    toolbar.add_css_class("hk-toolbar");
    toolbar.add_css_class("hk-tool-toolbar");
    let filter = gtk::SearchEntry::new();
    filter.add_css_class("hk-input");
    filter.set_hexpand(true);
    filter.set_valign(gtk::Align::Center);
    filter.set_placeholder_text(Some(&tr("Filter by file name or folder")));
    filter.update_property(&[gtk::accessible::Property::Label(&tr("Filter the files"))]);
    toolbar.append(&filter);
    let check_label = label(&tr("Check"), &["hk-t2", "hk-keep-label"]);
    toolbar.append(&check_label);
    let start_at_once = check || tracks.is_some();
    let check = dropdown(&[tr("The whole library")], 0, 220);
    check.set_widget_name("verify-scope");
    check.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        check_label.upcast_ref()
    ])]);
    toolbar.append(&check);
    let again = compact_button(&tr("Verify"));
    again.set_widget_name("verify-again");
    toolbar.append(&again);
    column.append(&toolbar);
    let head = table_header(&[
        (String::new(), 16, false),
        (tr("File, and what was found"), -1, false),
        (tr("Format"), FORMAT, false),
    ]);
    // Room for "Show in folder".
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_size_request(112, -1);
    head.append(&spacer);
    column.append(&head);

    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    let factory = gtk::SignalListItemFactory::new();
    let view = gtk::ListView::new(Some(selection), Some(factory.clone()));
    view.add_css_class("hk-dup-list");
    view.add_css_class("hk-verify-list");
    view.set_widget_name("verify-table");
    view.update_property(&[gtk::accessible::Property::Label(&tr("Checked files"))]);
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
    let (bar, summary) = footer();
    summary.set_wrap(true);
    summary.set_single_line_mode(false);
    summary.set_ellipsize(gtk::pango::EllipsizeMode::None);
    let copy = compact_button(&tr("Copy as text"));
    copy.set_widget_name("verify-copy");
    bar.append(&copy);
    let save = compact_button(&tr("Save report…"));
    save.set_widget_name("verify-save");
    bar.append(&save);
    column.append(&bar);
    let work = WorkBar::new();
    column.append(&work.root);

    let verify = Rc::new(Verify {
        window: window.clone(),
        backend,
        tracks,
        roots: RefCell::new(Vec::new()),
        scopes: RefCell::new(vec![Some(VerifyScope::Library)]),
        folder: RefCell::new(None),
        reports: RefCell::new(Vec::new()),
        total: Cell::new(0),
        group: Cell::new(Group::Problems),
        shown: RefCell::new(Vec::new()),
        store,
        counts,
        filter: filter.clone(),
        check: check.clone(),
        again: again.clone(),
        title,
        header_summary,
        last_place,
        last_when,
        summary,
        footer: bar,
        copy: copy.clone(),
        save: save.clone(),
        work,
        empty,
        stack,
        running: RefCell::new(None),
        filling: Cell::new(false),
        checked: Cell::new(false),
    });

    let weak = Rc::downgrade(&verify);
    factory.connect_setup(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&row_widget(&weak, item)));
            item.set_activatable(false);
        }
    });
    let weak = Rc::downgrade(&verify);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(this), Some(row)) = (weak.upgrade(), item.child().and_downcast::<gtk::Box>())
        else {
            return;
        };
        this.bind(&row, item, item.position());
    });
    let weak = Rc::downgrade(&verify);
    list.connect_row_selected(move |_, row| {
        let (Some(this), Some(row)) = (weak.upgrade(), row) else {
            return;
        };
        if let Some(group) = usize::try_from(row.index())
            .ok()
            .and_then(|i| GROUPS.get(i))
        {
            this.group.set(*group);
            this.rebuild();
        }
    });
    let weak = Rc::downgrade(&verify);
    filter.connect_search_changed(move |_| {
        if let Some(this) = weak.upgrade() {
            this.rebuild();
        }
    });
    let weak = Rc::downgrade(&verify);
    on_selected(&check, move |index| {
        let Some(this) = weak.upgrade() else { return };
        if this.filling.get() {
            return;
        }
        if this.scopes.borrow().get(index).is_some_and(Option::is_none) {
            let weak = Rc::downgrade(&this);
            choose_folder(
                &this.window,
                &tr("Verify the files in a folder"),
                &tr("Choose"),
                None,
                move |folder| {
                    if let Some(this) = weak.upgrade() {
                        this.folder.replace(Some(folder));
                        this.fill_check();
                    }
                },
            );
            this.filling.set(true);
            this.check.set_selected(0);
            this.filling.set(false);
        }
    });
    let weak = Rc::downgrade(&verify);
    again.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.start();
        }
    });
    let weak = Rc::downgrade(&verify);
    verify.work.stop.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade()
            && let Some(stop) = this.running.borrow().as_ref()
        {
            stop.store(true, Ordering::Relaxed);
        }
    });
    let weak = Rc::downgrade(&verify);
    copy.connect_clicked(move |button| {
        if let Some(this) = weak.upgrade() {
            copy_text(button, &this.report_text());
        }
    });
    let weak = Rc::downgrade(&verify);
    save.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.save_report();
        }
    });
    // A check in progress stops with the window.
    let weak = Rc::downgrade(&verify);
    window.connect_close_request(move |_| {
        if let Some(this) = weak.upgrade()
            && let Some(stop) = this.running.borrow().as_ref()
        {
            stop.store(true, Ordering::Relaxed);
        }
        glib::Propagation::Proceed
    });
    let keep_alive = RefCell::new(Some(verify.clone()));
    window.connect_destroy(move |_| {
        keep_alive.borrow_mut().take();
    });
    if let Some(row) = list.row_at_index(2) {
        list.select_row(Some(&row));
    }
    verify.set_busy(false);
    verify.rebuild();
    verify.load_roots(start_at_once);
    window.present();
    Some(window)
}
