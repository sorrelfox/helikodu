// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Library: the library folders with their state, rescan and
//! remove; the names to skip; how folders are scanned; what is done with
//! byte-identical copies, applied to the library at once with its progress;
//! and the size limits of the caches and stores.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::{Context, page_body};
use crate::backend::Update;
use crate::backend::services::{FolderState, LibraryFolder, StorageUsage};
use crate::dialogs::common::{
    OWN_LABEL, bind_segmented, bytes, compact_button, divider, dropdown, note, on_selected, row,
    segmented, switch_row, tilde, with_note,
};
use crate::i18n::{fill, ntr, tr};
use crate::widgets::{icon_button, label};
use helikodu_model::settings::DuplicateScan;

/// Asks for a folder with the desktop's folder chooser and calls `chosen`.
pub(crate) fn choose_folder(parent: &gtk::Window, chosen: impl FnOnce(PathBuf) + 'static) {
    let dialog = gtk::FileDialog::builder()
        .title(tr("Add a music folder"))
        .accept_label(tr("Add"))
        .modal(true)
        .build();
    dialog.set_initial_folder(Some(&gio::File::for_path(glib::home_dir())));
    dialog.select_folder(Some(parent), gio::Cancellable::NONE, move |result| {
        if let Ok(path) = result.map(|f| f.path())
            && let Some(path) = path
        {
            chosen(path);
        }
    });
}

/// The line under a folder's path: how it is kept current and its tracks.
pub(crate) fn folder_line(folder: &LibraryFolder) -> String {
    let how = match folder.state {
        FolderState::Watched => tr("Watched for changes"),
        FolderState::Polled(minutes) => fill(
            &ntr(
                "Cannot be watched, so it is checked every {} minute",
                "Cannot be watched, so it is checked every {} minutes",
                minutes as usize,
            ),
            &[&minutes.to_string()],
        ),
        FolderState::Manual => tr("Scanned when you ask"),
        FolderState::NotConnected => tr("Not connected · its tracks are hidden until it is back"),
    };
    match folder.tracks {
        Some(n) if folder.state != FolderState::NotConnected => format!(
            "{how} · {}",
            fill(&ntr("{} track", "{} tracks", n), &[&n.to_string()])
        ),
        _ => how,
    }
}

/// The short state tag in capitals.
fn folder_tag(state: FolderState) -> String {
    match state {
        FolderState::Watched => tr("Watched"),
        FolderState::Polled(minutes) => fill(&tr("Every {} min"), &[&minutes.to_string()]),
        FolderState::Manual => tr("Manual"),
        FolderState::NotConnected => tr("Not connected"),
    }
    .to_uppercase()
}

fn folder_row(context: &Rc<Context>, folder: &LibraryFolder) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.add_css_class("hk-folder-row");
    if folder.state == FolderState::NotConnected {
        line.add_css_class("hk-dim");
    }
    let icon = if folder.state == FolderState::NotConnected {
        "drive-harddisk-symbolic"
    } else {
        "folder-symbolic"
    };
    let image = gtk::Image::from_icon_name(icon);
    image.add_css_class("hk-t2");
    line.append(&image);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let path = tilde(&folder.path);
    text.append(&label(&path, &["hk-mono", "hk-path"]));
    text.append(&label(&folder_line(folder), &["hk-t3", "hk-secondary"]));
    line.append(&text);
    let tag = label(
        &folder_tag(folder.state),
        &["hk-mono-meta", "hk-t2", "hk-tag"],
    );
    tag.set_ellipsize(gtk::pango::EllipsizeMode::None);
    line.append(&tag);
    let rescan = icon_button(
        "view-refresh-symbolic",
        &fill(&tr("Scan {} again"), &[&path]),
    );
    let target = folder.path.clone();
    let weak = Rc::downgrade(context);
    rescan.connect_clicked(move |_| {
        if let Some(context) = weak.upgrade()
            && let Err(error) = context.services.scan(std::slice::from_ref(&target))
        {
            context.toast(&error.0);
        }
    });
    line.append(&rescan);
    let remove = icon_button("window-close-symbolic", &fill(&tr("Remove {}"), &[&path]));
    let target = folder.path.clone();
    let weak = Rc::downgrade(context);
    remove.connect_clicked(move |_| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let removed = target.clone();
        if context.update(|s| s.library.folders.retain(|f| *f != removed)) {
            let undo_context = Rc::downgrade(&context);
            let back = target.clone();
            if let Some(main) = context.main.upgrade() {
                main.toast_undo(
                    &fill(&tr("{} is no longer in the library"), &[&tilde(&target)]),
                    move || {
                        if let Some(context) = undo_context.upgrade()
                            && context.update(|s| {
                                if !s.library.folders.contains(&back) {
                                    s.library.folders.push(back.clone());
                                }
                            })
                        {
                            context.rebuild();
                        }
                    },
                );
            }
            context.rebuild();
        }
    });
    line.append(&remove);
    line
}

