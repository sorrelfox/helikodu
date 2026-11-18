// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the file tools share (Organize files, Missing files, Verify files): a
//! left-pane row with a mono second line, the footer of a tool at work (a meter
//! in the accent fill, the progress in words and "Stop"), work on a thread of
//! its own with its result back on the main loop, a folder chooser, paths shown
//! relative to their library folder, and the undo of a run of the journal.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::services::{FileRunId, RunReport, Services};
use crate::dialogs::common::compact_button;
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use crate::window::MainWindow;

/// A row of the left pane: the name over a line in mono 11 px, and a count at
/// the right. Returns the row, its name, its line and its count.
pub(crate) fn mono_row(name: &str, line: &str) -> (gtk::ListBoxRow, gtk::Label, gtk::Label) {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("two-line");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_valign(gtk::Align::Center);
    let title = label(name, &["hk-side-name"]);
    text.append(&title);
    let second = label(line, &["hk-t3", "hk-side-line", "hk-mono", "hk-side-mono"]);
    text.append(&second);
    row.set_child(Some(&text));
    row.update_property(&[gtk::accessible::Property::Label(name)]);
    (row, title, second)
}

/// A row of the left pane with a count at the right, 32 high.
pub(crate) fn count_row(name: &str) -> (gtk::ListBoxRow, gtk::Label) {
    let row = gtk::ListBoxRow::new();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let title = label(name, &["hk-side-name"]);
    title.set_hexpand(true);
    content.append(&title);
    let count = label("", &["hk-count"]);
    content.append(&count);
    row.set_child(Some(&content));
    row.update_property(&[gtk::accessible::Property::Label(name)]);
    (row, count)
}

/// The footer of a tool at work: a meter, the progress in words and "Stop".
pub(crate) struct WorkBar {
    pub(crate) root: gtk::Box,
    pub(crate) meter: gtk::ProgressBar,
    pub(crate) text: gtk::Label,
    pub(crate) stop: gtk::Button,
}

impl WorkBar {
    pub(crate) fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        root.add_css_class("hk-tool-footer");
        root.set_visible(false);
        let meter = gtk::ProgressBar::new();
        meter.add_css_class("hk-tool-meter");
        meter.set_size_request(180, -1);
        meter.set_valign(gtk::Align::Center);
        root.append(&meter);
        let text = label("", &["hk-t3", "hk-secondary"]);
        text.set_hexpand(true);
        root.append(&text);
        let stop = compact_button(&tr("Stop"));
        root.append(&stop);
        Self {
            root,
            meter,
            text,
            stop,
        }
    }

    /// Shows `done` of `total`, in words after `what`.
    pub(crate) fn show(&self, done: usize, total: usize) {
        let fraction = if total == 0 {
            0.0
        } else {
            done as f64 / total as f64
        };
        self.meter.set_fraction(fraction.clamp(0.0, 1.0));
        self.text.set_text(&fill(
            &tr("{} of {}"),
            &[&format::count(done), &format::count(total)],
        ));
    }
}

/// Runs `job` on a thread of its own and hands its result to `done` on the
/// main loop. `progress` receives what the job reports through its sender.
pub(crate) fn run_in_background<T: Send + 'static>(
    job: impl FnOnce(&async_channel::Sender<(usize, usize)>) -> T + Send + 'static,
    progress: impl Fn(usize, usize) + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let (report, reports) = async_channel::unbounded::<(usize, usize)>();
    let (finish, finished) = async_channel::bounded::<T>(1);
    let spawned = std::thread::Builder::new()
        .name("helikodu-file-tool".into())
        .spawn(move || {
            let result = job(&report);
            let _ = finish.send_blocking(result);
        });
    if spawned.is_err() {
        return;
    }
    glib::spawn_future_local(async move {
        while let Ok((d, t)) = reports.recv().await {
            progress(d, t);
        }
    });
    glib::spawn_future_local(async move {
        if let Ok(result) = finished.recv().await {
            done(result);
        }
    });
}

/// Asks for a folder, with `title` on the dialog and `accept` on its button.
pub(crate) fn choose_folder(
    parent: &gtk::Window,
    title: &str,
    accept: &str,
    start: Option<&Path>,
    chosen: impl FnOnce(PathBuf) + 'static,
) {
    let start = start.map_or_else(glib::home_dir, Path::to_path_buf);
    crate::chooser::select_folder(parent, title, Some(accept), Some(&start), move |path| {
        if let Some(path) = path {
            chosen(path);
        }
    });
}

