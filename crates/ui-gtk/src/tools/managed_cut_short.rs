// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialog of a run of the managed folder that was cut short: by Stop, or by
//! closing Helikodu, a power cut or a pulled drive. It says what the run placed
//! and what waits, and offers "Continue" (places the rest) and "Roll back"
//! (removes the copies, or moves the files back). Closing it decides nothing.
//! While one of them works, both buttons wait and the foot shows the progress;
//! a roll back that cannot put a file back names it and leaves the run cut
//! short.
//!
//! It never opens on its own: the toast of a run cut short, Settings › Library
//! and the managed folder's window open it.

use std::rc::Rc;

use gtk::prelude::*;

use super::managed_runs::{self, RunView, Work};
use crate::backend::services::{ItemResult, ManagedReport, ManagedRunSummary, Transfer};
use crate::dialogs::common::{dialog_window, find_open, note};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the dialog is known by among the toplevels.
pub(crate) const NAME: &str = "managed-cut-short";

struct CutShort {
    window: gtk::Window,
    main: gtk::glib::WeakRef<MainWindow>,
    title: gtk::Label,
    body: gtk::Label,
    problems: gtk::Box,
    placed: gtk::Label,
    waiting: gtk::Label,
    when: gtk::Label,
    meter: gtk::ProgressBar,
    count: gtk::Label,
    roll_back: gtk::Button,
    resume: gtk::Button,
}

/// "Continue places the rest. Roll back removes the 37 copies; …".
fn body_text(summary: &ManagedRunSummary) -> String {
    let rest = tr("Continue places the rest.");
    let back = match summary.transfer {
        Transfer::Move => fill(
            &ntr(
                "Roll back removes the copies and moves the {} file back.",
                "Roll back removes the copies and moves the {} files back.",
                summary.done,
            ),
            &[&format::count(summary.done)],
        ),
        Transfer::Copy => fill(
            &ntr(
                "Roll back removes the {} copy; your own files are as they were.",
                "Roll back removes the {} copies; your own files are as they were.",
                summary.done,
            ),
            &[&format::count(summary.done)],
        ),
    };
    format!("{rest} {back}")
}

/// "Started on 20 November at 14:32.": when the run began, in local time.
fn started_text(seconds: i64) -> String {
    let Ok(time) = gtk::glib::DateTime::from_unix_local(seconds) else {
        return String::new();
    };
    let day = time
        .format(&tr("%-d %B"))
        .map(|d| d.to_string())
        .unwrap_or_default();
    let clock = time
        .format("%H:%M")
        .map(|t| t.to_string())
        .unwrap_or_default();
    fill(&tr("Started on {} at {}."), &[&day, &clock])
}

/// "37 of 214 files · 410 MB".
fn placed_text(summary: &ManagedRunSummary) -> String {
    let total = summary.done + summary.waiting;
    fill(
        &tr("{} · {}"),
        &[
            &fill(
                &ntr("{} of {} file", "{} of {} files", total),
                &[&format::count(summary.done), &format::count(total)],
            ),
            &format::size(summary.done_bytes),
        ],
    )
}

/// "177 files · 2.0 GB".
fn waiting_text(summary: &ManagedRunSummary) -> String {
    fill(
        &tr("{} · {}"),
        &[
            &fill(
                &ntr("{} file", "{} files", summary.waiting),
                &[&format::count(summary.waiting)],
            ),
            &format::size(summary.waiting_bytes),
        ],
    )
}

impl CutShort {
    /// Shows the run cut short as last read, the work on it, or how a roll
    /// back ended.
    fn update(self: &Rc<Self>) {
        let run = managed_runs::current();
        let working = run
            .as_ref()
            .is_some_and(|r| r.is_working() && matches!(r.work, Work::Resume | Work::RollBack));
        if let Some(summary) = managed_runs::interrupted() {
            self.title
                .set_text(&managed_runs::cut_short_text(summary.operation));
            self.body.set_text(&body_text(&summary));
            self.placed.set_text(&placed_text(&summary));
            self.waiting.set_text(&waiting_text(&summary));
            self.when.set_text(&format!(
                "{}\n{}",
                started_text(summary.started_at),
                tr("No new run can start until you choose.")
            ));
        } else if !working {
            // Settled: the dialog has nothing more to say.
            self.window.close();
            return;
        }
        self.roll_back.set_sensitive(!working);
        self.resume.set_sensitive(!working);
        self.when.set_visible(!working);
        self.meter.set_visible(working);
        self.count.set_visible(working);
        if let Some(run) = run.as_ref().filter(|_| working) {
            self.meter.set_fraction(run.fraction());
            self.count.set_text(&managed_runs::progress_text(run));
        }
        self.show_problems(run.as_ref());
    }

