// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The runs of the managed folder as the windows show them: the run that
//! works (adding music, gathering the library, going on with a run that was
//! cut short, or rolling one back), the run that was cut short, and the
//! application actions that open the managed folder's window.
//!
//! One run works at a time, on a thread of its own; its progress arrives on the
//! main loop and is kept here, so the managed folder's window, Settings ›
//! Library and the sidebar footer show the same run, and the run goes on when
//! a window closes. Nothing runs and nothing is drawn while no run works.
//!
//! A finished run has no undo. A run that stops before its end (Stop, or a
//! crash) is cut short: a toast with "Details…" leads to the dialog that
//! continues it or rolls it back, and no new run starts until then.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::file_tools::run_in_background;
use crate::backend::services::{
    CancelToken, ItemKind, ItemResult, ManagedOperation, ManagedPlan, ManagedReport,
    ManagedRunSummary, Services, Transfer,
};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::{icon_button, label};
use crate::window::{MainWindow, ToastLead};

/// What a run does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Work {
    /// Carries out a plan.
    Run,
    /// Goes on with a run that was cut short.
    Resume,
    /// Rolls back a run that was cut short.
    RollBack,
}

/// The run that works or worked last, and where it stands.
#[derive(Debug, Clone)]
pub(crate) struct RunView {
    /// What it does.
    pub(crate) work: Work,
    /// Adding music or gathering the library.
    pub(crate) operation: ManagedOperation,
    /// Whether it copies or moves.
    pub(crate) transfer: Transfer,
    /// The plan it carries out, for a run started from the window.
    pub(crate) plan: Option<Arc<ManagedPlan>>,
    /// The files done so far.
    pub(crate) done: usize,
    /// The files to do.
    pub(crate) total: usize,
    /// The bytes of the files done, when the plan is known.
    pub(crate) bytes: u64,
    /// How it ended, once it has.
    pub(crate) result: Option<Result<ManagedReport, String>>,
}

impl RunView {
    /// Whether it still works.
    pub(crate) fn is_working(&self) -> bool {
        self.result.is_none()
    }

    /// The share done, from 0 to 1.
    pub(crate) fn fraction(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f64 / self.total as f64).clamp(0.0, 1.0)
        }
    }
}

type Listener = Box<dyn Fn()>;

#[derive(Default)]
struct Hub {
    run: RefCell<Option<RunView>>,
    cancel: RefCell<Option<CancelToken>>,
    interrupted: RefCell<Option<ManagedRunSummary>>,
    /// Whether the end of the last run was told (by its window or a toast).
    told: Cell<bool>,
    listeners: RefCell<Vec<(u64, Listener)>>,
    next: Cell<u64>,
}

thread_local! {
    static HUB: Rc<Hub> = Rc::new(Hub::default());
}

fn hub() -> Rc<Hub> {
    HUB.with(Rc::clone)
}

/// The run that works or worked last.
pub(crate) fn current() -> Option<RunView> {
    hub().run.borrow().clone()
}

/// Whether a run works now.
pub(crate) fn is_working() -> bool {
    hub().run.borrow().as_ref().is_some_and(RunView::is_working)
}

/// The run that was cut short, as last read.
pub(crate) fn interrupted() -> Option<ManagedRunSummary> {
    hub().interrupted.borrow().clone()
}

/// Calls `listener` on the main loop after each change. Returns its handle.
pub(crate) fn listen(listener: impl Fn() + 'static) -> u64 {
    let hub = hub();
    let id = hub.next.get() + 1;
    hub.next.set(id);
    hub.listeners.borrow_mut().push((id, Box::new(listener)));
    id
}

/// Stops calling the listener `id`.
pub(crate) fn unlisten(id: u64) {
    hub().listeners.borrow_mut().retain(|(i, _)| *i != id);
}

fn notify() {
    let hub = hub();
    // A listener may add or remove listeners; it sees the list as it was.
    let listeners = std::mem::take(&mut *hub.listeners.borrow_mut());
    for (_, listener) in &listeners {
        listener();
    }
    let mut now = hub.listeners.borrow_mut();
    let added = std::mem::take(&mut *now);
    *now = listeners;
    now.extend(added);
}