fn folders(context: &Rc<Context>) -> gtk::Box {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("hk-bordered-list");
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows.add_css_class("hk-rows");
    list.append(&rows);
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    footer.add_css_class("hk-list-footer");
    let add = gtk::Button::new();
    add.add_css_class("hk-btn");
    add.add_css_class("text");
    let add_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    add_line.append(&gtk::Image::from_icon_name("list-add-symbolic"));
    add_line.append(&gtk::Label::new(Some(&tr("Add folder"))));
    add.set_child(Some(&add_line));
    add.set_hexpand(true);
    add.set_halign(gtk::Align::Start);
    let weak = Rc::downgrade(context);
    add.connect_clicked(move |_| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let weak = Rc::downgrade(&context);
        choose_folder(&context.window, move |path| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            let added = path.clone();
            if context.update(|s| {
                if !s.library.folders.contains(&added) {
                    s.library.folders.push(added.clone());
                }
            }) {
                if let Err(error) = context.services.scan(&[path]) {
                    context.toast(&error.0);
                }
                context.rebuild();
            }
        });
    });
    footer.append(&add);
    let rescan = compact_button(&tr("Rescan all"));
    let weak = Rc::downgrade(context);
    rescan.connect_clicked(move |_| {
        if let Some(context) = weak.upgrade()
            && let Err(error) = context.services.scan(&[])
        {
            context.toast(&error.0);
        }
    });
    footer.append(&rescan);
    list.append(&footer);

    let services = context.services.clone();
    let weak = Rc::downgrade(context);
    let rows_weak = rows.downgrade();
    glib::spawn_future_local(async move {
        let folders = gio::spawn_blocking(move || services.library_folders())
            .await
            .unwrap_or_else(|_| Ok(Vec::new()));
        let (Some(context), Some(rows)) = (weak.upgrade(), rows_weak.upgrade()) else {
            return;
        };
        match folders {
            Ok(folders) if folders.is_empty() => {
                let empty = label(&tr("No folders yet"), &["hk-t3", "hk-folder-row"]);
                rows.append(&empty);
            }
            Ok(folders) => {
                for folder in &folders {
                    rows.append(&folder_row(&context, folder));
                }
            }
            Err(error) => rows.append(&label(&error.0, &["hk-t3", "hk-folder-row"])),
        }
    });
    list
}

fn skip(context: &Rc<Context>) -> gtk::Box {
    let entry = gtk::Entry::new();
    entry.add_css_class("hk-input");
    entry.add_css_class("hk-mono");
    entry.set_text(&context.settings().library.skip.join("; "));
    entry.update_property(&[gtk::accessible::Property::Label(&tr("Skip"))]);
    let save = {
        let weak = Rc::downgrade(context);
        move |entry: &gtk::Entry| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            let patterns: Vec<String> = entry
                .text()
                .split(';')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect();
            if context.settings().library.skip != patterns {
                context.update(|s| s.library.skip.clone_from(&patterns));
            }
        }
    };
    let on_activate = save.clone();
    entry.connect_activate(move |entry| on_activate(entry));
    let focus = gtk::EventControllerFocus::new();
    let weak = entry.downgrade();
    focus.connect_leave(move |_| {
        if let Some(entry) = weak.upgrade() {
            save(&entry);
        }
    });
    entry.add_controller(focus);
    with_note(
        &entry,
        &tr(
            "Files and folders with these names are not scanned. Separate patterns with semicolons.",
        ),
    )
}

