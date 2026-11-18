// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Renders the main window to PNG files, dark and light, with the Albums view
//! and an album page, over the sample backend. Run it headless:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo run -p helikodu-ui-gtk --example screenshot -- --out target/screenshots
//! ```
//!
//! Options: `--out DIR` (default `target/screenshots`), `--size WxH` (default
//! 1440x900), `--accent amber|blue|green|violet`, `--style integrated|classic`,
//! and `--database PATH` to show a real library instead of the sample one. With
//! a database the player plays its first album to the null output, so nothing
//! is heard.

#![forbid(unsafe_code)]

use std::cell::Cell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_service::files::AppDirs;
use helikodu_service::library::MusicLibrary;
use helikodu_service::settings::SETTINGS_FILE;
use helikodu_service::{LibraryChoice, Service, ServiceConfig};
use helikodu_ui_gtk::backend::{
    AlbumKey, AlbumQuery, Backend, BackendError, PlayerRequest, sample, service,
};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice, set_choice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display, snapshot};

struct Options {
    out: PathBuf,
    width: i32,
    height: i32,
    accent: AccentChoice,
    style: WindowStyle,
    database: Option<PathBuf>,
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        out: PathBuf::from("target/screenshots"),
        width: 1440,
        height: 900,
        accent: AccentChoice::Amber,
        style: WindowStyle::Integrated,
        database: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => options.out = PathBuf::from(value()?),
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or("--size is WxH")?;
                options.width = w.parse().map_err(|_| "bad width")?;
                options.height = h.parse().map_err(|_| "bad height")?;
            }
            "--accent" => {
                options.accent = match value()?.as_str() {
                    "blue" => AccentChoice::Blue,
                    "green" => AccentChoice::Green,
                    "violet" => AccentChoice::Violet,
                    _ => AccentChoice::Amber,
                }
            }
            "--style" => {
                options.style = if value()? == "classic" {
                    WindowStyle::Classic
                } else {
                    WindowStyle::Integrated
                }
            }
            "--database" => options.database = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

/// The service over `database`, with its settings, session and caches in a
/// folder of its own beside the screenshots, playing to the null output.
fn real_backend(
    database: &std::path::Path,
    out: &std::path::Path,
) -> Result<Backend, BackendError> {
    let fail = |e: &dyn std::fmt::Display| BackendError(e.to_string());
    let root = out.join("service");
    let _ = std::fs::remove_dir_all(&root);
    let dirs = AppDirs::under(&root);
    std::fs::create_dir_all(&dirs.config).map_err(|e| fail(&e))?;
    std::fs::write(
        dirs.config.join(SETTINGS_FILE),
        "[output]\nbackend = \"null\"\n",
    )
    .map_err(|e| fail(&e))?;
    let mut config = ServiceConfig::new(dirs);
    config.library =
        LibraryChoice::Open(MusicLibrary::open_existing(database).map_err(|e| fail(&e))?);
    let service = Service::start(config).map_err(|e| fail(&e))?;
    service::connect(
        service,
        service::ServiceOptions {
            mpris: false,
            inhibit_suspend: false,
        },
    )
}

fn main() -> ExitCode {
    let options = match options() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("screenshot: {e}");
            return ExitCode::FAILURE;
        }
    };
    if gtk::init().is_err() {
        eprintln!("screenshot: no display; run it under tests/headless.sh");
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::create_dir_all(&options.out) {
        eprintln!("screenshot: {e}");
        return ExitCode::FAILURE;
    }
    prepare_display(SchemeChoice::Dark, options.accent);
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_decoration_layout(Some("icon:minimize,maximize,close"));
    }
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.Screenshot")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let failed = Rc::new(Cell::new(false));
    let f = failed.clone();
    application.connect_activate(move |application| {
        let backend = match &options.database {
            Some(path) => match real_backend(path, &options.out) {
                Ok(backend) => backend,
                Err(e) => {
                    eprintln!("screenshot: {e}");
                    f.set(true);
                    application.quit();
                    return;
                }
            },
            None => sample::backend(&sample::SampleOptions {
                playing: Some((0, 2, 102)),
                ..sample::SampleOptions::default()
            }),
        };
        let first: Option<AlbumKey> = backend
            .library()
            .album_keys(&AlbumQuery::default())
            .ok()
            .and_then(|k| k.first().copied());
        if options.database.is_some()
            && let Some(album) = first
        {
            backend.send(PlayerRequest::PlayAlbum {
                album,
                start: 2,
                shuffle: false,
            });
        }
        install_actions(application, &backend);
        let window = MainWindow::new(application, &backend, options.style);
        window.set_default_size(options.width, options.height);
        window.present();
        let steps: Vec<(&str, SchemeChoice, bool)> = vec![
            ("main-dark", SchemeChoice::Dark, false),
            ("main-light", SchemeChoice::Light, false),
            ("album-light", SchemeChoice::Light, true),
            ("album-dark", SchemeChoice::Dark, true),
        ];
        let out = options.out.clone();
        let prefix = if options.database.is_some() {
            "real-"
        } else {
            ""
        };
        let application = application.clone();
        let failed = f.clone();
        glib::spawn_future_local(async move {
            for (name, scheme, album) in steps {
                set_choice(scheme, options.accent);
                if album && let Some(key) = first {
                    window.open_album(key);
                }
                glib::timeout_future(Duration::from_millis(1200)).await;
                let path = out.join(format!("{prefix}{name}.png"));
                match snapshot::save_png(&window, &path) {
                    Ok(()) => println!("wrote {}", path.display()),
                    Err(e) => {
                        eprintln!("screenshot: {e}");
                        failed.set(true);
                    }
                }
            }
            application.quit();
        });
    });
    application.run_with_args(&["screenshot"]);
    if failed.get() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