/// Stops the run that works after the file it is on; it is then cut short.
pub(crate) fn stop() {
    if let Some(cancel) = hub().cancel.borrow().as_ref() {
        cancel.cancel();
    }
}

/// Marks the end of the last run as told, so that no toast repeats it.
pub(crate) fn mark_told() {
    hub().told.set(true);
}

/// Whether the end of the last run is still to be told.
pub(crate) fn untold() -> bool {
    let hub = hub();
    !hub.told.get() && hub.run.borrow().as_ref().is_some_and(|r| !r.is_working())
}

/// Reads the run that was cut short again, then calls `then`.
pub(crate) fn refresh(services: Arc<dyn Services>, then: impl FnOnce() + 'static) {
    glib::spawn_future_local(async move {
        let found = gio::spawn_blocking(move || services.interrupted_managed_run())
            .await
            .ok()
            .and_then(Result::ok)
            .flatten();
        hub().interrupted.replace(found);
        notify();
        then();
    });
}

/// The words of a run's operation, as a toast or a title says it was cut short.
pub(crate) fn cut_short_text(operation: ManagedOperation) -> String {
    match operation {
        ManagedOperation::Consolidate => tr("Gathering the library was cut short"),
        _ => tr("Adding music to the managed folder was cut short"),
    }
}

/// How many files a report placed, in words: "214 files were added to the
/// managed folder".
pub(crate) fn done_text(operation: ManagedOperation, report: &ManagedReport) -> String {
    let n = report.transferred();
    match operation {
        ManagedOperation::Consolidate => fill(
            &ntr(
                "{} file was gathered into the managed folder",
                "{} files were gathered into the managed folder",
                n,
            ),
            &[&format::count(n)],
        ),
        _ => fill(
            &ntr(
                "{} file was added to the managed folder",
                "{} files were added to the managed folder",
                n,
            ),
            &[&format::count(n)],
        ),
    }
}

/// The toast of a run that was cut short, with "Details…"; it stays until it is
/// dismissed or the run is settled.
fn toast_cut_short(main: &MainWindow, summary: &ManagedRunSummary) {
    let weak = main.downgrade();
    main.toast_marked(
        &cut_short_text(summary.operation),
        ToastLead::Warning,
        Some(&tr("Details…")),
        true,
        move || {
            if let Some(main) = weak.upgrade() {
                super::managed_cut_short::open(&main);
            }
        },
    );
}

/// Removes the toast of a run cut short, once the run is settled.
fn dismiss_cut_short(main: &MainWindow) {
    for operation in [ManagedOperation::Import, ManagedOperation::Consolidate] {
        main.dismiss_toast_if(&cut_short_text(operation));
    }
}

