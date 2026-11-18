// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Renders the dialogs to PNG files, dark and light, over the sample backend:
//! the Settings pages, the full-output confirmation, the output switcher, the
//! tag editor, the first run, a scan in progress, Supported formats and
//! About; and DSP and EQ with each editor (`--dsp` renders only those and the
//! pages with sound-processing controls); and the copies (`--copies` renders
//! only those): Settings › Library, the track table with its badges, the
//! copies window with its confirmation and results, and Find duplicates;
//! and the converter, Sync and export and the cover manager (`--tools`
//! renders only those).
//! `--backdrop` draws every window as when another window has the focus
//! (themes colour some text differently then). Run it headless:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo run -p helikodu-ui-gtk --example dialogs -- --out target/screenshots
//! ```

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_model::settings::VolumeControl;
use helikodu_ui_gtk::backend::services::ScanStatus;
use helikodu_ui_gtk::backend::{
    AlbumQuery, Backend, OutputInfo, PlaybackState, PlayerRequest, SignalState, Update, sample,
};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice, set_choice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display, snapshot};

/// Set by `--backdrop`.
static BACKDROP: AtomicBool = AtomicBool::new(false);

/// Marks `widget`'s window as without the focus, with `--backdrop`.
fn backdrop(widget: &impl IsA<gtk::Widget>) {
    if BACKDROP.load(Ordering::Relaxed)
        && let Some(root) = widget.root()
    {
        root.set_state_flags(gtk::StateFlags::BACKDROP, false);
        spin(50);
    }
}

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

/// Renders a popover (its own surface) to `path`, cut to its frame: the
/// child and the 1 px border around it, without the shadow.
fn save_native(widget: &impl IsA<gtk::Native>, path: &Path) -> Result<(), String> {
    let widget = widget.upcast_ref::<gtk::Native>();
    let as_widget = widget.upcast_ref::<gtk::Widget>();
    if let Some(parent) = as_widget.parent() {
        backdrop(&parent);
    }
    let paintable = gtk::WidgetPaintable::new(Some(as_widget));
    let snapshot = gtk::Snapshot::new();
    let (w, h) = (as_widget.width(), as_widget.height());
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let node = snapshot.to_node().ok_or("the popover drew nothing")?;
    let renderer = widget.renderer().ok_or("the popover is not realised")?;
    let bounds = as_widget
        .downcast_ref::<gtk::Popover>()
        .and_then(|p| p.child())
        .and_then(|c| c.compute_bounds(as_widget))
        .map(|b| {
            gtk::graphene::Rect::new(b.x() - 1.0, b.y() - 1.0, b.width() + 2.0, b.height() + 2.0)
        })
        .unwrap_or_else(|| gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32));
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
    backdrop(window.upcast_ref::<gtk::Window>());
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

    // A scan in progress, with tiles still loading, and the output device
    // lost: paused, the output button not connected, a toast. The panel is
    // hidden, as drawn.
    let (backend, updates) = sample::scanning_backend(&options, 10);
    let main = window(application, &backend);
    let _ = WidgetExt::activate_action(&main, "win.toggle-panel", None);
    main.show_scan_status(&ScanStatus {
        current: Some(glib::home_dir().join("Music/Halden/2018 Concrete Gardens")),
        done: 12,
        total: Some(100),
        ..ScanStatus::default()
    });
    let _ = updates.send_blocking(Update::State(PlaybackState::Paused));
    let _ = updates.send_blocking(Update::Output(OutputInfo {
        device: "Studio Interface".to_owned(),
        signal: SignalState::NotConnected,
        sample_rate: None,
        bits: None,
    }));
    spin(400);
    save(&main, dir, &format!("scanning{suffix}"));
    let _ = WidgetExt::activate_action(&main, "win.toggle-panel", None);
    main.destroy();

    // The first run.
    let backend = sample::first_run_backend(&options);
    let main = window(application, &backend);
    save(&main, dir, &format!("first-run{suffix}"));
    main.destroy();
}