fn scanning(context: &Rc<Context>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let settings = context.settings();
    type Set = fn(&mut crate::backend::services::Settings, bool);
    let rows: [(String, String, bool, Set); 5] = [
        (
            tr("Watch the folders for changes"),
            tr("New and changed files appear on their own."),
            settings.library.watch,
            |s, on| s.library.watch = on,
        ),
        (
            tr("Follow symbolic links"),
            tr("Off: linked files and folders are skipped, so nothing is listed twice."),
            settings.library.follow_symlinks,
            |s, on| s.library.follow_symlinks = on,
        ),
        (
            tr("Include video files"),
            tr("Plays their sound only. No picture is ever shown."),
            settings.library.include_video,
            |s, on| s.library.include_video = on,
        ),
        (
            tr("Hide tracks on drives that are not connected"),
            tr("They return with their ratings and play counts when the drive is back."),
            settings.library.hide_missing_drives,
            |s, on| s.library.hide_missing_drives = on,
        ),
        (
            tr("Import playlist files found in the folders"),
            tr("M3U, PLS and XSPF files become playlists that follow their files."),
            settings.library.import_playlists,
            |s, on| s.library.import_playlists = on,
        ),
    ];
    for (title, explanation, value, set) in rows {
        let (line, switch) = switch_row(&title, Some(&explanation));
        switch.set_active(value);
        let weak = Rc::downgrade(context);
        switch.connect_active_notify(move |switch| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            let on = switch.is_active();
            if !context.update(|s| set(s, on)) {
                switch.set_active(!on);
            }
        });
        column.append(&line);
    }
    column
}