/// `path` as the tables show it: relative to the library folder in `roots`
/// that holds it, else with the home folder as `~`.
pub(crate) fn relative(path: &Path, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .filter(|r| path.starts_with(r) && path != r.as_path())
        .max_by_key(|r| r.as_os_str().len())
        .and_then(|r| path.strip_prefix(r).ok())
        .map_or_else(
            || crate::dialogs::common::tilde(path),
            |p| p.to_string_lossy().into_owned(),
        )
}

/// "N files moved", with what stayed.
pub(crate) fn run_summary(report: &RunReport, undo: Option<bool>) -> String {
    let moved = report.moved();
    let stayed = report.outcomes.len() - moved;
    let moved_text = match undo {
        Some(true) => fill(
            &ntr("{} file moved back", "{} files moved back", moved),
            &[&format::count(moved)],
        ),
        Some(false) => fill(
            &ntr("{} file moved again", "{} files moved again", moved),
            &[&format::count(moved)],
        ),
        None => fill(
            &ntr("{} file moved", "{} files moved", moved),
            &[&format::count(moved)],
        ),
    };
    if stayed == 0 {
        return moved_text;
    }
    fill(
        &tr("{} · {}"),
        &[
            &moved_text,
            &fill(
                &ntr(
                    "{} file stayed where it was",
                    "{} files stayed where they were",
                    stayed,
                ),
                &[&format::count(stayed)],
            ),
        ],
    )
}

/// What happened to the files of a run that stayed: one line each.
pub(crate) fn problems_box(report: &RunReport, roots: &[PathBuf]) -> gtk::Box {
    use crate::backend::services::MoveProblem;
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.add_css_class("hk-trash-results");
    column.set_widget_name("run-results");
    column.append(&label(&run_summary(report, None), &["hk-medium"]));
    for outcome in report.problems() {
        let why = match outcome.problem.as_ref() {
            Some(MoveProblem::Gone) => tr("Left alone: the file is not where it was"),
            Some(MoveProblem::Changed) => tr("Left alone: the library has the file elsewhere now"),
            Some(MoveProblem::NameTaken) => {
                tr("Left alone: a file with that name is already there")
            }
            Some(MoveProblem::OutsideLibrary) => {
                tr("Left alone: the destination is not in a library folder")
            }
            Some(MoveProblem::Stopped) => tr("Not moved: stopped before this file"),
            Some(MoveProblem::Failed(reason)) => {
                fill(&tr("Left alone: it could not be moved ({})"), &[reason])
            }
            _ => tr("Left alone"),
        };
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let icon = gtk::Image::from_icon_name("helikodu-warning-symbolic");
        icon.add_css_class("hk-t1");
        line.append(&icon);
        let shown = relative(&outcome.from, roots);
        let path = label(&shown, &["hk-mono", "hk-tool-path"]);
        path.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        path.set_hexpand(true);
        path.set_max_width_chars(1);
        line.append(&path);
        let text = label(&why, &["hk-t2"]);
        text.set_xalign(1.0);
        line.append(&text);
        line.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{}: {}"),
            &[&shown, &why],
        ))]);
        column.append(&line);
    }
    column
}

/// After a run, an undo or a redo of `run`: a toast in the main window, whose
/// Undo undoes the run (or does it again after an undo). `then` runs on the
/// main loop once that is done.
pub(crate) fn toast_run(
    main: &MainWindow,
    services: Arc<dyn Services>,
    run: FileRunId,
    text: &str,
    redo: bool,
    then: Rc<dyn Fn()>,
) {
    let weak = main.downgrade();
    main.toast_undo(text, move || {
        let services = services.clone();
        let then = then.clone();
        let weak = weak.clone();
        let services_for_job = services.clone();
        run_in_background(
            move |progress| {
                let report = |d: usize, t: usize| {
                    let _ = progress.send_blocking((d, t));
                };
                services_for_job.step_file_run(run, redo, &report)
            },
            |_, _| {},
            move |result| {
                let Some(main) = weak.upgrade() else { return };
                match result {
                    Ok(report) => {
                        let text = run_summary(&report, Some(!redo));
                        toast_run(&main, services, run, &text, !redo, then.clone());
                    }
                    Err(error) => main.toast(&error.0, true),
                }
                then();
            },
        );
    });
}