/// The query of the Tracks view that shows the files a run placed: the folders
/// its music went to.
pub(crate) fn tracks_query(report: &ManagedReport) -> Option<String> {
    let folders: BTreeSet<&Path> = report
        .outcomes
        .iter()
        .filter(|o| {
            o.kind == ItemKind::Audio
                && matches!(
                    o.result,
                    ItemResult::Copied | ItemResult::Moved | ItemResult::SourceKept(_)
                )
        })
        .filter_map(|o| o.to.parent())
        .collect();
    if folders.is_empty() {
        return None;
    }
    Some(
        folders
            .into_iter()
            .map(|folder| format!("folder={}", quote(&folder.to_string_lossy())))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

/// `text` in the quotes of the query language.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' if matches!(chars.peek(), None | Some('"' | '\\')) => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Opens the Tracks view with the files of `report`.
pub(crate) fn show_in_library(main: &MainWindow, report: &ManagedReport) {
    if let Some(query) = tracks_query(report) {
        main.show_tracks_matching(&query);
        main.present();
    }
}

/// The toast at the end of a run whose window is not open to tell it.
pub(crate) fn toast_done(main: &MainWindow) {
    let Some(run) = current() else { return };
    let Some(Ok(report)) = run.result.clone() else {
        return;
    };
    mark_told();
    if !report.finished || report.transferred() == 0 {
        return;
    }
    match run.work {
        Work::RollBack => {}
        _ => {
            let weak = main.downgrade();
            main.toast_marked(
                &done_text(run.operation, &report),
                ToastLead::Check,
                Some(&tr("Show")),
                false,
                move || {
                    if let Some(main) = weak.upgrade() {
                        show_in_library(&main, &report);
                    }
                },
            );
        }
    }
}

/// Starts `work` with `job` on a thread of its own; `total` is the number of
/// files when it is known before the first report.
fn begin(
    main: &MainWindow,
    view: RunView,
    job: impl FnOnce(
        &CancelToken,
        &(dyn Fn(usize, usize) + Sync),
    ) -> Result<ManagedReport, crate::backend::BackendError>
    + Send
    + 'static,
) -> bool {
    if is_working() {
        return false;
    }
    let Some(backend) = main.backend() else {
        return false;
    };
    let shared = hub();
    let cancel = CancelToken::new();
    shared.cancel.replace(Some(cancel.clone()));
    shared.run.replace(Some(view));
    shared.told.set(false);
    notify();
    let services = backend.services();
    let weak = main.downgrade();
    // The sizes of the files that move, in the order the run takes them.
    let sizes: Vec<u64> = shared
        .run
        .borrow()
        .as_ref()
        .and_then(|r| r.plan.clone())
        .map(|plan| {
            plan.items
                .iter()
                .filter(|i| i.transfers())
                .map(|i| i.size)
                .collect()
        })
        .unwrap_or_default();
    run_in_background(
        move |progress| {
            let report = |d: usize, t: usize| {
                let _ = progress.send_blocking((d, t));
            };
            job(&cancel, &report)
        },
        move |done, total| {
            let hub = hub();
            if let Some(run) = hub.run.borrow_mut().as_mut() {
                run.done = done;
                run.total = total;
                run.bytes = sizes.iter().take(done).sum();
            }
            notify();
        },
        move |result| {
            let hub = hub();
            hub.cancel.replace(None);
            let work = {
                let mut run = hub.run.borrow_mut();
                let Some(run) = run.as_mut() else { return };
                run.result = Some(result.map_err(|e| e.0));
                run.work
            };
            notify();
            let Some(main) = weak.upgrade() else { return };
            let finished = current().and_then(|r| r.result).map(|r| match r {
                Ok(report) => Ok(report.finished),
                Err(error) => Err(error),
            });
            let weak = main.downgrade();
            refresh(services, move || {
                let Some(main) = weak.upgrade() else { return };
                match finished {
                    Some(Ok(true)) => {
                        dismiss_cut_short(&main);
                        if work == Work::RollBack {
                            mark_told();
                            main.toast_marked(
                                &tr("The run was rolled back."),
                                ToastLead::Check,
                                None,
                                false,
                                || {},
                            );
                        } else if !super::managed_folder::is_open(&main) {
                            toast_done(&main);
                        }
                    }
                    Some(Ok(false)) => {
                        mark_told();
                        if let Some(summary) = interrupted() {
                            toast_cut_short(&main, &summary);
                        }
                    }
                    Some(Err(error)) => {
                        mark_told();
                        main.toast(&error, true);
                    }
                    None => {}
                }
            });
        },
    );
    true
}

/// Carries out `plan` in the background.
pub(crate) fn start(main: &MainWindow, plan: Arc<ManagedPlan>) -> bool {
    let Some(backend) = main.backend() else {
        return false;
    };
    let services = backend.services();
    let view = RunView {
        work: Work::Run,
        operation: plan.operation,
        transfer: plan.options.transfer,
        total: plan.to_transfer(),
        plan: Some(plan.clone()),
        done: 0,
        bytes: 0,
        result: None,
    };
    begin(main, view, move |cancel, progress| {
        services.apply_managed(&plan, progress, cancel)
    })
}

/// Goes on with the run that was cut short.
pub(crate) fn resume(main: &MainWindow) -> bool {
    let (Some(backend), Some(summary)) = (main.backend(), interrupted()) else {
        return false;
    };
    let services = backend.services();
    let view = RunView {
        work: Work::Resume,
        operation: summary.operation,
        transfer: summary.transfer,
        plan: None,
        done: 0,
        total: summary.waiting,
        bytes: 0,
        result: None,
    };
    let run = summary.run;
    begin(main, view, move |cancel, progress| {
        services.resume_managed_run(run, progress, cancel)
    })
}

/// Rolls back the run that was cut short.
pub(crate) fn roll_back(main: &MainWindow) -> bool {
    let (Some(backend), Some(summary)) = (main.backend(), interrupted()) else {
        return false;
    };
    let services = backend.services();
    let view = RunView {
        work: Work::RollBack,
        operation: summary.operation,
        transfer: summary.transfer,
        plan: None,
        done: 0,
        total: summary.done,
        bytes: 0,
        result: None,
    };
    let run = summary.run;
    begin(main, view, move |_, progress| {
        services.roll_back_managed_run(run, progress)
    })
}

/// "412 of 1,098 files".
pub(crate) fn progress_text(run: &RunView) -> String {
    fill(
        &ntr("{} of {} file", "{} of {} files", run.total),
        &[&format::count(run.done), &format::count(run.total)],
    )
}

/// What the run does, as the sidebar footer names it.
fn work_title(run: &RunView) -> String {
    match (run.work, run.operation) {
        (Work::RollBack, _) => tr("Rolling back"),
        (_, ManagedOperation::Consolidate) => tr("Gathering the library"),
        _ => tr("Adding music"),
    }
}

/// The sidebar footer of a run: a title with the share done, a bar, the files
/// placed and a stop button, as for a library scan.
struct RunFooter {
    root: gtk::Box,
    title: gtk::Label,
    share: gtk::Label,
    stop: gtk::Button,
    bar: gtk::ProgressBar,
    files: gtk::Label,
}

impl RunFooter {
    fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("hk-scan-footer");
        root.set_widget_name("managed-footer");
        root.set_visible(false);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = label("", &[]);
        title.set_hexpand(true);
        head.append(&title);
        let share = label("", &["hk-numeric", "hk-t2"]);
        head.append(&share);
        let stop = icon_button("process-stop-symbolic", &tr("Stop"));
        stop.add_css_class("hk-small-icon");
        stop.connect_clicked(|button| {
            button.set_sensitive(false);
            self::stop();
        });
        head.append(&stop);
        root.append(&head);
        let bar = gtk::ProgressBar::new();
        bar.add_css_class("hk-scan-bar");
        root.append(&bar);
        let files = label("", &["hk-mono-meta", "hk-t3"]);
        root.append(&files);
        Self {
            root,
            title,
            share,
            stop,
            bar,
            files,
        }
    }

    fn update(&self) {
        let Some(run) = current().filter(RunView::is_working) else {
            self.root.set_visible(false);
            return;
        };
        if !self.root.is_visible() {
            self.stop.set_sensitive(true);
            self.root.set_visible(true);
        }
        // Stop is for runs that place files; a roll back goes to its end.
        self.stop.set_visible(run.work != Work::RollBack);
        let title = work_title(&run);
        self.title.set_text(&title);
        self.bar
            .update_property(&[gtk::accessible::Property::Label(&title)]);
        let fraction = run.fraction();
        self.bar.set_fraction(fraction);
        let percent = (fraction * 100.0).floor() as u32;
        self.share
            .set_text(&fill(&tr("{}%"), &[&percent.to_string()]));
        self.files.set_text(&progress_text(&run));
    }
}

/// Puts the run's footer in the sidebar of `main`, and at start tells of a run
/// that was cut short.
pub(crate) fn attach(main: &MainWindow, sidebar: &gtk::Box, services: Arc<dyn Services>) {
    let footer = RunFooter::new();
    sidebar.append(&footer.root);
    // The listener keeps the footer for as long as the window lives.
    let id = listen(move || footer.update());
    main.connect_destroy(move |_| unlisten(id));
    let weak = main.downgrade();
    refresh(services, move || {
        if let Some(main) = weak.upgrade()
            && let Some(summary) = interrupted()
        {
            toast_cut_short(&main, &summary);
        }
    });
}

/// Turns the actions of the managed folder on while it is on.
pub(crate) fn sync_actions(application: &gtk::Application, on: bool) {
    for name in [ADD, GATHER] {
        if let Some(action) = application
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(on);
        }
    }
}