/// What is done with copies: added or skipped by the scan, and shown once.
/// A change is applied to the library at once; the line under the rows
/// follows it until the library reports it applied.
fn copies(context: &Rc<Context>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let settings = context.settings();
    let duplicates = settings.library.duplicates;

    let scan_line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    scan_line.add_css_class("hk-switch-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let title = gtk::Label::new(Some(&tr("Copies of the same file")));
    title.set_xalign(0.0);
    text.append(&title);
    text.append(&note(&tr(
        "Skipped copies get no tracks; one takes over if the main copy goes.",
    )));
    scan_line.append(&text);
    let (group, buttons) = segmented(&[tr("Add them"), tr("Skip them")]);
    group.set_widget_name("copies-on-scan");
    group.update_property(&[gtk::accessible::Property::Label(&tr(
        "Copies of the same file",
    ))]);
    if let [add, skip] = buttons.as_slice() {
        add.set_widget_name("copies-add");
        skip.set_widget_name("copies-skip");
    }
    scan_line.append(&group);
    column.append(&scan_line);

    let (show_line, show_one) = switch_row(
        &tr("Show one track for identical copies"),
        Some(&tr(
            "Lists, counts and search show only the main copy; a badge marks the track.",
        )),
    );
    show_one.set_widget_name("copies-show-one");
    show_one.set_active(duplicates.show_one);
    column.append(&show_line);
    let skipped_note = note(&tr(
        "Not used while copies are skipped: they have no tracks to show.",
    ));
    skipped_note.set_widget_name("copies-show-one-note");
    column.append(&skipped_note);

    let status = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    status.set_visible(false);
    let meter = gtk::LevelBar::new();
    meter.add_css_class("hk-meter");
    meter.set_size_request(180, -1);
    meter.set_valign(gtk::Align::Center);
    meter.update_property(&[gtk::accessible::Property::Label(&tr("Applying"))]);
    status.append(&meter);
    let status_text = label("", &["hk-t2", "hk-secondary"]);
    status_text.set_widget_name("copies-status");
    status_text.set_hexpand(true);
    status.append(&status_text);
    column.append(&status);

    let applying = Rc::new(Cell::new(false));
    let skip_chosen = duplicates.on_scan == DuplicateScan::Skip;
    let show_state = {
        let (show_line, skipped_note) = (show_line.downgrade(), skipped_note.downgrade());
        move |skipping: bool| {
            if let Some(line) = show_line.upgrade() {
                line.set_sensitive(!skipping);
            }
            if let Some(note) = skipped_note.upgrade() {
                note.set_visible(skipping);
            }
        }
    };
    show_state(skip_chosen);
    let started = {
        let (status, meter, text) = (
            status.downgrade(),
            meter.downgrade(),
            status_text.downgrade(),
        );
        let applying = applying.clone();
        move || {
            applying.set(true);
            if let (Some(status), Some(meter), Some(text)) =
                (status.upgrade(), meter.upgrade(), text.upgrade())
            {
                status.set_visible(true);
                meter.set_visible(true);
                meter.set_value(0.0);
                text.set_text(&tr("Applying to the library…"));
            }
        }
    };
    let weak = Rc::downgrade(context);
    let started_scan = started.clone();
    let show_scan = show_state.clone();
    bind_segmented(&buttons, usize::from(skip_chosen), move |index| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let on_scan = if index == 1 {
            DuplicateScan::Skip
        } else {
            DuplicateScan::Add
        };
        if context.settings().library.duplicates.on_scan == on_scan {
            return;
        }
        started_scan();
        if context.update(|s| s.library.duplicates.on_scan = on_scan) {
            show_scan(on_scan == DuplicateScan::Skip);
        }
    });
    let weak = Rc::downgrade(context);
    show_one.connect_active_notify(move |switch| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let on = switch.is_active();
        if context.settings().library.duplicates.show_one == on {
            return;
        }
        started();
        if !context.update(|s| s.library.duplicates.show_one = on) {
            switch.set_active(!on);
        }
    });
    // Follow the change as the library applies it.
    if let Some(playback) = &context.playback {
        let (status, meter, text) = (
            status.downgrade(),
            meter.downgrade(),
            status_text.downgrade(),
        );
        playback.connect_event(move |update| {
            let (Some(status), Some(meter), Some(text)) =
                (status.upgrade(), meter.upgrade(), text.upgrade())
            else {
                return;
            };
            match update {
                Update::Scan(scan) if applying.get() && !scan.finished => {
                    if let Some(fraction) = scan.fraction() {
                        meter.set_value(fraction);
                        let percent = (fraction * 100.0).floor() as u32;
                        text.set_text(&fill(
                            &tr("Applying to the library… {}%"),
                            &[&percent.to_string()],
                        ));
                    }
                }
                Update::DuplicatesApplied(result) => {
                    applying.set(false);
                    status.set_visible(true);
                    meter.set_visible(false);
                    text.set_text(&match result {
                        Ok(0) => tr("Applied to the library."),
                        Ok(read) => fill(
                            &ntr(
                                "Applied to the library: {} file read.",
                                "Applied to the library: {} files read.",
                                *read,
                            ),
                            &[&read.to_string()],
                        ),
                        Err(error) => fill(&tr("Not applied: {}"), &[error]),
                    });
                }
                _ => {}
            }
        });
    }
    column
}

/// The limits offered for a store, in MiB, and the one chosen.
fn limits(options: &[u32], value: u32) -> (Vec<u32>, usize) {
    let mut options = options.to_vec();
    if !options.contains(&value) {
        options.push(value);
        options.sort_unstable();
    }
    let index = options.iter().position(|&v| v == value).unwrap_or(0);
    (options, index)
}

