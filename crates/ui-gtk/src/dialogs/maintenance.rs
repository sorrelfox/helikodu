// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keeping the library safe: putting a backup back, and what the window says
//! after a crash or about the library database at start.
//!
//! The design draws neither the restore confirmation nor the crash notice; both
//! follow the full-output confirmation (an icon tile, a question, a sentence, a
//! readout box and the buttons at the bottom right), and the crash notice is a
//! toast with "Details", whose dialog holds the report in a readout box with
//! "Open folder", "Copy" and "Close".

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::common::{copy_text, dialog_window, note};
use crate::backend::Backend;
use crate::backend::services::CrashNotice;
use crate::i18n::{fill, tr};
use crate::widgets::label;
use crate::window::MainWindow;

/// The name the crash report window is known by among the toplevels.
pub(crate) const CRASH_NAME: &str = "crash-report";
/// The name of the restore confirmation.
pub(crate) const RESTORE_NAME: &str = "restore-backup";

/// The frame shared by the two dialogs: icon tile, title, sentence, an
/// optional readout, and a button row whose buttons the caller adds.
struct Frame {
    window: gtk::Window,
    body: gtk::Box,
    buttons: gtk::Box,
}

fn frame(parent: &gtk::Window, name: &str, icon: &str, title: &str, sentence: &str) -> Frame {
    let window = dialog_window(Some(parent), name, title, (480, -1), true);
    window.set_resizable(false);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("hk-dialog-body");
    body.add_css_class("hk-confirm");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tile.add_css_class("hk-icon-tile");
    tile.set_valign(gtk::Align::Start);
    let image = gtk::Image::from_icon_name(icon);
    image.set_vexpand(true);
    image.set_valign(gtk::Align::Center);
    tile.append(&image);
    top.append(&tile);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 7);
    text.set_hexpand(true);
    text.append(&label(title, &["hk-dialog-title"]));
    let explanation = note(sentence);
    explanation.remove_css_class("hk-note");
    explanation.add_css_class("hk-t2");
    explanation.add_css_class("hk-confirm-text");
    explanation.set_max_width_chars(46);
    text.append(&explanation);
    top.append(&text);
    body.append(&top);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.add_css_class("hk-dialog-buttons");
    buttons.set_halign(gtk::Align::End);
    window.set_child(Some(&body));
    Frame {
        window,
        body,
        buttons,
    }
}

/// A readout box with one micro label and a mono value.
fn readout(title: &str, value: &str) -> gtk::Box {
    let readout = gtk::Box::new(gtk::Orientation::Vertical, 3);
    readout.add_css_class("hk-readout");
    readout.add_css_class("hk-glass");
    readout.add_css_class("hk-confirm-readout");
    readout.append(&label(title, &["hk-micro-label"]));
    let value = label(value, &["hk-mono", "hk-confirm-value"]);
    value.set_wrap(true);
    value.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    value.set_selectable(true);
    readout.append(&value);
    readout
}

fn button(text: &str) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    button.add_css_class("hk-btn");
    button.set_valign(gtk::Align::Center);
    button
}

/// Asks for a backup file in `folder`, then asks before putting it back:
/// `accepted` runs with the chosen file once the user agrees.
pub(crate) fn choose_backup(
    parent: &gtk::Window,
    folder: &Path,
    accepted: impl Fn(PathBuf) + 'static,
) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(&tr("Library backups")));
    filter.add_pattern("*.db");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title(tr("Choose a backup to put back"))
        .accept_label(tr("Choose"))
        .modal(true)
        .filters(&filters)
        .build();
    if folder.is_dir() {
        dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
    }
    let parent_weak = parent.downgrade();
    dialog.open(Some(parent), gio::Cancellable::NONE, move |result| {
        let Ok(Some(path)) = result.map(|f| f.path()) else {
            return;
        };
        if let Some(parent) = parent_weak.upgrade() {
            confirm_restore(&parent, path, accepted);
        }
    });
}

/// Asks before putting the backup at `backup` back.
pub(crate) fn confirm_restore(
    parent: &gtk::Window,
    backup: PathBuf,
    accepted: impl Fn(PathBuf) + 'static,
) -> gtk::Window {
    let Frame {
        window,
        body,
        buttons,
    } = frame(
        parent,
        RESTORE_NAME,
        "document-revert-symbolic",
        &tr("Put this backup back?"),
        &tr(
            "The library goes back to how it was when the backup was made: playlists, ratings and play counts changed since then are lost. The library as it is now is kept in the backups folder. Helikodu starts again to do it.",
        ),
    );
    let name = backup
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    body.append(&readout(&tr("Backup"), &name));
    let cancel = button(&tr("Cancel"));
    let restore = button(&tr("Restore and restart"));
    buttons.append(&cancel);
    buttons.append(&restore);
    body.append(&buttons);
    window.set_default_widget(Some(&cancel));
    let weak = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    let weak = window.downgrade();
    restore.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
        accepted(backup.clone());
    });
    window.present();
    cancel.grab_focus();
    window.set_focus_visible(true);
    window
}

/// Starts the program again once it has quit: the backup staged to be put
/// back is put back at the start.
pub(crate) fn restart(widget: &impl IsA<gtk::Widget>) {
    crate::application::request_restart();
    let application = widget
        .as_ref()
        .root()
        .and_downcast::<gtk::Window>()
        .and_then(|w| w.application());
    match application {
        Some(application) => application.quit(),
        None => {
            if let Some(application) = gio::Application::default() {
                application.quit();
            }
        }
    }
}

