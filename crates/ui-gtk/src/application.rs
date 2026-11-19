// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The GtkApplication: start-up, the player actions and their shortcuts, and
//! the main window.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::{Backend, BackendError, PlayerRequest};
use crate::i18n::{self, tr};
use crate::style::{self, AccentChoice, SchemeChoice};
use crate::window::{MainWindow, WindowStyle};

/// The application ID.
pub const APP_ID: &str = "io.github.sorrelfox.Helikodu";

/// How the seek actions move.
const SEEK_STEP: f64 = 5.0;

/// How the front end starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunOptions {
    /// The window style; `None` picks the desktop's default.
    pub style: Option<WindowStyle>,
    /// Light, dark or the desktop's scheme.
    pub scheme: SchemeChoice,
    /// The accent.
    pub accent: AccentChoice,
    /// Announce each new track with a desktop notification while the window is
    /// not in front.
    pub notifications: bool,
}

/// Set when the program should start again once it has quit.
static RESTART: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Asks for the program to start again after it quits (a backup waits to be
/// put back). The program that called [`run`] does it.
pub fn request_restart() {
    RESTART.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Whether the program should start again now that [`run`] returned.
pub fn restart_requested() -> bool {
    RESTART.load(std::sync::atomic::Ordering::SeqCst)
}

/// Registers the resource bundle (stylesheet, templates, icons). Safe to call
/// more than once.
pub fn register_resources() {
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    if let Err(error) = gio::resources_register_include!("helikodu.gresource") {
        glib::g_critical!("helikodu", "cannot register the resource bundle: {error}");
    }
}

/// Prepares the default display: resources, stylesheet and theme following.
/// Call after GTK is initialised.
pub fn prepare_display(scheme: SchemeChoice, accent: AccentChoice) {
    register_resources();
    if let Some(display) = gtk::gdk::Display::default() {
        style::install(&display);
        if cfg!(debug_assertions) {
            check_icons(&display);
        }
    }
    style::set_choice(scheme, accent);
}

/// Logs the icon names the theme does not have (debug builds).
fn check_icons(display: &gtk::gdk::Display) {
    const NAMES: &[&str] = &[
        "go-previous-symbolic",
        "go-next-symbolic",
        "system-search-symbolic",
        "open-menu-symbolic",
        "media-playback-start-symbolic",
        "media-playback-pause-symbolic",
        "media-skip-backward-symbolic",
        "media-skip-forward-symbolic",
        "media-playlist-shuffle-symbolic",
        "media-playlist-repeat-symbolic",
        "helikodu-shuffle-symbolic",
        "helikodu-repeat-symbolic",
        "helikodu-repeat-one-symbolic",
        "helikodu-moon-symbolic",
        "helikodu-copies-symbolic",
        "audio-volume-high-symbolic",
        "audio-volume-medium-symbolic",
        "audio-volume-low-symbolic",
        "audio-volume-muted-symbolic",
        "list-add-symbolic",
        "view-more-symbolic",
        "go-home-symbolic",
        "folder-symbolic",
        "media-optical-symbolic",
        "audio-x-generic-symbolic",
        "avatar-default-symbolic",
        "helikodu-heart-symbolic",
        "starred-symbolic",
        "helikodu-sidebar-symbolic",
        "helikodu-tag-symbolic",
        "helikodu-composer-symbolic",
        "helikodu-calendar-symbolic",
        "helikodu-playlist-symbolic",
    ];
    let theme = gtk::IconTheme::for_display(display);
    for name in NAMES {
        if !theme.has_icon(name) {
            glib::g_warning!(
                "helikodu",
                "icon theme {:?} has no icon {name}",
                theme.theme_name()
            );
        }
    }
}

/// Adds the player actions to `application` and their shortcuts.
pub fn install_actions(application: &gtk::Application, backend: &Backend) {
    let requests: [(&str, PlayerRequest); 6] = [
        ("play-pause", PlayerRequest::PlayPause),
        ("stop", PlayerRequest::Stop),
        ("next", PlayerRequest::Next),
        ("previous", PlayerRequest::Previous),
        ("seek-forward", PlayerRequest::SeekBy(SEEK_STEP)),
        ("seek-back", PlayerRequest::SeekBy(-SEEK_STEP)),
    ];
    for (name, request) in requests {
        let action = gio::SimpleAction::new(name, None);
        let backend = backend.clone();
        action.connect_activate(move |_, _| backend.send(request.clone()));
        application.add_action(&action);
    }
    let stop_after = gio::SimpleAction::new_stateful("stop-after", None, &false.to_variant());
    let b = backend.clone();
    stop_after.connect_activate(move |action, _| {
        let on = action
            .state()
            .and_then(|s| s.get::<bool>())
            .unwrap_or(false);
        b.send(PlayerRequest::SetStopAfter(!on));
    });
    application.add_action(&stop_after);
    let clear = gio::SimpleAction::new("clear-queue", None);
    let b = backend.clone();
    clear.connect_activate(move |_, _| b.send(PlayerRequest::ClearQueue));
    application.add_action(&clear);
    let quit = gio::SimpleAction::new("quit", None);
    let app = application.downgrade();
    quit.connect_activate(move |_, _| {
        if let Some(app) = app.upgrade() {
            app.quit();
        }
    });
    application.add_action(&quit);
    crate::dialogs::install_actions(application, backend);
    crate::play_modes::install_actions(application, backend);
    crate::shortcuts::install_actions(application, backend);
    crate::mini_player::install_actions(application);
    crate::shortcuts::apply(application, &backend.services().settings().shortcuts);
}

/// Runs the application. `connect` starts the core when the application starts
/// (also when it is started over D-Bus for one of its actions); the front end
/// then talks to the backend it returns. `args` are passed on to GApplication:
/// files to open and `--gapplication-service`.
pub fn run(
    options: RunOptions,
    args: &[String],
    connect: impl Fn() -> Result<Backend, BackendError> + 'static,
) -> std::process::ExitCode {
    i18n::init();
    glib::set_application_name(&tr("Helikodu"));
    let application = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    let backend: Rc<RefCell<Option<Backend>>> = Rc::new(RefCell::new(None));
    let b = backend.clone();
    application.connect_startup(move |application| {
        prepare_display(options.scheme, options.accent);
        match connect() {
            Ok(backend) => {
                install_actions(application, &backend);
                b.replace(Some(backend));
            }
            Err(error) => {
                log::error!("{error}");
                let application = application.clone();
                glib::timeout_add_local_once(Duration::ZERO, move || application.quit());
            }
        }
    });
    // The restored session plays on once, when the program is first shown,
    // unless it was started to play files.
    let started = Rc::new(std::cell::Cell::new(false));
    let b = backend.clone();
    let s = started.clone();
    application.connect_activate(move |application| {
        present(application, &b, options);
        if let Some(backend) = b.borrow().as_ref()
            && !s.replace(true)
        {
            backend.resume_session();
        }
    });
    let b = backend.clone();
    application.connect_open(move |application, files, hint| {
        let paths: Vec<_> = files.iter().filter_map(gio::File::path).collect();
        present(application, &b, options);
        let Some(backend) = b.borrow().clone() else {
            return;
        };
        let enqueue = hint == "enqueue";
        let plays = !paths.is_empty() && !enqueue;
        if !paths.is_empty() {
            backend.send(PlayerRequest::PlayFiles {
                files: paths,
                enqueue,
            });
        }
        if !started.replace(true) && !plays {
            backend.resume_session();
        }
    });
    let mut all = vec![std::env::args().next().unwrap_or_else(|| "helikodu".into())];
    all.extend(args.iter().cloned());
    if application.run_with_args(&all) == glib::ExitCode::SUCCESS {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

/// Shows the main window, making it first if needed.
fn present(
    application: &gtk::Application,
    backend: &Rc<RefCell<Option<Backend>>>,
    options: RunOptions,
) {
    if let Some(window) = application.active_window() {
        window.present();
        return;
    }
    let Some(backend) = backend.borrow().clone() else {
        return;
    };
    let style = options.style.unwrap_or_else(WindowStyle::for_desktop);
    let window = MainWindow::new(application, &backend, style);
    window.set_notifications(options.notifications);
    window.present();
    window.start_desktop_services();
    crate::dialogs::maintenance::show_start_notices(&window, &backend);
}