fn storage(context: &Rc<Context>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let settings = context.settings();
    let storage = &settings.library.storage;
    type Set = fn(&mut crate::backend::services::Settings, u32);
    let stores: [(String, &[u32], u32, bool, Set); 4] = [
        (
            tr("Covers"),
            &[256, 512, 1024, 2048, 4096, 8192],
            storage.covers_mib,
            false,
            |s, v| s.library.storage.covers_mib = v,
        ),
        (
            tr("Analysis data"),
            &[256, 512, 1024, 2048, 4096],
            storage.analysis_mib,
            false,
            |s, v| s.library.storage.analysis_mib = v,
        ),
        (
            tr("Logs"),
            &[10, 50, 100, 500],
            storage.logs_mib,
            false,
            |s, v| s.library.storage.logs_mib = v,
        ),
        (
            tr("Undo history"),
            &[30, 90, 180, 365],
            storage.undo_days,
            true,
            |s, v| s.library.storage.undo_days = v,
        ),
    ];
    let mut meters = Vec::new();
    for (name, options, value, days, set) in stores {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let title = label(&name, &[]);
        title.set_size_request(148, -1);
        line.append(&title);
        let (meter, size) = if days {
            let text = label(&tr("Tag edits and file moves"), &["hk-t3", "hk-secondary"]);
            text.set_hexpand(true);
            line.append(&text);
            (None, label("", &["hk-mono", "hk-t2", "hk-secondary"]))
        } else {
            let meter = gtk::LevelBar::new();
            meter.add_css_class("hk-meter");
            meter.set_hexpand(true);
            meter.set_valign(gtk::Align::Center);
            meter.update_property(&[gtk::accessible::Property::Label(&name)]);
            line.append(&meter);
            (
                Some(meter),
                label("", &["hk-mono", "hk-t2", "hk-secondary"]),
            )
        };
        size.set_xalign(1.0);
        size.set_size_request(68, -1);
        line.append(&size);
        let (options, index) = limits(options, value);
        let items: Vec<String> = options
            .iter()
            .map(|&v| {
                if days {
                    fill(&ntr("{} day", "{} days", v as usize), &[&v.to_string()])
                } else {
                    bytes(u64::from(v) << 20)
                }
            })
            .collect();
        let choice = dropdown(&items, index, 112);
        choice.add_css_class(OWN_LABEL);
        choice.update_relation(&[gtk::accessible::Relation::LabelledBy(&[title.upcast_ref()])]);
        choice.update_property(&[gtk::accessible::Property::Description(&fill(
            &tr("Limit for {}"),
            &[&name],
        ))]);
        let weak = Rc::downgrade(context);
        on_selected(&choice, move |i| {
            if let (Some(context), Some(&v)) = (weak.upgrade(), options.get(i)) {
                context.update(|s| set(s, v));
            }
        });
        line.append(&choice);
        let clear = compact_button(&tr("Clear"));
        // Clearing the stores comes in a later version.
        clear.set_sensitive(false);
        line.append(&clear);
        column.append(&line);
        meters.push((meter, size, value, days));
    }
    let services = context.services.clone();
    glib::spawn_future_local(async move {
        let usage: StorageUsage = gio::spawn_blocking(move || services.storage())
            .await
            .unwrap_or_default();
        let used = [usage.covers, usage.analysis, usage.logs, usage.undo];
        for ((meter, size, limit, days), used) in meters.into_iter().zip(used) {
            let Some(used) = used else {
                size.set_text("–");
                continue;
            };
            size.set_text(&bytes(used));
            if let Some(meter) = meter
                && !days
            {
                let limit = (u64::from(limit) << 20).max(1);
                meter.set_value((used as f64 / limit as f64).clamp(0.0, 1.0));
            }
        }
    });
    column
}

/// The Library page.
pub(super) fn page(context: &Rc<Context>) -> gtk::Widget {
    let (scroller, body) = page_body();
    body.append(&row(
        &tr("Folders"),
        Some(&tr("Files are played where they are. Nothing is copied.")),
        &folders(context),
    ));
    body.append(&row(&tr("Skip"), None, &skip(context)));
    body.append(&row(&tr("Scanning"), None, &scanning(context)));
    body.append(&row(
        &tr("Copies"),
        Some(&tr("Files whose bytes are identical, tags included.")),
        &copies(context),
    ));
    body.append(&divider());
    body.append(&row(
        &tr("Storage"),
        Some(&tr("At a limit, the oldest items are removed first.")),
        &storage(context),
    ));
    scroller.upcast()
}