/// Shows the crash report `notice` with "Open folder", "Copy" and "Close".
pub(crate) fn crash_details(main: &MainWindow, notice: &CrashNotice) -> gtk::Window {
    let parent: &gtk::Window = main.upcast_ref();
    let Frame {
        window,
        body,
        buttons,
    } = frame(
        parent,
        CRASH_NAME,
        "dialog-warning-symbolic",
        &tr("Helikodu stopped unexpectedly"),
        &tr(
            "A report was saved on this computer. Nothing was sent anywhere. To report the problem, copy the report and add it to a bug report.",
        ),
    );
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(180)
        .max_content_height(260)
        .propagate_natural_height(true)
        .build();
    scroller.set_child(Some(&readout(&tr("Report"), notice.text.trim_end())));
    body.append(&scroller);
    let open = button(&tr("Open folder"));
    let copy = button(&tr("Copy"));
    let close = button(&tr("Close"));
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    buttons.set_halign(gtk::Align::Fill);
    buttons.append(&open);
    buttons.append(&spacer);
    buttons.append(&copy);
    buttons.append(&close);
    body.append(&buttons);
    window.set_default_widget(Some(&close));
    let path = notice.path.clone();
    let main_weak = main.downgrade();
    open.connect_clicked(move |_| {
        if let Some(main) = main_weak.upgrade() {
            crate::views::actions::open_folder(&main, Some(path.clone()));
        }
    });
    let text = notice.text.clone();
    let main_weak = main.downgrade();
    copy.connect_clicked(move |button| {
        copy_text(button, &text);
        if let Some(main) = main_weak.upgrade() {
            main.toast(&tr("The report is on the clipboard."), false);
        }
    });
    let weak = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    window.present();
    close.grab_focus();
    window
}

/// Says once, after the window is shown, what the start found: a crash report
/// not shown yet (a toast with "Details"), or else the first notice about the
/// library database.
pub(crate) fn show_start_notices(main: &MainWindow, backend: &Backend) {
    let services = backend.services().clone();
    let weak = main.downgrade();
    glib::spawn_future_local(async move {
        let found = gio::spawn_blocking(move || {
            let crash = services.unseen_crash_report();
            if crash.is_some() {
                services.mark_crash_reports_seen();
            }
            (crash, services.start_notices())
        })
        .await;
        let (Ok((crash, notices)), Some(main)) = (found, weak.upgrade()) else {
            return;
        };
        if let Some(crash) = crash {
            let notice = Rc::new(crash);
            let weak = main.downgrade();
            main.toast_with(
                &tr(
                    "Helikodu stopped unexpectedly last time. A report was saved on this computer.",
                ),
                &tr("Details"),
                true,
                move || {
                    if let Some(main) = weak.upgrade() {
                        crash_details(&main, &notice);
                    }
                },
            );
        } else if let Some(first) = notices.first() {
            main.toast(first, true);
        }
    });
}

/// "today at 03:10", "yesterday at 03:10" or the date and time of `time`.
pub(crate) fn when(time: std::time::SystemTime) -> String {
    let seconds = time
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let Ok(at) = glib::DateTime::from_unix_local(i64::try_from(seconds).unwrap_or(0)) else {
        return String::new();
    };
    let clock = at
        .format("%H:%M")
        .map(|s| s.to_string())
        .unwrap_or_default();
    let today = glib::DateTime::now_local().ok();
    let days = today.map_or(i32::MAX, |today| {
        (today.year() * 400 + today.day_of_year()) - (at.year() * 400 + at.day_of_year())
    });
    match days {
        0 => fill(&tr("today at {}"), &[&clock]),
        1 => fill(&tr("yesterday at {}"), &[&clock]),
        _ => crate::format::local_time(time),
    }
}

/// The line under the database path: its size and the last check.
pub(crate) fn check_line(
    size: Option<u64>,
    info: Option<&crate::backend::services::BackupsInfo>,
) -> String {
    let mut parts = Vec::new();
    if let Some(size) = size {
        parts.push(super::common::bytes(size));
    }
    if let Some(info) = info {
        if info.read_only {
            parts.push(tr("read-only"));
        }
        match &info.check {
            Some((at, None)) => parts.push(fill(
                &tr("last checked {}, no problems found"),
                &[&when(*at)],
            )),
            Some((at, Some(problem))) => parts.push(fill(
                &tr("last checked {}, problems found: {}"),
                &[&when(*at), problem],
            )),
            None => parts.push(tr("not checked yet")),
        }
    }
    parts.join(" · ")
}

/// The line under "Backups": the rotation and the newest backup.
pub(crate) fn backups_line(info: &crate::backend::services::BackupsInfo) -> String {
    let kept = info.kept.to_string();
    let rotation = if info.daily {
        fill(&tr("One a day, the last {} are kept"), &[&kept])
    } else {
        fill(&tr("Made when you ask, the last {} are kept"), &[&kept])
    };
    let latest = match info.latest {
        Some(at) => fill(&tr("latest {}"), &[&when(at)]),
        None => tr("none yet"),
    };
    let mut line = format!("{rotation} · {latest}");
    if info.restore_pending {
        line.push_str(" · ");
        line.push_str(&tr("one is put back at the next start"));
    }
    line
}