    /// The files a roll back could not put back, and why.
    fn show_problems(&self, run: Option<&RunView>) {
        while let Some(child) = self.problems.first_child() {
            self.problems.remove(&child);
        }
        let Some(RunView {
            work: Work::RollBack,
            result: Some(Ok(report)),
            ..
        }) = run
        else {
            self.problems.set_visible(false);
            return;
        };
        let lines = problem_lines(report);
        self.problems.set_visible(!lines.is_empty());
        for line in lines {
            let text = label(&line, &["hk-t1", "hk-managed-problem"]);
            text.set_wrap(true);
            text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            text.set_ellipsize(gtk::pango::EllipsizeMode::None);
            self.problems.append(&text);
        }
    }
}

/// "~/Music/Okto/01.flac: the reason" for each file a roll back left.
fn problem_lines(report: &ManagedReport) -> Vec<String> {
    report
        .outcomes
        .iter()
        .filter_map(|o| match &o.result {
            ItemResult::NotRolledBack(reason) => Some(fill(
                &tr("{} could not be put back: {}"),
                &[&crate::dialogs::common::tilde(&o.to), reason],
            )),
            _ => None,
        })
        .collect()
}

/// Opens the dialog over `main`, or brings it to the front.
pub(crate) fn open(main: &MainWindow) -> Option<gtk::Window> {
    let parent = main.upcast_ref::<gtk::Window>();
    if let Some(window) = find_open(parent, NAME) {
        window.present();
        return Some(window);
    }
    let window = dialog_window(
        Some(parent),
        NAME,
        &tr("A run was cut short"),
        (480, -1),
        true,
    );
    window.set_resizable(false);
    let body_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body_box.add_css_class("hk-dialog-body");
    body_box.add_css_class("hk-confirm");
    body_box.set_size_request(480, 300);

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tile.add_css_class("hk-icon-tile");
    tile.set_valign(gtk::Align::Start);
    let icon = gtk::Image::from_icon_name("folder-symbolic");
    icon.set_vexpand(true);
    icon.set_valign(gtk::Align::Center);
    tile.append(&icon);
    top.append(&tile);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 7);
    text.set_hexpand(true);
    let title = gtk::Label::new(None);
    title.set_xalign(0.0);
    title.set_wrap(true);
    title.set_max_width_chars(36);
    title.add_css_class("hk-dialog-title");
    title.set_accessible_role(gtk::AccessibleRole::Heading);
    text.append(&title);
    let body = gtk::Label::new(None);
    body.set_xalign(0.0);
    body.set_wrap(true);
    body.set_max_width_chars(46);
    body.add_css_class("hk-t2");
    body.add_css_class("hk-confirm-text");
    text.append(&body);
    let problems = gtk::Box::new(gtk::Orientation::Vertical, 4);
    problems.set_widget_name("roll-back-problems");
    problems.set_visible(false);
    text.append(&problems);
    top.append(&text);
    body_box.append(&top);

    let readout = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    readout.add_css_class("hk-readout");
    readout.add_css_class("hk-glass");
    readout.add_css_class("hk-confirm-readout");
    let column = |caption: &str| {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 3);
        column.set_hexpand(true);
        column.set_homogeneous(false);
        column.append(&label(caption, &["hk-micro-label"]));
        let value = label("", &["hk-mono", "hk-confirm-value"]);
        column.append(&value);
        readout.append(&column);
        value
    };
    let placed = column(&tr("Placed"));
    placed.set_widget_name("cut-short-placed");
    let waiting = column(&tr("Waiting"));
    waiting.set_widget_name("cut-short-waiting");
    body_box.append(&readout);
    let space = gtk::Box::new(gtk::Orientation::Vertical, 0);
    space.set_vexpand(true);
    body_box.append(&space);

    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bottom.add_css_class("hk-dialog-buttons");
    let when = note("");
    when.set_hexpand(true);
    when.set_valign(gtk::Align::Center);
    bottom.append(&when);
    let meter = gtk::ProgressBar::new();
    meter.add_css_class("hk-tool-meter");
    meter.set_size_request(120, -1);
    meter.set_valign(gtk::Align::Center);
    meter.set_visible(false);
    bottom.append(&meter);
    let count = label("", &["hk-t3", "hk-secondary"]);
    count.set_hexpand(true);
    count.set_visible(false);
    bottom.append(&count);
    let roll_back = gtk::Button::with_label(&tr("Roll back"));
    roll_back.add_css_class("hk-btn");
    roll_back.set_valign(gtk::Align::Center);
    roll_back.set_widget_name("roll-back");
    let resume = gtk::Button::with_label(&tr("Continue"));
    resume.add_css_class("hk-btn");
    resume.add_css_class("primary");
    resume.set_valign(gtk::Align::Center);
    resume.set_widget_name("continue");
    bottom.append(&roll_back);
    bottom.append(&resume);
    body_box.append(&bottom);
    window.set_child(Some(&body_box));
    window.set_default_widget(Some(&resume));

    let dialog = Rc::new(CutShort {
        window: window.clone(),
        main: main.downgrade(),
        title,
        body,
        problems,
        placed,
        waiting,
        when,
        meter,
        count,
        roll_back: roll_back.clone(),
        resume: resume.clone(),
    });
    let weak = Rc::downgrade(&dialog);
    resume.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade()
            && let Some(main) = this.main.upgrade()
        {
            managed_runs::resume(&main);
        }
    });
    let weak = Rc::downgrade(&dialog);
    roll_back.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade()
            && let Some(main) = this.main.upgrade()
        {
            managed_runs::roll_back(&main);
        }
    });
    let weak = Rc::downgrade(&dialog);
    let id = managed_runs::listen(move || {
        if let Some(this) = weak.upgrade() {
            this.update();
        }
    });
    let keep = std::cell::RefCell::new(Some(dialog.clone()));
    window.connect_destroy(move |_| {
        managed_runs::unlisten(id);
        keep.borrow_mut().take();
    });
    if managed_runs::interrupted().is_some() || managed_runs::is_working() {
        dialog.update();
    } else if let Some(backend) = main.backend() {
        // Not read yet: the listener shows it, or closes the dialog when there
        // is nothing to settle.
        managed_runs::refresh(backend.services(), || {});
    }
    window.present();
    resume.grab_focus();
    Some(window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::services::{ManagedOperation, ManagedRunId};
    use std::path::PathBuf;

    fn summary(transfer: Transfer) -> ManagedRunSummary {
        ManagedRunSummary {
            run: ManagedRunId::from_raw(3),
            operation: ManagedOperation::Import,
            transfer,
            started_at: 0,
            folder: PathBuf::from("/music"),
            done: 37,
            waiting: 177,
            skipped: 9,
            done_bytes: 410_000_000,
            waiting_bytes: 2_000_000_000,
        }
    }

    #[test]
    fn the_dialog_says_what_each_button_does() {
        assert_eq!(
            body_text(&summary(Transfer::Copy)),
            "Continue places the rest. Roll back removes the 37 copies; your own files are as they were."
        );
        assert_eq!(
            body_text(&summary(Transfer::Move)),
            "Continue places the rest. Roll back removes the copies and moves the 37 files back."
        );
        assert_eq!(
            placed_text(&summary(Transfer::Copy)).replace('\u{a0}', " "),
            "37 of 214 files · 410.0 MB"
        );
        assert_eq!(
            waiting_text(&summary(Transfer::Copy)).replace('\u{a0}', " "),
            "177 files · 2.0 GB"
        );
    }
}
