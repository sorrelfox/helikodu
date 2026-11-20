// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings › Advanced: the ID3 version of new tags, in-place writing of
//! very large files, ratings and play counts in files, the library
//! database, loading each track into memory for slow drives, the log detail
//! and the diagnostics, developer mode, and putting every setting back to its
//! default. The library database row holds its backups: a daily one, "Back up
//! now" and "Restore…", which puts a backup back at the next start.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_model::settings::{Id3Version, LogDetail};

use super::{Context, page_body};
use crate::backend::BackendError;
use crate::backend::services::BackupsInfo;
use crate::dialogs::common::{
    bind_segmented, bytes, compact_button, copy_text, dialog_window, divider, dropdown, note,
    on_selected, row, segmented, switch_row, tilde, with_note,
};
use crate::dialogs::maintenance;
use crate::i18n::{fill, tr};
use crate::widgets::label;

fn id3(context: &Rc<Context>) -> gtk::Box {
    let (group, buttons) = segmented(&["ID3v2.3".to_owned(), "ID3v2.4".to_owned()]);
    let current = context.settings().advanced.id3_version;
    let weak = Rc::downgrade(context);
    bind_segmented(
        &buttons,
        usize::from(current == Id3Version::V2_4),
        move |i| {
            if let Some(context) = weak.upgrade() {
                context.update(|s| {
                    s.advanced.id3_version = if i == 1 {
                        Id3Version::V2_4
                    } else {
                        Id3Version::V2_3
                    };
                });
            }
        },
    );
    with_note(
        &group,
        &tr(
            "ID3v2.4 is the current version. Choose ID3v2.3 if an old car stereo or portable player shows no tags. Tag fields that Helikodu does not know are kept as they are.",
        ),
    )
}

fn large_files(context: &Rc<Context>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let current = context.settings().advanced.in_place_tag_write_above_mib;
    let mut options = vec![512u32, 1024, 2048, 4096, 8192];
    if !options.contains(&current) {
        options.push(current);
        options.sort_unstable();
    }
    let items: Vec<String> = options.iter().map(|&m| bytes(u64::from(m) << 20)).collect();
    let index = options.iter().position(|&m| m == current).unwrap_or(0);
    let choice = dropdown(&items, index, 104);
    // Translators: {} is a dropdown with file sizes ("1 GB").
    let pattern = tr("Change tags in place for files over {}");
    choice.update_property(&[gtk::accessible::Property::Label(
        crate::i18n::fill(&pattern, &[""]).trim(),
    )]);
    let weak = Rc::downgrade(context);
    on_selected(&choice, move |i| {
        if let (Some(context), Some(&mib)) = (weak.upgrade(), options.get(i)) {
            context.update(|s| s.advanced.in_place_tag_write_above_mib = mib);
        }
    });
    crate::widgets::sentence(&line, &pattern, &[choice.upcast_ref()], &[]);
    with_note(
        &line,
        &tr(
            "Smaller files are changed through a checked copy, which is safe even if the power fails. A change in place is faster for huge files but has no such safety net. The tag editor tells you before it happens.",
        ),
    )
}

