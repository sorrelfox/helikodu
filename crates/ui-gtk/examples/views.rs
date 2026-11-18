// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Renders the library views to PNG files, dark and light, over the sample
//! backend, for comparison with the design: Tracks, Artists, Genres, Folders,
//! a playlist, search with and without results, the empty library, and the
//! main menu, the track context menu and the play button menu. Run it
//! headless:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo run -p helikodu-ui-gtk --example views -- --out target/views
//! ```
//!
//! Options: `--out DIR` (default `target/views`), `--size WxH` (default
//! 1440x900), `--only NAME` to render one view.

#![forbid(unsafe_code)]

use std::cell::Cell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_ui_gtk::backend::sample;
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice, set_choice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display, snapshot};

/// A step: its file name, what to do before rendering, and whether the image
/// is of a popover rather than the window.
type Step = (&'static str, fn(&MainWindow) -> Option<gtk::Popover>);

fn steps() -> Vec<Step> {
    vec![
        ("tracks", |w| {
            w.open_page("tracks");
            None
        }),
        ("artists", |w| {
            w.open_page("artists");
            None
        }),
        ("genres", |w| {
            w.open_page("genres");
            None
        }),
        ("folders", |w| {
            w.open_page("folders");
            w.demo_folder_step();
            None
        }),
        ("playlist", |w| {
            w.open_first_playlist();
            None
        }),
        ("search", |w| {
            w.search_text("harbour");
            None
        }),
        ("no-results", |w| {
            w.search_text("harbuor");
            None
        }),
        ("context-menu", |w| {
            w.search_text("");
            w.open_first_playlist();
            None
        }),
        ("main-menu", |w| w.demo_main_menu()),
        ("play-menu", |w| Some(w.demo_play_menu())),
    ]
}

fn main() -> ExitCode {
    let mut out = PathBuf::from("target/views");
    let (mut width, mut height) = (1440, 900);
    let mut only: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--out", Some(v)) => out = PathBuf::from(v),
            ("--size", Some(v)) => {
                if let Some((w, h)) = v.split_once('x') {
                    width = w.parse().unwrap_or(width);
                    height = h.parse().unwrap_or(height);
                }
            }
            ("--only", Some(v)) => only = Some(v),
            _ => {
                eprintln!("views: unknown option {arg}");
                return ExitCode::FAILURE;
            }
        }
    }
    if gtk::init().is_err() {
        eprintln!("views: no display; run it under tests/headless.sh");
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("views: {e}");
        return ExitCode::FAILURE;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_decoration_layout(Some("icon:minimize,maximize,close"));
    }
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.Views")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let failed = Rc::new(Cell::new(false));
    let f = failed.clone();
    application.connect_activate(move |application| {
        let backend = sample::backend(&sample::SampleOptions {
            playing: Some((0, 2, 102)),
            ..sample::SampleOptions::default()
        });
        install_actions(application, &backend);
        let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
        window.set_default_size(width, height);
        window.present();
        let empty = sample::backend(&sample::SampleOptions {
            albums: 0,
            playing: None,
        });
        let empty_window = MainWindow::new(application, &empty, WindowStyle::Integrated);
        empty_window.set_default_size(width, height);
        let out = out.clone();
        let only = only.clone();
        let application = application.clone();
        let failed = f.clone();
        glib::spawn_future_local(async move {
            glib::timeout_future(Duration::from_millis(600)).await;
            let save = |name: &str, widget: &gtk::Widget| {
                let path = out.join(format!("{name}.png"));
                let result = match widget.downcast_ref::<gtk::Window>() {
                    Some(window) => snapshot::save_png(window, &path),
                    None => snapshot::save_widget_png(widget, &path),
                };
                match result {
                    Ok(()) => println!("wrote {}", path.display()),
                    Err(e) => {
                        eprintln!("views: {name}: {e}");
                        failed.set(true);
                    }
                }
            };
            for (name, prepare) in steps() {
                if only.as_deref().is_some_and(|o| o != name) {
                    continue;
                }
                for (scheme, suffix) in
                    [(SchemeChoice::Dark, "dark"), (SchemeChoice::Light, "light")]
                {
                    set_choice(scheme, AccentChoice::Amber);
                    let popover = prepare(&window);
                    glib::timeout_future(Duration::from_millis(900)).await;
                    if name == "folders" {
                        for _ in 0..3 {
                            window.demo_folder_step();
                            glib::timeout_future(Duration::from_millis(400)).await;
                        }
                    }
                    let popover = if name == "context-menu" {
                        window.demo_track_menu()
                    } else {
                        popover
                    };
                    glib::timeout_future(Duration::from_millis(300)).await;
                    let file = format!("{name}-{suffix}");
                    save(&file, window.upcast_ref());
                    if let Some(popover) = popover {
                        save(&format!("{file}-popover"), popover.upcast_ref());
                        popover.popdown();
                    }
                }
            }
            if only.as_deref().is_none_or(|o| o == "empty-library") {
                window.set_visible(false);
                empty_window.present();
                for (scheme, suffix) in
                    [(SchemeChoice::Dark, "dark"), (SchemeChoice::Light, "light")]
                {
                    set_choice(scheme, AccentChoice::Amber);
                    glib::timeout_future(Duration::from_millis(900)).await;
                    save(
                        &format!("empty-library-{suffix}"),
                        empty_window.upcast_ref(),
                    );
                }
            }
            application.quit();
        });
    });
    application.run_with_args(&["views"]);
    if failed.get() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
