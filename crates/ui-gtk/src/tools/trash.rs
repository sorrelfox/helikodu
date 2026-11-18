// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Moving files to the desktop trash: the confirmation that lists the files
//! and says where they go (Cancel is the default and Escape cancels), and the
//! result of each file afterwards. A file that changed, that is the same file
//! as another path, or that no trash took is named with the reason and stays
//! where it was.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use super::tool_window::shown_path;
use crate::backend::services::{TrashProblem, TrashResult};
use crate::dialogs::common::{dialog_window, note};
use crate::format;
use crate::i18n::{fill, ntr, tr};
use crate::widgets::label;

/// The name the confirmation is known by among the toplevels.
pub(crate) const CONFIRM: &str = "trash-confirm";

/// The answer to the confirmation, taken by whichever comes first: a button
/// or closing the dialog.
type Answer = Rc<RefCell<Option<Box<dyn FnOnce(bool)>>>>;

/// "Move N files to the trash".
pub(crate) fn move_label(count: usize) -> String {
    fill(
        &ntr(
            "Move {} file to the trash",
            "Move {} files to the trash",
            count,
        ),
        &[&format::count(count)],
    )
}

/// Asks whether `paths` (freeing `freed` bytes) may go to the desktop trash.
/// `answer` gets the choice; closing the dialog cancels.
pub(crate) fn confirm(
    parent: &gtk::Window,
    paths: &[String],
    freed: u64,
    answer: impl FnOnce(bool) + 'static,
) -> gtk::Window {
    let count = paths.len();
    let question = fill(
        &ntr(
            "Move {} file to the trash?",
            "Move {} files to the trash?",
            count,
        ),
        &[&format::count(count)],
    );
    let window = dialog_window(Some(parent), CONFIRM, &question, (520, -1), true);
    window.set_resizable(false);
    let answer: Answer = Rc::new(RefCell::new(Some(Box::new(answer))));

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("hk-dialog-body");
    body.add_css_class("hk-confirm");
    body.set_size_request(520, -1);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tile.add_css_class("hk-icon-tile");
    tile.set_valign(gtk::Align::Start);
    let icon = gtk::Image::from_icon_name("user-trash-symbolic");
    icon.set_vexpand(true);
    icon.set_valign(gtk::Align::Center);
    tile.append(&icon);
    top.append(&tile);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 7);
    text.set_hexpand(true);
    let title = gtk::Label::new(Some(&question));
    title.set_xalign(0.0);
    title.add_css_class("hk-dialog-title");
    text.append(&title);
    let explanation = gtk::Label::new(Some(&fill(
        &tr(
            "They go to the desktop Trash, where you can restore them; {} will be freed when the Trash is emptied. Each file is checked again just before it moves, and one that changed stays where it is.",
        ),
        &[&format::size(freed)],
    )));
    explanation.set_xalign(0.0);
    explanation.set_wrap(true);
    explanation.set_max_width_chars(52);
    explanation.add_css_class("hk-t2");
    text.append(&explanation);
    top.append(&text);
    body.append(&top);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("hk-trash-list");
    for path in paths {
        let line = label(path, &["hk-mono", "hk-tool-path"]);
        line.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        line.set_tooltip_text(Some(path));
        list.append(&line);
    }
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_propagate_natural_height(true);
    scroller.set_max_content_height(180);
    scroller.set_child(Some(&list));
    scroller.add_css_class("hk-trash-files");
    scroller.update_property(&[gtk::accessible::Property::Label(&tr("Files to move"))]);
    body.append(&scroller);

    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bottom.add_css_class("hk-dialog-buttons");
    let hint = note(&tr("Are you sure?"));
    hint.set_hexpand(true);
    hint.set_valign(gtk::Align::Center);
    bottom.append(&hint);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.add_css_class("hk-btn");
    cancel.set_valign(gtk::Align::Center);
    let go = gtk::Button::with_label(&tr("Move to the trash"));
    go.add_css_class("hk-btn");
    go.set_valign(gtk::Align::Center);
    go.set_widget_name("trash-confirm-yes");
    bottom.append(&cancel);
    bottom.append(&go);
    body.append(&bottom);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&cancel));

    for (button, yes) in [(&cancel, false), (&go, true)] {
        let answer = answer.clone();
        let weak = window.downgrade();
        button.connect_clicked(move |_| {
            if let Some(answer) = answer.borrow_mut().take() {
                answer(yes);
            }
            if let Some(window) = weak.upgrade() {
                window.close();
            }
        });
    }
    window.connect_close_request(move |_| {
        if let Some(answer) = answer.borrow_mut().take() {
            answer(false);
        }
        gtk::glib::Propagation::Proceed
    });
    window.present();
    cancel.grab_focus();
    window.set_focus_visible(true);
    window
}