/// Settings › DSP and EQ with each editor, as drawn, and the pages with the
/// sound-processing controls.
fn shoot_dsp(application: &gtk::Application, dir: &Path, suffix: &str) {
    use helikodu_model::settings::{FilterKind, ParametricFilter};
    let options = sample::SampleOptions {
        albums: 120,
        playing: Some((3, 1, 102)),
    };
    let backend = sample::backend(&options);
    let main = window(application, &backend);
    let filter = |kind, frequency_hz, gain_db, q| ParametricFilter {
        kind,
        frequency_hz,
        gain_db,
        q,
        enabled: true,
    };
    let _ = backend.services().update_settings(&mut |s| {
        let eq = &mut s.dsp.equalizer;
        eq.enabled = true;
        eq.parametric = vec![
            filter(FilterKind::LowShelf, 105.0, 4.5, 0.7),
            filter(FilterKind::Peak, 220.0, -2.4, 0.9),
            filter(FilterKind::Peak, 1800.0, -1.8, 1.6),
            filter(FilterKind::Peak, 3500.0, 3.0, 2.2),
            filter(FilterKind::HighShelf, 9000.0, -3.5, 0.7),
        ];
        eq.graphic_10 = [4.5, 3.5, 1.5, -1.0, -2.0, -1.0, 0.5, 2.5, 1.0, -2.0];
        eq.graphic_31 = [
            5.0, 5.0, 4.5, 4.0, 4.0, 3.5, 3.0, 2.0, 1.5, 0.5, 0.0, -1.0, -1.5, -1.5, -2.0, -1.5,
            -1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 2.0, 2.5, 2.0, 1.5, 1.0, 0.0, -1.0, -2.0, -2.5,
        ];
        s.dsp.limiter.enabled = true;
    });
    for (stage, name) in [
        ("equalizer", "dsp-parametric"),
        ("crossfeed", "dsp-crossfeed"),
        ("channel-mix", "dsp-channel-mix"),
        ("limiter", "dsp-limiter"),
    ] {
        let settings = main.open_dsp_stage(stage);
        settings.set_default_size(1080, 780);
        save(&settings, dir, &format!("{name}{suffix}"));
    }
    for (mode, name) in [
        ("Graphic, 10 bands", "dsp-graphic-10"),
        ("Graphic, 31 bands", "dsp-graphic-31"),
    ] {
        let settings = main.open_dsp_stage("equalizer");
        if let Some(button) = descendants(settings.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::ToggleButton>().ok())
            .find(|b| b.label().as_deref() == Some(mode))
        {
            button.set_active(true);
        }
        save(&settings, dir, &format!("{name}{suffix}"));
    }
    for page in ["playback", "output", "loudness", "advanced"] {
        let settings = main.open_settings(page);
        save(&settings, dir, &format!("settings-{page}{suffix}"));
    }
    if let Some(settings) = toplevel("settings") {
        settings.close();
    }
    gio::prelude::ActionGroupExt::activate_action(application, "formats", None);
    if let Some(formats) = toplevel("formats") {
        save(&formats, dir, &format!("formats{suffix}"));
        formats.close();
    }
    main.destroy();
}

fn named<T: IsA<gtk::Widget>>(root: &impl IsA<gtk::Widget>, name: &str) -> Option<T> {
    descendants(root.upcast_ref())
        .into_iter()
        .find(|w| w.widget_name() == name)
        .and_then(|w| w.downcast::<T>().ok())
}

fn shoot_copies(application: &gtk::Application, dir: &Path, suffix: &str) {
    let options = sample::SampleOptions {
        albums: 120,
        playing: None,
    };
    let backend = sample::backend(&options);
    let main = window(application, &backend);

    let settings = main.open_settings("library");
    settings.set_default_size(1080, 780);
    save(&settings, dir, &format!("settings-library{suffix}"));
    if let Some(skip) = named::<gtk::ToggleButton>(&settings, "copies-skip") {
        skip.set_active(true);
        save(&settings, dir, &format!("settings-library-skip{suffix}"));
        if let Some(add) = named::<gtk::ToggleButton>(&settings, "copies-add") {
            add.set_active(true);
        }
    }
    settings.close();

    main.open_page("tracks");
    save(&main, dir, &format!("tracks-badges{suffix}"));

    if let Some(copies) = main.open_copies(5) {
        spin(300);
        if let Some(row) = named::<gtk::Widget>(&copies, "copy-51")
            && let Some(tick) = named::<gtk::CheckButton>(&row, "trash-tick")
        {
            tick.set_active(true);
        }
        save(&copies, dir, &format!("copies{suffix}"));
        if let Some(button) = named::<gtk::Button>(&copies, "trash-copies") {
            button.emit_clicked();
            spin(300);
            if let Some(confirm) = toplevel("trash-confirm") {
                save(&confirm, dir, &format!("trash-confirm{suffix}"));
                if let Some(yes) = named::<gtk::Button>(&confirm, "trash-confirm-yes") {
                    yes.emit_clicked();
                }
            }
            spin(300);
            save(&copies, dir, &format!("copies-results{suffix}"));
        }
        copies.close();
    }

    for by in ["same-recording", "identical-files", "same-file"] {
        if let Some(finder) = main.open_find_duplicates(by) {
            spin(400);
            if by != "same-file"
                && let Some(mark) = named::<gtk::Button>(&finder, "mark")
            {
                mark.emit_clicked();
            }
            save(&finder, dir, &format!("find-duplicates-{by}{suffix}"));
        }
    }
    if let Some(finder) = toplevel("find-duplicates") {
        finder.close();
    }
    main.close();
}

