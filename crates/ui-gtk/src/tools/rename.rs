// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Renaming one track's file, from the context menu: a small dialog with the
//! name (the part before the extension chosen), Cancel and Rename. The file
//! keeps its folder; a name that is taken is refused. The rename is a run of
//! the undo journal, so the toast that follows can undo it.

use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use super::file_tools::{run_in_background, toast_run};
use crate::backend::TrackKey;
use crate::dialogs::common::{dialog_window, note};
use crate::i18n::{fill, tr};
use crate::window::MainWindow;

/// The name the dialog is known by among the toplevels.
pub(crate) const NAME: &str = "rename-file";

/// Opens the dialog for the file `path` of `track`.
pub(crate) fn open(main: &MainWindow, track: TrackKey, path: PathBuf) -> gtk::Window {
    let window = dialog_window(
        Some(main.upcast_ref()),
        NAME,
        &tr("Rename the file"),
        (460, -1),
        true,
    );
    window.set_resizable(false);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.add_css_class("hk-dialog-body");
    let title = gtk::Label::new(Some(&tr("Rename the file")));
    title.set_xalign(0.0);
    title.add_css_class("hk-dialog-title");
    body.append(&title);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let entry = gtk::Entry::new();
    entry.add_css_class("hk-input");
    entry.add_css_class("hk-mono");
    entry.set_text(&name);
    entry.set_widget_name("rename-entry");
    entry.set_activates_default(true);
    entry.update_property(&[gtk::accessible::Property::Label(&tr("New file name"))]);
    body.append(&entry);
    let hint = note(&tr(
        "The file stays in its folder. The rename can be undone.",
    ));
    body.append(&hint);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.add_css_class("hk-dialog-buttons");
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.add_css_class("hk-btn");
    let rename = gtk::Button::with_label(&tr("Rename"));
    rename.add_css_class("hk-btn");
    rename.add_css_class("primary");
    rename.set_widget_name("rename-yes");
    buttons.append(&cancel);
    buttons.append(&rename);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&rename));

    let weak = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    let check = {
        let rename = rename.clone();
        let original = name.clone();
        move |entry: &gtk::Entry| {
            let text = entry.text();
            let valid = !text.trim().is_empty()
                && !text.contains('/')
                && text != "."
                && text != ".."
                && text.as_str() != original;
            rename.set_sensitive(valid);
        }
    };
    check(&entry);
    entry.connect_changed(check);
    let weak = window.downgrade();
    let main_weak = main.downgrade();
    let entry_for_click = entry.clone();
    rename.connect_clicked(move |_| {
        let (Some(window), Some(main)) = (weak.upgrade(), main_weak.upgrade()) else {
            return;
        };
        let Some(backend) = main.backend() else { return };
        let services = backend.services();
        let name = entry_for_click.text().trim().to_owned();
        window.close();
        let weak = main.downgrade();
        let for_toast = services.clone();
        run_in_background(
            move |_| services.rename_track_file(track, &name),
            |_, _| {},
            move |result| {
                let Some(main) = weak.upgrade() else { return };
                match result {
                    Ok(report) => match (report.run, report.problems().next()) {
                        (Some(run), None) => toast_run(
                            &main,
                            for_toast,
                            run,
                            &tr("File renamed"),
                            false,
                            Rc::new(|| {}),
                        ),
                        _ => main.toast(
                            &tr("The file was not renamed: a file with that name is already there, or it could not be moved."),
                            true,
                        ),
                    },
                    Err(error) => main.toast(
                        &fill(&tr("The file was not renamed: {}"), &[&error.0]),
                        true,
                    ),
                }
            },
        );
    });
    window.present();
    // Choose the name without its extension.
    let stem = path
        .file_stem()
        .map_or(0, |s| s.to_string_lossy().chars().count());
    let entry_weak = entry.downgrade();
    glib::idle_add_local_once(move || {
        if let Some(entry) = entry_weak.upgrade() {
            entry.grab_focus();
            entry.select_region(0, i32::try_from(stem).unwrap_or(-1));
        }
    });
    window
}