/// Ratings and play counts also written into the files.
fn statistics(context: &Rc<Context>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let advanced = context.settings().advanced;
    type Set = fn(&mut crate::backend::services::Settings, bool);
    let rows: [(String, String, bool, &str, Set); 2] = [
        (
            tr("Also write ratings to the tags of the files"),
            tr("Other players can then read them. Off: ratings stay in the library only."),
            advanced.write_ratings_to_tags,
            "write-ratings",
            |s, on| s.advanced.write_ratings_to_tags = on,
        ),
        (
            tr("Also write play counts to the tags of the files"),
            tr(
                "Written in the background a few minutes after the last play, never into the file that is playing. These writes are not on the Undo history.",
            ),
            advanced.write_play_counts_to_tags,
            "write-play-counts",
            |s, on| s.advanced.write_play_counts_to_tags = on,
        ),
    ];
    for (title, explanation, value, name, set) in rows {
        let (line, switch) = switch_row(&title, Some(&explanation));
        switch.set_active(value);
        switch.set_widget_name(name);
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

fn database(context: &Rc<Context>) -> gtk::Box {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("hk-bordered-list");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.add_css_class("hk-folder-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    match context.services.database() {
        Some(info) => {
            let backups = context.services.backups();
            text.append(&label(&tilde(&info.path), &["hk-mono", "hk-path"]));
            let facts = label(
                &maintenance::check_line(info.size, backups.as_ref()),
                &["hk-t3", "hk-secondary"],
            );
            facts.set_widget_name("database-facts");
            text.append(&facts);
            let open = compact_button(&tr("Open folder"));
            let path = info.path.clone();
            let weak = Rc::downgrade(context);
            open.connect_clicked(move |_| {
                if let Some(context) = weak.upgrade() {
                    show_in_folder(&context, path.clone());
                }
            });
            text.set_hexpand(true);
            line.append(&text);
            line.append(&open);
            list.append(&line);
            if let Some(backups) = backups {
                list.append(&backups_row(context, &backups, &facts, info.size));
            }
            list.append(&super::your_data::row(context));
        }
        None => {
            text.append(&label(&tr("Helikodu runs without a library."), &["hk-t3"]));
            line.append(&text);
            list.append(&line);
        }
    }
    list
}

/// Shows `path` in the file manager.
fn show_in_folder(context: &Rc<Context>, path: std::path::PathBuf) {
    if let Some(main) = context.main.upgrade() {
        crate::views::actions::open_folder(&main, Some(path));
    }
}

/// "Backups": the rotation and the newest one, "Back up now" and "Restore…".
fn backups_row(
    context: &Rc<Context>,
    info: &BackupsInfo,
    facts: &gtk::Label,
    size: Option<u64>,
) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line.add_css_class("hk-folder-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&label(&tr("Backups"), &[]));
    let status = label(&maintenance::backups_line(info), &["hk-t3", "hk-secondary"]);
    status.set_widget_name("backups-status");
    text.append(&status);
    line.append(&text);
    let back_up = compact_button(&tr("Back up now"));
    back_up.set_widget_name("back-up-now");
    let weak = Rc::downgrade(context);
    let status_weak = status.downgrade();
    let facts_weak = facts.downgrade();
    back_up.connect_clicked(move |button| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        button.set_sensitive(false);
        let services = context.services.clone();
        let (weak, status_weak, facts_weak) =
            (weak.clone(), status_weak.clone(), facts_weak.clone());
        let button = button.downgrade();
        glib::spawn_future_local(async move {
            let made = gio::spawn_blocking(move || services.back_up_now())
                .await
                .unwrap_or_else(|_| Err(BackendError(tr("The backup could not be made."))));
            if let Some(button) = button.upgrade() {
                button.set_sensitive(true);
            }
            let Some(context) = weak.upgrade() else {
                return;
            };
            match made {
                Ok(info) => {
                    if let Some(status) = status_weak.upgrade() {
                        status.set_text(&maintenance::backups_line(&info));
                    }
                    if let Some(facts) = facts_weak.upgrade() {
                        facts.set_text(&maintenance::check_line(size, Some(&info)));
                    }
                    context.toast(&tr("A backup of the library was made."));
                }
                Err(error) => context.toast(&error.0),
            }
        });
    });
    line.append(&back_up);
    let restore = compact_button(&tr("Restore…"));
    restore.set_widget_name("restore-backup");
    let folder = info.folder.clone();
    let weak = Rc::downgrade(context);
    restore.connect_clicked(move |_| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let weak = weak.clone();
        maintenance::choose_backup(&context.window, &folder, move |backup| {
            let Some(context) = weak.upgrade() else {
                return;
            };
            let services = context.services.clone();
            let weak = weak.clone();
            glib::spawn_future_local(async move {
                let staged = gio::spawn_blocking(move || services.stage_restore(&backup))
                    .await
                    .unwrap_or_else(|_| Err(BackendError(tr("The backup could not be checked."))));
                let Some(context) = weak.upgrade() else {
                    return;
                };
                match staged {
                    Ok(()) => maintenance::restart(&context.window),
                    Err(error) => context.toast(&error.0),
                }
            });
        });
    });
    line.append(&restore);
    line
}

fn logs(context: &Rc<Context>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let current = context.settings().advanced.log_detail;
    let choice = dropdown(
        &[
            tr("No logs"),
            tr("Normal detail"),
            tr("Full detail, for a bug report"),
        ],
        match current {
            LogDetail::Off => 0,
            LogDetail::Normal => 1,
            LogDetail::Full => 2,
        },
        240,
    );
    choice.update_property(&[gtk::accessible::Property::Label(&tr("Logs"))]);
    let weak = Rc::downgrade(context);
    on_selected(&choice, move |i| {
        if let Some(context) = weak.upgrade() {
            context.update(|s| {
                s.advanced.log_detail = match i {
                    0 => LogDetail::Off,
                    2 => LogDetail::Full,
                    _ => LogDetail::Normal,
                };
            });
        }
    });
    line.append(&choice);
    let space = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    space.set_hexpand(true);
    line.append(&space);
    let open = compact_button(&tr("Open log folder"));
    open.set_widget_name("open-log-folder");
    let weak = Rc::downgrade(context);
    open.connect_clicked(move |_| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let Some(folder) = context.services.log_folder() else {
            return;
        };
        let current = folder.join(helikodu_service::logging::LOG_FILE);
        if current.is_file() {
            show_in_folder(&context, current);
        } else if folder.is_dir() {
            show_in_folder(&context, folder);
        } else {
            context.toast(&tr("There are no logs yet."));
        }
    });
    line.append(&open);
    let copy = compact_button(&tr("Copy diagnostics"));
    let weak = Rc::downgrade(context);
    copy.connect_clicked(move |button| {
        if let Some(context) = weak.upgrade() {
            copy_text(
                button,
                &crate::dialogs::about::diagnostics(&context.backend),
            );
            context.toast(&tr("The diagnostics are on the clipboard."));
        }
    });
    line.append(&copy);
    with_note(
        &line,
        &tr("Logs stay on this computer. Helikodu never sends anything anywhere."),
    )
}

