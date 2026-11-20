// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! "Folders that moved" of the Import window: a chip for each pair (the old
//! folder, an arrow, the new folder and a remove button) and the popover "A
//! folder that moved" with "Was" and "Is now", which adds a pair.
//!
//! Everything under the old folder is looked for under the new one; Windows
//! paths match without regard to capitals. A new folder that does not exist
//! is reported under its entry, and nothing is added.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use super::import_view::notice;
use crate::backend::interchange::PathMapping;
use crate::dialogs::common::{compact_button, tilde};
use crate::i18n::{fill, tr};
use crate::widgets::label;

/// The chip of a pair; `remove` runs when its remove button is used.
pub(super) fn chip(mapping: &PathMapping, remove: impl Fn() + 'static) -> gtk::Box {
    let chip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    chip.add_css_class("hk-import-chip");
    chip.set_valign(gtk::Align::Center);
    let now = tilde(&mapping.to);
    let text = fill(
        // Translators: a folder that moved: the old folder, an arrow, the new one.
        &tr("{} → {}"),
        &[&mapping.from, &now],
    );
    let words = label(&text, &[]);
    words.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    words.set_max_width_chars(48);
    words.set_tooltip_text(Some(&text));
    chip.append(&words);
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.set_tooltip_text(Some(&tr("Remove")));
    close.update_property(&[gtk::accessible::Property::Label(&fill(
        &tr("Remove {}"),
        &[&text],
    ))]);
    close.connect_clicked(move |_| remove());
    chip.append(&close);
    chip
}

/// Opens "A folder that moved" under `anchor` in `window`; `added` gets the
/// pair once "Add" is used with a new folder that exists.
pub(super) fn popover(
    anchor: &impl IsA<gtk::Widget>,
    window: &gtk::Window,
    added: impl Fn(PathMapping) + 'static,
) {
    let popover = gtk::Popover::new();
    popover.set_widget_name("import-moved-popover");
    popover.add_css_class("menu");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 10);
    column.add_css_class("hk-import-moved");
    column.set_size_request(384 - 24, -1);
    column.set_margin_start(10);
    column.set_margin_end(10);
    column.set_margin_top(10);
    column.set_margin_bottom(10);
    column.append(&label(&tr("A folder that moved"), &["hk-strong"]));

    let grid = gtk::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(8);
    let was_label = label(&tr("Was"), &["hk-t2"]);
    grid.attach(&was_label, 0, 0, 1, 1);
    let was = gtk::Entry::new();
    was.set_widget_name("moved-was");
    was.set_hexpand(true);
    was.set_placeholder_text(Some("C:\\Users\\me\\Music"));
    was.update_property(&[gtk::accessible::Property::Label(&tr("Was"))]);
    grid.attach(&was, 1, 0, 2, 1);
    let now_label = label(&tr("Is now"), &["hk-t2"]);
    grid.attach(&now_label, 0, 1, 1, 1);
    let now = gtk::Entry::new();
    now.set_widget_name("moved-now");
    now.set_hexpand(true);
    now.set_placeholder_text(Some(&tilde(&glib::home_dir().join("Music"))));
    now.update_property(&[gtk::accessible::Property::Label(&tr("Is now"))]);
    grid.attach(&now, 1, 1, 1, 1);
    let choose = compact_button(&tr("Choose…"));
    choose.set_widget_name("moved-choose");
    grid.attach(&choose, 2, 1, 1, 1);
    let (missing, missing_message, missing_line, _) = notice();
    missing.set_widget_name("moved-missing");
    missing.set_visible(false);
    grid.attach(&missing, 1, 2, 2, 1);
    column.append(&grid);

    let hint = label(
        &tr(
            "Everything under the old folder is looked for under the new one. Windows paths match without regard to capitals.",
        ),
        &["hk-t3", "hk-secondary"],
    );
    hint.set_wrap(true);
    hint.set_single_line_mode(false);
    hint.set_ellipsize(gtk::pango::EllipsizeMode::None);
    hint.set_max_width_chars(44);
    column.append(&hint);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = compact_button(&tr("Cancel"));
    buttons.append(&cancel);
    let add = gtk::Button::with_label(&tr("Add"));
    add.set_widget_name("moved-add");
    add.add_css_class("hk-btn");
    add.add_css_class("primary");
    add.add_css_class("compact");
    add.set_sensitive(false);
    buttons.append(&add);
    column.append(&buttons);
    popover.set_child(Some(&column));

    let filled = {
        let (was, now, add) = (was.clone(), now.clone(), add.clone());
        Rc::new(move || {
            add.set_sensitive(!was.text().trim().is_empty() && !now.text().trim().is_empty());
        })
    };
    for entry in [&was, &now] {
        let filled = filled.clone();
        let missing = missing.clone();
        entry.connect_changed(move |_| {
            missing.set_visible(false);
            filled();
        });
    }
    {
        let window = window.downgrade();
        let now = now.downgrade();
        choose.connect_clicked(move |_| {
            let (Some(window), Some(now)) = (window.upgrade(), now.upgrade()) else {
                return;
            };
            let start = expand(now.text().trim());
            crate::chooser::select_folder(
                &window,
                &tr("Where the music is now"),
                Some(&tr("Choose")),
                start.as_deref().filter(|p| p.is_dir()),
                move |path| {
                    if let Some(path) = path {
                        now.set_text(&path.to_string_lossy());
                    }
                },
            );
        });
    }
    {
        let popover = popover.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
        });
    }
    let added = Rc::new(added);
    let commit = {
        let popover = popover.downgrade();
        let (was, now) = (was.downgrade(), now.downgrade());
        let missing = missing.downgrade();
        Rc::new(move || {
            let (Some(popover), Some(was), Some(now), Some(missing)) = (
                popover.upgrade(),
                was.upgrade(),
                now.upgrade(),
                missing.upgrade(),
            ) else {
                return;
            };
            let from = was.text().trim().to_owned();
            let Some(to) = expand(now.text().trim()) else {
                return;
            };
            if from.is_empty() {
                return;
            }
            if !to.is_dir() {
                missing_message.set_text(&fill(&tr("There is no folder {}."), &[&tilde(&to)]));
                missing_line.set_text(&tr("Nothing was added."));
                missing.set_visible(true);
                return;
            }
            popover.popdown();
            added(PathMapping::new(from, to));
        })
    };
    {
        let commit = commit.clone();
        add.connect_clicked(move |_| commit());
    }
    for entry in [&was, &now] {
        let commit = commit.clone();
        let add = add.downgrade();
        entry.connect_activate(move |_| {
            if add.upgrade().is_some_and(|a| a.is_sensitive()) {
                commit();
            }
        });
    }
    popover.set_parent(anchor);
    popover.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
    was.grab_focus();
}

/// A typed folder: `~` as the home folder.
fn expand(text: &str) -> Option<PathBuf> {
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return Some(glib::home_dir().join(rest));
    }
    if text == "~" {
        return Some(glib::home_dir());
    }
    Some(Path::new(text).to_path_buf())
}