/// What happened to a file, in words.
pub(crate) fn outcome_text(problem: Option<&TrashProblem>) -> String {
    match problem {
        None => tr("Moved to the trash"),
        Some(TrashProblem::Changed) => {
            tr("Left in place: it changed, or is no longer identical to the main copy")
        }
        Some(TrashProblem::SameFile) => {
            tr("Left in place: it is the same file as another path in the library, not a copy")
        }
        Some(TrashProblem::MainCopy) => tr("Left in place: it is the main copy"),
        Some(TrashProblem::Missing) => tr("Not moved: the file is missing"),
        Some(TrashProblem::Failed(reason)) => fill(
            &tr("Left in place: it could not be moved to the trash ({})"),
            &[reason],
        ),
    }
}

/// The summary of `results`: how many went, how many stayed.
pub(crate) fn summary(results: &[TrashResult]) -> String {
    let moved = results.iter().filter(|r| r.problem.is_none()).count();
    let stayed = results.len() - moved;
    let moved_text = fill(
        &ntr(
            "{} file moved to the trash",
            "{} files moved to the trash",
            moved,
        ),
        &[&format::count(moved)],
    );
    if stayed == 0 {
        return moved_text;
    }
    fill(
        &tr("{} · {}"),
        &[
            &moved_text,
            &fill(
                &ntr("{} file left in place", "{} files left in place", stayed),
                &[&format::count(stayed)],
            ),
        ],
    )
}

/// The results of a move, one line a file: a mark, the path and what happened.
/// With `problems_only`, the files that went are counted in the summary only.
pub(crate) fn results_box(results: &[TrashResult], problems_only: bool) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.add_css_class("hk-trash-results");
    column.set_widget_name("trash-results");
    let head = label(&summary(results), &["hk-medium"]);
    column.append(&head);
    for result in results {
        if problems_only && result.problem.is_none() {
            continue;
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let icon = gtk::Image::from_icon_name(if result.problem.is_none() {
            "helikodu-check-symbolic"
        } else {
            "dialog-warning-symbolic"
        });
        icon.add_css_class(if result.problem.is_none() {
            "hk-t3"
        } else {
            "hk-t1"
        });
        line.append(&icon);
        let path = result
            .path
            .as_deref()
            .map(shown_path)
            .unwrap_or_else(|| result.file.to_string());
        let shown = label(&path, &["hk-mono", "hk-tool-path"]);
        shown.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        shown.set_hexpand(true);
        shown.set_max_width_chars(1);
        line.append(&shown);
        let what = outcome_text(result.problem.as_ref());
        let text = label(
            &what,
            if result.problem.is_none() {
                &["hk-t3"][..]
            } else {
                &["hk-t2"][..]
            },
        );
        text.set_xalign(1.0);
        line.append(&text);
        line.update_property(&[gtk::accessible::Property::Label(&fill(
            &tr("{}: {}"),
            &[&path, &what],
        ))]);
        column.append(&line);
    }
    column
}