/// The action "Add Music to the Managed Folder…".
const ADD: &str = "managed-add";
/// The action "Gather into the Managed Folder…".
const GATHER: &str = "managed-gather";

/// Asks for folders of music, then opens the plan of adding them.
pub(crate) fn add_music(main: &MainWindow) {
    let dialog = gtk::FileDialog::builder()
        .title(tr("Add music to the managed folder"))
        .accept_label(tr("Add"))
        .modal(true)
        .build();
    let weak = main.downgrade();
    dialog.select_multiple_folders(Some(main), gio::Cancellable::NONE, move |files| {
        let Ok(files) = files else { return };
        let paths: Vec<PathBuf> = (0..files.n_items())
            .filter_map(|i| files.item(i).and_downcast::<gio::File>())
            .filter_map(|f| f.path())
            .collect();
        if let Some(main) = weak.upgrade()
            && !paths.is_empty()
        {
            super::managed_folder::open_add(&main, paths);
        }
    });
}

/// Adds the application actions of the managed folder, on while it is on.
pub(crate) fn install_actions(application: &gtk::Application, on: bool) {
    let main_window = |app: &glib::WeakRef<gtk::Application>| {
        app.upgrade()?
            .windows()
            .into_iter()
            .find_map(|w| w.downcast::<MainWindow>().ok())
    };
    let add = gio::SimpleAction::new(ADD, None);
    let app = application.downgrade();
    add.connect_activate(move |_, _| {
        if let Some(main) = main_window(&app) {
            add_music(&main);
        }
    });
    application.add_action(&add);
    let gather = gio::SimpleAction::new(GATHER, None);
    let app = application.downgrade();
    gather.connect_activate(move |_, _| {
        if let Some(main) = main_window(&app) {
            super::managed_folder::open_gather(&main);
        }
    });
    application.add_action(&gather);
    sync_actions(application, on);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::services::{ManagedOutcome, ManagedRunId};

    fn outcome(to: &str, kind: ItemKind, result: ItemResult) -> ManagedOutcome {
        ManagedOutcome {
            kind,
            file: None,
            from: PathBuf::from("/in/x"),
            to: PathBuf::from(to),
            result,
        }
    }

    #[test]
    fn the_tracks_of_a_run_are_found_by_their_folders() {
        let report = ManagedReport {
            run: Some(ManagedRunId::from_raw(1)),
            outcomes: vec![
                outcome(
                    "/m/Okto/Night Ferry/01.flac",
                    ItemKind::Audio,
                    ItemResult::Copied,
                ),
                outcome(
                    "/m/Okto/Night Ferry/02.flac",
                    ItemKind::Audio,
                    ItemResult::Moved,
                ),
                outcome(
                    "/m/Okto/Night Ferry/cover.jpg",
                    ItemKind::Cover,
                    ItemResult::Copied,
                ),
                outcome(
                    "/m/Say \"Hi\"/01.flac",
                    ItemKind::Audio,
                    ItemResult::SourceKept("read-only".into()),
                ),
                outcome(
                    "/m/Sola/Amber/01.flac",
                    ItemKind::Audio,
                    ItemResult::Waiting,
                ),
            ],
            finished: false,
            library_error: None,
        };
        assert_eq!(
            tracks_query(&report).unwrap(),
            r#"folder="/m/Okto/Night Ferry" OR folder="/m/Say \"Hi\"""#
        );
        assert_eq!(tracks_query(&ManagedReport::default()), None);
    }
}
