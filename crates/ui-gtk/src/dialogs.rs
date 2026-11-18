// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialogs: Settings, the full-output confirmation, the tag editor, the
//! first run, Keyboard shortcuts, Supported formats and About, with the
//! application actions that open them.

pub(crate) mod about;
pub(crate) mod common;
pub(crate) mod decoder_files;
pub(crate) mod first_run;
pub(crate) mod formats;
pub(crate) mod loudness_footer;
pub(crate) mod settings;
pub(crate) mod shortcuts;
pub(crate) mod tag_editor;
pub(crate) mod track_flags;
pub(crate) mod volume_confirm;

use gtk::gio;
use gtk::prelude::*;

use crate::backend::{Backend, PlayerRequest};
use crate::window::MainWindow;

/// The application's main window, whichever window is in front.
fn main_window(application: &gtk::Application) -> Option<MainWindow> {
    application
        .windows()
        .into_iter()
        .find_map(|w| w.downcast::<MainWindow>().ok())
}

fn add(application: &gtk::Application, name: &str, run: impl Fn(&MainWindow) + 'static) {
    let action = gio::SimpleAction::new(name, None);
    let app = application.downgrade();
    action.connect_activate(move |_, _| {
        if let Some(main) = app.upgrade().as_ref().and_then(main_window) {
            run(&main);
        }
    });
    application.add_action(&action);
}

/// Adds the actions that open the dialogs, and their shortcuts.
pub(crate) fn install_actions(application: &gtk::Application, backend: &Backend) {
    add(application, "preferences", |main| {
        settings::open(main, None);
    });
    add(application, "preferences-output", |main| {
        settings::open(main, Some(settings::Page::Output));
    });
    add(application, "formats", |main| {
        if let Some(backend) = main.backend() {
            formats::open(main.upcast_ref(), backend.services().formats());
        }
    });
    add(application, "shortcuts", |main| {
        shortcuts::open(main.upcast_ref());
    });
    add(application, "about", |main| {
        if let Some(backend) = main.backend() {
            about::open(main.upcast_ref(), &backend);
        }
    });
    add(application, "add-folder", |main| {
        let weak = main.downgrade();
        settings::library::choose_folder(main.upcast_ref(), move |path| {
            let Some(main) = weak.upgrade() else { return };
            let Some(backend) = main.backend() else {
                return;
            };
            let services = backend.services();
            let added = path.clone();
            let stored = services.update_settings(&mut |s| {
                if !s.library.folders.contains(&added) {
                    s.library.folders.push(added.clone());
                }
            });
            match stored.and_then(|()| services.scan(&[path])) {
                Ok(()) => first_run::hide(&main),
                Err(error) => main.toast(&error.0, true),
            }
        });
    });
    add(application, "rescan", |main| {
        if let Some(backend) = main.backend()
            && let Err(error) = backend.services().scan(&[])
        {
            main.toast(&error.0, true);
        }
    });
    // The seek shortcuts move by the step the settings name.
    for (name, sign) in [("seek-forward", 1.0), ("seek-back", -1.0)] {
        let action = gio::SimpleAction::new(name, None);
        let backend = backend.clone();
        action.connect_activate(move |_, _| {
            let step = f64::from(backend.services().settings().playback.seek_step_s);
            backend.send(PlayerRequest::SeekBy(sign * step));
        });
        application.add_action(&action);
    }
    application.set_accels_for_action("app.preferences", &["<Control>comma"]);
    application.set_accels_for_action("app.shortcuts", &["<Control>question"]);
}