fn developer(context: &Rc<Context>) -> gtk::Box {
    let (line, switch) = switch_row(
        &tr("Show tools for testing"),
        Some(&tr("Adds the silent and WAV-file outputs.")),
    );
    switch.set_active(context.settings().advanced.developer_mode);
    let weak = Rc::downgrade(context);
    switch.connect_active_notify(move |switch| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let on = switch.is_active();
        if !context.update(|s| s.advanced.developer_mode = on) {
            switch.set_active(!on);
        }
    });
    line
}

/// Asks before putting every setting back, then does it.
fn confirm_reset(context: &Rc<Context>) {
    let window = dialog_window(
        Some(&context.window),
        "reset-settings",
        &tr("Reset every setting?"),
        (480, -1),
        true,
    );
    window.set_resizable(false);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("hk-dialog-body");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tile.add_css_class("hk-icon-tile");
    tile.set_valign(gtk::Align::Start);
    let icon = gtk::Image::from_icon_name("edit-undo-symbolic");
    icon.set_vexpand(true);
    icon.set_valign(gtk::Align::Center);
    tile.append(&icon);
    top.append(&tile);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let question = label(&tr("Reset every setting?"), &["hk-dialog-title"]);
    text.append(&question);
    let sentence = note(&tr(
        "Every setting goes back to its default. This cannot be undone. The library folders, playlists, ratings and play counts are kept.",
    ));
    sentence.remove_css_class("hk-note");
    sentence.add_css_class("hk-t2");
    sentence.set_max_width_chars(46);
    text.append(&sentence);
    top.append(&text);
    body.append(&top);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.add_css_class("hk-dialog-buttons");
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.add_css_class("hk-btn");
    let reset = gtk::Button::with_label(&tr("Reset settings"));
    reset.add_css_class("hk-btn");
    buttons.append(&cancel);
    buttons.append(&reset);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&cancel));
    let weak = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    let weak = window.downgrade();
    let context_weak = Rc::downgrade(context);
    reset.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
        let Some(context) = context_weak.upgrade() else {
            return;
        };
        if context.update(|s| {
            let folders = std::mem::take(&mut s.library.folders);
            *s = crate::backend::services::Settings::default();
            s.library.folders = folders;
        }) {
            super::appearance::apply_look(&context.settings());
            context.rebuild();
            context.toast(&tr("Every setting is back to its default."));
        }
    });
    window.present();
    cancel.grab_focus();
}

fn reset(context: &Rc<Context>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.append(&label(&tr("Put every setting back to its default"), &[]));
    let place = context
        .services
        .settings_file()
        .map(|p| tilde(&p))
        .unwrap_or_default();
    text.append(&note(&fill(
        &tr("The library, playlists, ratings and play counts are kept. Settings are stored in {}"),
        &[&place],
    )));
    line.append(&text);
    let button = compact_button(&tr("Reset settings…"));
    let weak = Rc::downgrade(context);
    button.connect_clicked(move |_| {
        if let Some(context) = weak.upgrade() {
            confirm_reset(&context);
        }
    });
    line.append(&button);
    line
}

/// Reading each track into memory before it plays, up to a size.
fn slow_drives(context: &Rc<Context>) -> gtk::Box {
    let (line, switch) = switch_row(
        &tr("Load each track into memory before it plays"),
        Some(&fill(
            &tr(
                "For drives that are slow or go to sleep. Tracks over {} are read from the drive as usual.",
            ),
            &[&bytes(helikodu_model::settings::MEMORY_BUFFER_LIMIT)],
        )),
    );
    switch.set_active(context.settings().playback.decoders.load_into_memory);
    switch.set_widget_name("load-into-memory");
    let weak = Rc::downgrade(context);
    switch.connect_active_notify(move |switch| {
        let Some(context) = weak.upgrade() else {
            return;
        };
        let on = switch.is_active();
        if !context.update(|s| s.playback.decoders.load_into_memory = on) {
            switch.set_active(!on);
        }
    });
    line
}

/// The Advanced page.
pub(super) fn page(context: &Rc<Context>) -> gtk::Widget {
    let (scroller, body) = page_body();
    body.append(&row(&tr("Tags in MP3 files"), None, &id3(context)));
    body.append(&row(&tr("Very large files"), None, &large_files(context)));
    body.append(&row(&tr("Statistics in files"), None, &statistics(context)));
    body.append(&divider());
    body.append(&row(
        &tr("Library database"),
        Some(&tr(
            "Holds the library, playlists, ratings and play history.",
        )),
        &database(context),
    ));
    body.append(&divider());
    body.append(&row(&tr("Slow drives"), None, &slow_drives(context)));
    body.append(&row(&tr("Logs"), None, &logs(context)));
    body.append(&row(&tr("Developer mode"), None, &developer(context)));
    body.append(&divider());
    body.append(&row(&tr("Reset"), None, &reset(context)));
    scroller.upcast()
}