fn shoot_tools(application: &gtk::Application, dir: &Path, suffix: &str) {
    let options = sample::SampleOptions {
        albums: 40,
        playing: None,
    };
    let backend = sample::backend(&options);
    let main = window(application, &backend);
    for (action, name) in [
        ("convert", "converter"),
        ("sync-export", "sync-export"),
        ("cover-manager", "cover-manager"),
    ] {
        gio::prelude::ActionGroupExt::activate_action(application, action, None);
        spin(500);
        if let Some(tool) = toplevel(name) {
            save(&tool, dir, &format!("{name}{suffix}"));
            if name == "converter"
                && let Some(convert) = named::<gtk::Button>(&tool, "convert")
            {
                convert.emit_clicked();
                spin(500);
                save(&tool, dir, &format!("{name}-done{suffix}"));
                convert.emit_clicked();
                spin(300);
            }
            tool.close();
        }
    }
    main.close();
}

fn shoot_file_tools(application: &gtk::Application, dir: &Path, suffix: &str) {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 40,
        playing: None,
    });
    let main = window(application, &backend);
    if let Some(organizer) = main.open_organizer() {
        spin(300);
        if let Some(row) = named::<gtk::ListBoxRow>(&organizer, "pattern-2") {
            row.activate();
            if let Some(list) = row.parent().and_downcast::<gtk::ListBox>() {
                list.select_row(Some(&row));
            }
        }
        spin(500);
        save(&organizer, dir, &format!("organize-files{suffix}"));
        organizer.close();
    }
    if let Some(missing) = main.open_missing_files() {
        spin(500);
        save(&missing, dir, &format!("missing-files{suffix}"));
        missing.close();
    }
    if let Some(verify) = main.open_verify_files() {
        spin(800);
        save(&verify, dir, &format!("verify-files{suffix}"));
        verify.close();
    }
    main.close();
}

fn main() -> ExitCode {
    let mut out = PathBuf::from("target/screenshots");
    let dsp_only = std::env::args().any(|a| a == "--dsp");
    let copies_only = std::env::args().any(|a| a == "--copies");
    let tools_only = std::env::args().any(|a| a == "--tools");
    let files_only = std::env::args().any(|a| a == "--files");
    BACKDROP.store(
        std::env::args().any(|a| a == "--backdrop"),
        Ordering::Relaxed,
    );
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
        if tools_only {
            shoot_tools(application, &out, "");
            set_choice(SchemeChoice::Light, AccentChoice::Amber);
            spin(200);
            shoot_tools(application, &out, ".light");
            application.quit();
            return;
        }
        if files_only {
            shoot_file_tools(application, &out, "");
            set_choice(SchemeChoice::Light, AccentChoice::Amber);
            spin(200);
            shoot_file_tools(application, &out, ".light");
            application.quit();
            return;
        }
        if copies_only {
            shoot_copies(application, &out, "");
            set_choice(SchemeChoice::Light, AccentChoice::Amber);
            spin(200);
            shoot_copies(application, &out, ".light");
            application.quit();
            return;
        }
        if dsp_only {
            shoot_dsp(application, &out, "");
            set_choice(SchemeChoice::Light, AccentChoice::Amber);
            spin(200);
            shoot_dsp(application, &out, ".light");
            application.quit();
            return;
        }
        shoot(application, &out, "");
        set_choice(SchemeChoice::Light, AccentChoice::Amber);
        spin(200);
        shoot(application, &out, ".light");
        shoot_dsp(application, &out, ".light");
        shoot_copies(application, &out, ".light");
        set_choice(SchemeChoice::Dark, AccentChoice::Amber);
        spin(200);
        shoot_dsp(application, &out, "");
        shoot_copies(application, &out, "");
        application.quit();
    });
    application.run_with_args(&["dialogs"]);
    ExitCode::SUCCESS
}
