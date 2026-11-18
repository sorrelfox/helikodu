// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Renders the dialogs to PNG files, dark and light, over the sample backend:
//! the Settings pages, the full-output confirmation, the output switcher, the
//! tag editor, the first run, a scan in progress, Supported formats and
//! About. Run it headless:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo run -p helikodu-ui-gtk --example dialogs -- --out target/screenshots
//! ```

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_model::settings::VolumeControl;
use helikodu_ui_gtk::backend::services::ScanStatus;
use helikodu_ui_gtk::backend::{AlbumQuery, Backend, PlayerRequest, sample};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice, set_choice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display, snapshot};

fn spin(ms: u64) {
    let context = glib::MainContext::default();
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn toplevel(name: &str) -> Option<gtk::Window> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .find(|w| w.widget_name() == name && w.is_visible())
}

fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut all = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(widget) = pending.pop() {
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c.clone());
            all.push(c);
        }
    }
    all
}

/// Renders a popover (its own surface) to `path`.
fn save_native(widget: &impl IsA<gtk::Native>, path: &Path) -> Result<(), String> {
    let widget = widget.upcast_ref::<gtk::Native>();
    let as_widget = widget.upcast_ref::<gtk::Widget>();
    let paintable = gtk::WidgetPaintable::new(Some(as_widget));
    let snapshot = gtk::Snapshot::new();
    let (w, h) = (as_widget.width(), as_widget.height());
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let node = snapshot.to_node().ok_or("the popover drew nothing")?;
    let renderer = widget.renderer().ok_or("the popover is not realised")?;
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32);
    renderer
        .render_texture(&node, Some(&bounds))
        .save_to_png(path)
        .map_err(|e| e.to_string())
}

fn window(application: &gtk::Application, backend: &Backend) -> MainWindow {
    install_actions(application, backend);
    let window = MainWindow::new(application, backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(400);
    window
}

fn save(window: &impl IsA<gtk::Window>, dir: &Path, name: &str) {
    spin(300);
    let path = dir.join(format!("{name}.png"));
    match snapshot::save_png(window, &path) {
        Ok(()) => println!("{}", path.display()),
        Err(error) => eprintln!("{name}: {error}"),
    }
}

fn shoot(application: &gtk::Application, dir: &Path, suffix: &str) {
    let options = sample::SampleOptions {
        albums: 120,
        playing: Some((3, 1, 102)),
    };
    let backend = sample::backend(&options);
    let main = window(application, &backend);

    for page in [
        "library",
        "playback",
        "output",
        "loudness",
        "appearance",
        "advanced",
    ] {
        let settings = main.open_settings(page);
        settings.set_default_size(1080, 780);
        save(&settings, dir, &format!("settings-{page}{suffix}"));
    }
    if let Some(settings) = toplevel("settings") {
        settings.close();
    }

    // The output switcher.
    if let Some(popover) = main.open_output_switcher() {
        spin(500);
        let path = dir.join(format!("output-switcher{suffix}.png"));
        match save_native(&popover, &path) {
            Ok(()) => println!("{}", path.display()),
            Err(error) => eprintln!("output switcher: {error}"),
        }
        popover.popdown();
    }

    // The tag editor over three tracks of an album.
    let library = backend.library();
    let key = library
        .album_keys(&AlbumQuery::default())
        .unwrap_or_default()[3];
    let tracks: Vec<_> = library
        .album(key)
        .ok()
        .flatten()
        .map(|a| a.tracks().take(3).map(|t| t.key).collect())
        .unwrap_or_default();
    main.edit_tags(&tracks);
    spin(300);
    if let Some(editor) = toplevel("tag-editor") {
        save(&editor, dir, &format!("tag-editor{suffix}"));
        for (tab, name) in [("Cover", "cover"), ("File info", "info")] {
            if let Some(button) = descendants(editor.upcast_ref())
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::ToggleButton>().ok())
                .find(|b| b.label().as_deref() == Some(tab))
            {
                button.set_active(true);
                save(&editor, dir, &format!("tag-editor-{name}{suffix}"));
            }
        }
        editor.close();
    }

    gio::prelude::ActionGroupExt::activate_action(application, "formats", None);
    if let Some(formats) = toplevel("formats") {
        save(&formats, dir, &format!("formats{suffix}"));
        formats.close();
    }
    gio::prelude::ActionGroupExt::activate_action(application, "about", None);
    if let Some(about) = toplevel("about") {
        save(&about, dir, &format!("about{suffix}"));
        about.close();
    }

    // A scan in progress.
    main.show_scan_status(&ScanStatus {
        current: Some(glib::home_dir().join("Music/Halden/2018 Concrete Gardens")),
        done: 12,
        total: Some(100),
        ..ScanStatus::default()
    });
    save(&main, dir, &format!("scanning{suffix}"));

    // The full-output confirmation, before playback to a fixed-volume device.
    backend.send(PlayerRequest::Stop);
    let _ = backend
        .services()
        .update_settings(&mut |s| s.output.volume = VolumeControl::Fixed);
    backend.send(PlayerRequest::PlayAlbum {
        album: key,
        start: 0,
        shuffle: false,
    });
    spin(200);
    if let Some(confirm) = toplevel("volume-confirm") {
        save(&confirm, dir, &format!("volume-confirm{suffix}"));
        confirm.close();
    }
    main.destroy();

    // The first run.
    let backend = sample::first_run_backend(&options);
    let main = window(application, &backend);
    save(&main, dir, &format!("first-run{suffix}"));
    main.destroy();
}

fn main() -> ExitCode {
    let mut out = PathBuf::from("target/screenshots");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--out"
            && let Some(dir) = args.next()
        {
            out = PathBuf::from(dir);
        }
    }
    if let Err(error) = std::fs::create_dir_all(&out) {
        eprintln!("cannot create {}: {error}", out.display());
        return ExitCode::FAILURE;
    }
    if gtk::init().is_err() {
        eprintln!("no display; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::FAILURE;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.DialogShots")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application.connect_activate(move |application| {
        let _hold = application.hold();
        set_choice(SchemeChoice::Dark, AccentChoice::Amber);
        shoot(application, &out, "");
        set_choice(SchemeChoice::Light, AccentChoice::Amber);
        spin(200);
        shoot(application, &out, ".light");
        application.quit();
    });
    application.run_with_args(&["dialogs"]);
    ExitCode::SUCCESS
}
