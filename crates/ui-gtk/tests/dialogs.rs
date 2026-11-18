// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialogs against the sample backend: settings that round-trip through
//! the store, the full-output confirmation (also for requests from other
//! programs), the tag editor with mixed values, the first run, the output
//! switcher, Supported formats, About and Keyboard shortcuts.
//!
//! Like `widgets.rs`, this program runs only under the headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test dialogs
//! ```

#![forbid(unsafe_code)]
// A panic is how a test fails; these are tests without the test harness.
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_model::settings::VolumeControl;
use helikodu_ui_gtk::backend::{AlbumQuery, Backend, PlayerRequest, TrackKey, Update, sample};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display};

fn spin(ms: u64) {
    let context = glib::MainContext::default();
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for(what: &str, done: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(3);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        spin(10);
    }
}

fn options() -> sample::SampleOptions {
    sample::SampleOptions {
        albums: 40,
        playing: None,
    }
}

fn window_over(application: &gtk::Application, backend: Backend) -> (MainWindow, Backend) {
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(100);
    (window, backend)
}

fn window(application: &gtk::Application) -> (MainWindow, Backend) {
    window_over(application, sample::backend(&options()))
}

/// The open window called `name`.
fn toplevel(name: &str) -> Option<gtk::Window> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .find(|w| w.widget_name() == name && w.is_visible())
}

/// Every descendant of `root`.
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

fn named<T: IsA<gtk::Widget>>(root: &impl IsA<gtk::Widget>, name: &str) -> T {
    descendants(root.upcast_ref())
        .into_iter()
        .find(|w| w.widget_name() == name)
        .and_then(|w| w.downcast::<T>().ok())
        .unwrap_or_else(|| panic!("no widget {name}"))
}

/// The button whose label reads `text`.
fn button(root: &impl IsA<gtk::Widget>, text: &str) -> gtk::Button {
    descendants(root.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| {
            b.label().as_deref() == Some(text)
                || descendants(b.upcast_ref()).iter().any(|w| {
                    w.downcast_ref::<gtk::Label>()
                        .is_some_and(|l| l.text() == text)
                })
        })
        .unwrap_or_else(|| panic!("no button {text}"))
}

fn first_album_tracks(backend: &Backend, n: usize) -> Vec<TrackKey> {
    let library = backend.library();
    let key = library.album_keys(&AlbumQuery::default()).unwrap()[0];
    let album = library.album(key).unwrap().unwrap();
    album.tracks().take(n).map(|t| t.key).collect()
}

fn settings_round_trip(app: &gtk::Application) {
    let (window, backend) = window(app);
    let services = backend.services();
    assert_eq!(services.settings().playback.seek_step_s, 5);
    let settings = window.open_settings("playback");
    wait_for("the playback page", || {
        window.settings_page().as_deref() == Some("playback")
    });
    let step: gtk::DropDown = named(&settings, "seek-step");
    // 2, 5, 10, 15 or 30 seconds.
    step.set_selected(2);
    assert_eq!(services.settings().playback.seek_step_s, 10);
    let awake: gtk::Switch = named(&settings, "keep-awake");
    assert!(awake.is_active());
    awake.set_active(false);
    assert!(!services.settings().playback.keep_awake);
    settings.close();
    spin(50);
    // Opened again, the controls show the stored values.
    let settings = window.open_settings("playback");
    spin(50);
    let step: gtk::DropDown = named(&settings, "seek-step");
    assert_eq!(step.selected(), 2);
    let awake: gtk::Switch = named(&settings, "keep-awake");
    assert!(!awake.is_active());
    // A value the store refuses changes nothing.
    let refused = services.update_settings(&mut |s| s.output.buffer_ms = 10);
    assert!(refused.is_err());
    assert_eq!(services.settings().output.buffer_ms, 200);
    for page in ["library", "output", "loudness", "appearance", "advanced"] {
        window.open_settings(page);
        wait_for(page, || window.settings_page().as_deref() == Some(page));
    }
    settings.close();
    window.destroy();
}

fn the_full_output_confirmation(app: &gtk::Application) {
    let (window, backend) = window(app);
    let services = backend.services();
    services
        .update_settings(&mut |s| s.output.volume = VolumeControl::Fixed)
        .unwrap();
    let album = backend
        .library()
        .album_keys(&AlbumQuery::default())
        .unwrap()[0];
    let play = PlayerRequest::PlayAlbum {
        album,
        start: 0,
        shuffle: false,
    };
    // Cancel: nothing plays and nothing is remembered.
    backend.send(play.clone());
    wait_for("the question", || toplevel("volume-confirm").is_some());
    let dialog = toplevel("volume-confirm").unwrap();
    spin(50);
    assert!(!window.is_playing(), "nothing plays before the answer");
    let cancel = button(&dialog, "Cancel");
    assert_eq!(
        gtk::prelude::RootExt::focus(&dialog),
        Some(cancel.clone().upcast()),
        "Cancel has the focus"
    );
    cancel.emit_clicked();
    spin(50);
    assert!(toplevel("volume-confirm").is_none());
    assert!(!window.is_playing());
    assert!(
        services
            .settings()
            .output
            .profiles
            .values()
            .all(|p| !p.fixed_volume_confirmed)
    );
    // Yes: playback starts and the answer is kept for the device.
    backend.send(play.clone());
    wait_for("the question again", || {
        toplevel("volume-confirm").is_some()
    });
    button(&toplevel("volume-confirm").unwrap(), "Play at full output").emit_clicked();
    wait_for("playback", || window.is_playing());
    assert!(
        services
            .settings()
            .output
            .profiles
            .values()
            .any(|p| p.fixed_volume_confirmed)
    );
    // Asked once for each device.
    backend.send(PlayerRequest::Stop);
    spin(30);
    backend.send(play);
    spin(50);
    assert!(toplevel("volume-confirm").is_none());
    assert!(window.is_playing());
    window.destroy();
}

fn outside_requests_pass_the_gate(app: &gtk::Application) {
    let (backend, updates) = sample::backend_with_updates(&sample::SampleOptions {
        albums: 40,
        playing: Some((0, 0, 10)),
    });
    let (window, backend) = window_over(app, backend);
    wait_for("playback", || window.is_playing());
    backend.send(PlayerRequest::PlayPause);
    wait_for("the pause", || !window.is_playing());
    backend
        .services()
        .update_settings(&mut |s| s.output.volume = VolumeControl::Fixed)
        .unwrap();
    // Another program (MPRIS) asks to play: the window asks first.
    updates
        .try_send(Update::Request(PlayerRequest::Play))
        .unwrap();
    wait_for("the question", || toplevel("volume-confirm").is_some());
    spin(50);
    assert!(!window.is_playing(), "nothing plays before the answer");
    button(&toplevel("volume-confirm").unwrap(), "Cancel").emit_clicked();
    spin(50);
    assert!(!window.is_playing());
    // The same for PlayPause.
    updates
        .try_send(Update::Request(PlayerRequest::PlayPause))
        .unwrap();
    wait_for("the question again", || {
        toplevel("volume-confirm").is_some()
    });
    button(&toplevel("volume-confirm").unwrap(), "Play at full output").emit_clicked();
    wait_for("playback", || window.is_playing());
    window.destroy();
}

fn the_tag_editor_shows_mixed_values(app: &gtk::Application) {
    let (window, backend) = window(app);
    let tracks = first_album_tracks(&backend, 3);
    let services = backend.services();
    let before = services.edit_tracks(&tracks).unwrap();
    let album = before[0].fields[&helikodu_ui_gtk::backend::services::TagField::Album][0].clone();
    window.edit_tags(&tracks);
    wait_for("the tag editor", || toplevel("tag-editor").is_some());
    let editor = toplevel("tag-editor").unwrap();
    let entries: Vec<gtk::Entry> = descendants(editor.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Entry>().ok())
        .collect();
    let keep = entries
        .iter()
        .filter(|e| e.placeholder_text().as_deref() == Some("<keep>"))
        .count();
    assert!(keep >= 2, "titles and track numbers differ ({keep} kept)");
    let album_entry = entries
        .iter()
        .find(|e| e.text() == album)
        .expect("the shared album");
    let save = button(&editor, "Save 3 files");
    assert!(!save.is_sensitive(), "nothing to save yet");
    album_entry.set_text("Renamed Album");
    assert!(album_entry.has_css_class("edited"));
    assert!(save.is_sensitive());
    save.emit_clicked();
    wait_for("the save", || toplevel("tag-editor").is_none());
    let after = services.edit_tracks(&tracks).unwrap();
    for (old, new) in before.iter().zip(&after) {
        let field = helikodu_ui_gtk::backend::services::TagField::Album;
        assert_eq!(new.fields[&field], vec!["Renamed Album".to_owned()]);
        let title = helikodu_ui_gtk::backend::services::TagField::Title;
        assert_eq!(
            new.fields[&title], old.fields[&title],
            "untouched fields keep each file's value"
        );
    }
    wait_for("the toast", || {
        window
            .toast_text()
            .is_some_and(|t| t.contains("Saved 3 files"))
    });
    window.destroy();
}

fn the_first_run(app: &gtk::Application) {
    let (window, backend) = window_over(app, sample::first_run_backend(&options()));
    wait_for("the first run", || window.first_run_shown());
    let start = button(&window, "Scan and start");
    start.emit_clicked();
    spin(50);
    assert!(!window.first_run_shown());
    let settings = backend.services().settings();
    assert!(settings.library.watch);
    // A scan shows its progress in the sidebar and hides it at the end.
    window.show_scan_status(&helikodu_ui_gtk::backend::services::ScanStatus {
        done: 12,
        total: Some(100),
        ..Default::default()
    });
    assert!(window.scan_footer_shown());
    window.show_scan_status(&helikodu_ui_gtk::backend::services::ScanStatus {
        finished: true,
        ..Default::default()
    });
    assert!(!window.scan_footer_shown());
    window.destroy();
    // With a folder, the library opens at once.
    let (window, _) = window_over(app, sample::backend(&options()));
    spin(100);
    assert!(!window.first_run_shown());
    window.destroy();
}

fn the_output_switcher(app: &gtk::Application) {
    let (window, backend) = window(app);
    let popover = window.open_output_switcher().unwrap();
    wait_for("the devices", || {
        descendants(popover.upcast_ref()).iter().any(|w| {
            w.downcast_ref::<gtk::Label>()
                .is_some_and(|l| l.text() == "Studio Interface")
        })
    });
    button(&popover, "Studio Interface").emit_clicked();
    spin(50);
    let output = backend.services().output();
    assert_eq!(
        format!("{:?}", output.device),
        format!(
            "{:?}",
            helikodu_ui_gtk::backend::services::DeviceSelection::Device(
                helikodu_ui_gtk::backend::services::DeviceId::new(
                    "alsa_output.usb-Studio_Interface-00.analog-stereo"
                )
                .unwrap()
            )
        )
    );
    window.destroy();
}

fn formats_and_about(app: &gtk::Application) {
    let (window, _) = window(app);
    gio::prelude::ActionGroupExt::activate_action(app, "formats", None);
    wait_for("Supported formats", || toplevel("formats").is_some());
    gio::prelude::ActionGroupExt::activate_action(app, "about", None);
    wait_for("About", || toplevel("about").is_some());
    toplevel("about").unwrap().close();
    toplevel("formats").unwrap().close();
    window.destroy();
}

/// The keys the actions had before they could be changed in the settings.
const KEPT_SHORTCUTS: &[(&str, &str)] = &[
    ("app.next", "<Control>Right"),
    ("app.previous", "<Control>Left"),
    ("app.seek-forward", "<Control><Shift>Right"),
    ("app.seek-back", "<Control><Shift>Left"),
    ("app.quit", "<Control>q"),
    ("app.preferences", "<Control>comma"),
    ("app.shortcuts", "<Control>question"),
    ("app.mini-player", "<Control><Shift>m"),
    ("win.volume-up", "<Control>Up"),
    ("win.volume-down", "<Control>Down"),
    ("win.toggle-sidebar", "F9"),
    ("win.now-playing", "F11"),
    ("win.show-menubar", "<Control>m"),
    ("win.cycle-pane", "F6"),
    ("win.back", "<Alt>Left"),
    ("win.forward", "<Alt>Right"),
    ("win.jump-to-playing", "<Control>j"),
    ("win.search", "<Control>f"),
    ("win.track-properties", "<Alt>Return"),
    ("win.track-copies", "<Alt>c"),
    ("win.undo", "<Control>z"),
    ("win.redo", "<Control><Shift>z"),
    ("win.redo", "<Control>y"),
    ("win.select-all", "<Control>a"),
    ("app.new-playlist", "<Control>n"),
    ("app.loop-start", "<Control>bracketleft"),
    ("app.loop-end", "<Control>bracketright"),
    ("app.loop-clear", "<Control>backslash"),
];

fn keyboard_shortcuts(app: &gtk::Application) {
    let (window, _) = window(app);
    assert_eq!(
        app.accels_for_action("app.shortcuts")
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["<Control>question"]
    );
    // Every action that had keys before they could be changed still has them.
    let has = |action: &str, accel: &str| {
        app.accels_for_action(action)
            .iter()
            .any(|a| gtk::accelerator_parse(a.as_str()) == gtk::accelerator_parse(accel))
    };
    for (action, accel) in KEPT_SHORTCUTS {
        assert!(has(action, accel), "{action} has {accel}");
    }
    // A text field keeps select all, undo and redo for itself while it has
    // the focus; the window still lists them.
    let text = descendants(window.upcast_ref())
        .into_iter()
        .find_map(|w| w.downcast::<gtk::Text>().ok().filter(|t| t.is_mapped()))
        .expect("a text field");
    gtk::prelude::GtkWindowExt::set_focus(&window, Some(&text));
    spin(50);
    for (action, accel) in [
        ("win.undo", "<Control>z"),
        ("win.redo", "<Control><Shift>z"),
        ("win.redo", "<Control>y"),
        ("win.select-all", "<Control>a"),
    ] {
        assert!(!has(action, accel), "{accel} goes to the text field");
    }
    gio::prelude::ActionGroupExt::activate_action(app, "shortcuts", None);
    wait_for("Keyboard shortcuts", || toplevel("shortcuts").is_some());
    let dialog = toplevel("shortcuts").unwrap();
    let texts = |root: &gtk::Window| -> Vec<String> {
        descendants(root.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Label>().ok())
            .filter(|l| l.is_mapped())
            .map(|l| l.text().to_string())
            .collect()
    };
    spin(50);
    let shown = texts(&dialog);
    for expected in [
        "Next track",
        "Search",
        "Sidebar",
        "Ctrl",
        "→",
        "F9",
        "Not set",
    ] {
        assert!(
            shown.iter().any(|t| t == expected),
            "{expected} is shown: {shown:?}"
        );
    }
    let filter = descendants(dialog.upcast_ref())
        .into_iter()
        .find_map(|w| w.downcast::<gtk::SearchEntry>().ok())
        .unwrap();
    filter.set_text("f9");
    spin(400);
    let shown = texts(&dialog);
    assert!(shown.iter().any(|t| t == "Sidebar"));
    assert!(!shown.iter().any(|t| t == "Next track"), "{shown:?}");
    filter.set_text("ctrl+y");
    spin(400);
    let shown = texts(&dialog);
    assert!(shown.iter().any(|t| t == "Redo"), "{shown:?}");
    filter.set_text("undo");
    spin(400);
    let shown = texts(&dialog);
    assert!(
        shown.iter().any(|t| t == "Z") && !shown.iter().any(|t| t == "Not set"),
        "Undo keeps Ctrl+Z: {shown:?}"
    );
    dialog.close();
    gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
    spin(50);
    for (action, accel) in KEPT_SHORTCUTS {
        assert!(has(action, accel), "{action} has {accel} again");
    }
    window.destroy();
}

/// A widget called `name` exists under `root`.
fn has_named(root: &impl IsA<gtk::Widget>, name: &str) -> bool {
    descendants(root.upcast_ref())
        .iter()
        .any(|w| w.widget_name() == name)
}

fn dsp_editors_apply_live(app: &gtk::Application) {
    use helikodu_model::settings::{BuiltInPreset, ChannelMode, CrossfeedPreset, EqMode};
    let (window, backend) = window(app);
    let services = backend.services();
    let settings = window.open_dsp_stage("equalizer");
    wait_for("the DSP page", || {
        window.settings_page().as_deref() == Some("dsp")
    });
    let stack: gtk::Stack = named(&settings, "dsp-editors");
    assert_eq!(stack.visible_child_name().as_deref(), Some("equalizer"));
    // Every stage is off: the readout says the samples pass unchanged.
    let path: gtk::Label = named(&settings, "dsp-path");
    assert!(!path.text().contains("EQ"), "{}", path.text());
    let switch: gtk::Switch = named(&settings, "stage-switch-equalizer");
    switch.set_active(true);
    assert!(services.settings().dsp.equalizer.enabled);
    named::<gtk::Button>(&settings, "eq-add-filter").emit_clicked();
    assert_eq!(services.settings().dsp.equalizer.parametric.len(), 1);
    assert!(path.text().contains("EQ"), "{}", path.text());
    // Graphic, 10 bands: ten faders, a built-in preset, back to flat.
    button(&settings, "Graphic, 10 bands")
        .downcast::<gtk::ToggleButton>()
        .unwrap()
        .set_active(true);
    assert_eq!(services.settings().dsp.equalizer.mode, EqMode::Graphic10);
    spin(50);
    assert!(has_named(&settings, "eq-band-9"));
    assert!(!has_named(&settings, "eq-band-10"));
    let presets: gtk::DropDown = named(&settings, "eq-preset");
    let bass = BuiltInPreset::ALL
        .iter()
        .position(|p| p.id == "bass-boost")
        .unwrap();
    presets.set_selected(u32::try_from(bass).unwrap());
    assert_eq!(
        services.settings().dsp.equalizer.graphic_10,
        BuiltInPreset::ALL[bass].octaves
    );
    let preamp: gtk::Label = named(&settings, "eq-preamp");
    assert!(preamp.text().contains("6.0"), "{}", preamp.text());
    named::<gtk::Button>(&settings, "eq-flat").emit_clicked();
    assert_eq!(services.settings().dsp.equalizer.graphic_10, [0.0; 10]);
    button(&settings, "Graphic, 31 bands")
        .downcast::<gtk::ToggleButton>()
        .unwrap()
        .set_active(true);
    spin(50);
    assert!(has_named(&settings, "eq-band-30"));
    // The parametric filters were kept.
    assert_eq!(services.settings().dsp.equalizer.parametric.len(), 1);

    // Crossfeed: the chain selects the editor; a slider chooses Custom.
    named::<gtk::ToggleButton>(&settings, "stage-crossfeed").emit_clicked();
    assert_eq!(stack.visible_child_name().as_deref(), Some("crossfeed"));
    named::<gtk::CheckButton>(&settings, "crossfeed-2").set_active(true);
    assert_eq!(
        services.settings().dsp.crossfeed.preset,
        CrossfeedPreset::JanMeier
    );
    named::<gtk::Scale>(&settings, "crossfeed-level").set_value(8.0);
    spin(150);
    let crossfeed = services.settings().dsp.crossfeed;
    assert_eq!(crossfeed.preset, CrossfeedPreset::Custom);
    assert_eq!(crossfeed.level_db, 8.0);
    assert_eq!(crossfeed.cutoff_hz, 650);
    named::<gtk::Switch>(&settings, "stage-switch-crossfeed").set_active(true);
    assert!(path.text().contains("crossfeed"), "{}", path.text());

    // Channel mix and the limiter.
    let settings = window.open_dsp_stage("channel-mix");
    assert_eq!(stack.visible_child_name().as_deref(), Some("channel-mix"));
    button(&settings, "Mono")
        .downcast::<gtk::ToggleButton>()
        .unwrap()
        .set_active(true);
    assert_eq!(
        services.settings().dsp.channel_mix.channels,
        ChannelMode::Mono
    );
    named::<gtk::Switch>(&settings, "include-lfe").set_active(true);
    assert!(services.settings().dsp.channel_mix.include_lfe);
    window.open_dsp_stage("limiter");
    assert_eq!(stack.visible_child_name().as_deref(), Some("limiter"));
    named::<gtk::Scale>(&settings, "lookahead").set_value(3.5);
    spin(150);
    assert_eq!(services.settings().dsp.limiter.lookahead_ms, 3.5);
    assert_eq!(services.settings().validate(), Ok(()));
    settings.close();
    window.destroy();
}

fn m4_settings_controls(app: &gtk::Application) {
    use helikodu_model::settings::{ClippingPrevention, DsdOutput, ResamplerQuality};
    let (window, backend) = window(app);
    let services = backend.services();
    let settings = window.open_settings("playback");
    spin(50);
    // Crossfades: 2 s between tracks; fades from Off to 2 s.
    named::<gtk::DropDown>(&settings, "crossfade").set_selected(2);
    let transitions = services.settings().playback.transitions;
    assert!(transitions.crossfade);
    assert_eq!(transitions.crossfade_ms, 2000);
    named::<gtk::DropDown>(&settings, "pause-fade").set_selected(0);
    assert_eq!(services.settings().playback.transitions.pause_fade_ms, 0);
    named::<gtk::DropDown>(&settings, "crossfade").set_selected(0);
    assert!(!services.settings().playback.transitions.crossfade);
    // Looping tunes.
    named::<gtk::DropDown>(&settings, "loops").set_selected(2);
    assert_eq!(services.settings().playback.decoders.loops, 3);
    let soundfont: gtk::Label = named(&settings, "soundfont-path");
    assert!(soundfont.text().starts_with("None"), "{}", soundfont.text());

    window.open_settings("output");
    spin(50);
    named::<gtk::DropDown>(&settings, "dsd-output").set_selected(1);
    assert_eq!(services.settings().output.dsd, DsdOutput::Dop);
    named::<gtk::DropDown>(&settings, "resampler").set_selected(1);
    assert_eq!(services.settings().dsp.resampler, ResamplerQuality::Medium);
    named::<gtk::DropDown>(&settings, "exclusive-release").set_selected(1);
    assert_eq!(services.settings().output.exclusive_release_s, 0);

    window.open_settings("advanced");
    spin(50);
    named::<gtk::Switch>(&settings, "load-into-memory").set_active(true);
    assert!(services.settings().playback.decoders.load_into_memory);

    window.open_settings("loudness");
    spin(50);
    named::<gtk::CheckButton>(&settings, "clipping-limiter").set_active(true);
    assert_eq!(
        services.settings().loudness.clipping,
        ClippingPrevention::Limiter
    );
    named::<gtk::DropDown>(&settings, "write-measured").set_selected(1);
    assert!(services.settings().loudness.write_measured_to_tags);
    let count: gtk::Label = named(&settings, "loudness-count");
    wait_for("the loudness counts", || count.text().contains(" of "));
    // Measuring the rest ends with a toast; the button is ready again.
    let measure: gtk::Button = named(&settings, "measure-loudness");
    measure.emit_clicked();
    wait_for("the measurement", || {
        measure.is_sensitive() && measure.label().as_deref() == Some("Measure the rest")
    });
    assert_eq!(services.settings().validate(), Ok(()));
    settings.close();
    window.destroy();
}

fn formats_need_a_soundfont(app: &gtk::Application) {
    use helikodu_ui_gtk::backend::services::{Availability, FormatFamily};
    let (window, backend) = window(app);
    let services = backend.services();
    let midi = |services: &std::sync::Arc<dyn helikodu_ui_gtk::backend::services::Services>| {
        services
            .formats()
            .families
            .into_iter()
            .find(|f| f.family == FormatFamily::Midi)
            .unwrap()
            .availability
    };
    assert_eq!(midi(&services), Availability::NeedsSoundFont);
    gio::prelude::ActionGroupExt::activate_action(app, "formats", None);
    wait_for("Supported formats", || toplevel("formats").is_some());
    let formats = toplevel("formats").unwrap();
    assert!(has_named(&formats, "choose-soundfont"));
    formats.close();
    services
        .update_settings(&mut |s| {
            s.playback.decoders.soundfont = Some("/usr/share/soundfonts/default.sf2".into());
        })
        .unwrap();
    assert_eq!(midi(&services), Availability::Available);
    window.destroy();
}

/// The text of every label under `root`, joined.
fn texts(root: &impl IsA<gtk::Widget>) -> String {
    descendants(root.upcast_ref())
        .iter()
        .filter_map(|w| w.downcast_ref::<gtk::Label>().map(|l| l.text().to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Settings › Library: "Copies of the same file" and "Show one track for
/// identical copies" are stored, applied with their progress, and the second
/// is insensitive with a note while copies are skipped.
fn copies_settings_rows(app: &gtk::Application) {
    use helikodu_model::settings::DuplicateScan;
    let (window, backend) = window(app);
    let services = backend.services();
    let settings = window.open_settings("library");
    wait_for("the library page", || {
        window.settings_page().as_deref() == Some("library")
    });
    let add: gtk::ToggleButton = named(&settings, "copies-add");
    let skip: gtk::ToggleButton = named(&settings, "copies-skip");
    let show_one: gtk::Switch = named(&settings, "copies-show-one");
    let note: gtk::Label = named(&settings, "copies-show-one-note");
    let status: gtk::Label = named(&settings, "copies-status");
    assert!(add.is_active() && !skip.is_active());
    assert!(show_one.is_sensitive() && !note.is_visible());
    assert!(!show_one.is_active());

    skip.set_active(true);
    assert_eq!(
        services.settings().library.duplicates.on_scan,
        DuplicateScan::Skip
    );
    assert!(
        !show_one.is_sensitive(),
        "insensitive while copies are skipped"
    );
    assert!(note.is_visible(), "the note says why");
    wait_for("the change applied", || {
        status.text().starts_with("Applied")
    });
    assert!(status.is_visible());

    add.set_active(true);
    assert_eq!(
        services.settings().library.duplicates.on_scan,
        DuplicateScan::Add
    );
    assert!(show_one.is_sensitive() && !note.is_visible());
    show_one.set_active(true);
    assert!(services.settings().library.duplicates.show_one);
    wait_for("the second change applied", || {
        status.text().starts_with("Applied")
    });
    settings.close();
    spin(50);
    let settings = window.open_settings("library");
    wait_for("the library page again", || {
        window.settings_page().as_deref() == Some("library")
    });
    let show_one: gtk::Switch = named(&settings, "copies-show-one");
    assert!(show_one.is_active(), "the stored value shows again");
    window.destroy();
}

/// A copies badge in the track table names the copies and opens the copies
/// window; the context menu offers "Copies…" for the same track.
fn copies_badge_opens_the_window(app: &gtk::Application) {
    let (window, _backend) = window(app);
    window.open_page("tracks");
    wait_for("the track table", || window.track_table_stats().0 > 0);
    let badge = || {
        descendants(window.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.has_css_class("hk-copies-badge") && b.is_visible() && b.is_mapped())
    };
    wait_for("a copies badge", || badge().is_some());
    let badge = badge().unwrap();
    let tooltip = badge.tooltip_text().unwrap_or_default().to_string();
    assert!(
        tooltip.ends_with("identical copy") || tooltip.ends_with("identical copies"),
        "the tooltip names the copies: {tooltip}"
    );
    assert!(
        !badge.is_focusable(),
        "the rows keep their keyboard navigation"
    );
    let track = badge
        .action_target_value()
        .and_then(|v| v.get::<u64>())
        .unwrap();
    badge.emit_clicked();
    wait_for("the copies window", || toplevel("copies").is_some());
    let copies = toplevel("copies").unwrap();
    wait_for("its file rows", || {
        has_named(&copies, &format!("copy-{}", track * 10))
    });
    copies.close();
    // The keyboard path: Alt+C on the selected track, as the menu shows.
    let accels = app.accels_for_action("win.track-copies");
    assert!(accels.iter().any(|a| a == "<Alt>c"));
    window.destroy();
}

/// The copies window: the main copy changes with the radio and "Make main
/// copy"; a ticked copy goes to the trash only after the confirmation, and
/// each file's result is listed (the archive copy "changed" and stays).
fn copies_window_main_copy_and_trash(app: &gtk::Application) {
    use helikodu_ui_gtk::backend::services::FileId;
    let (window, backend) = window(app);
    let services = backend.services();
    // Track 5 has two copies and a second path to the main copy's file.
    let copies = window.open_copies(5).unwrap();
    wait_for("the rows", || has_named(&copies, "copy-51"));
    let row = |name: &str| -> gtk::ListBoxRow { named(&copies, name) };
    let radio = |name: &str| -> gtk::CheckButton { named(&row(name), "main-copy") };
    let tick = |name: &str| -> gtk::CheckButton { named(&row(name), "trash-tick") };
    let make_main: gtk::Button = named(&copies, "make-main-copy");
    let trash: gtk::Button = named(&copies, "trash-copies");
    assert!(radio("copy-50").is_active());
    assert!(
        !tick("copy-50").is_sensitive(),
        "the main copy can't be ticked"
    );
    assert!(
        !tick("copy-53").is_sensitive(),
        "nor a second path to a file"
    );
    assert!(!make_main.is_sensitive() && !trash.is_sensitive());

    radio("copy-51").set_active(true);
    assert!(make_main.is_sensitive());
    make_main.emit_clicked();
    wait_for("the new main copy", || {
        services
            .copies_of(TrackKey(5))
            .unwrap()
            .is_some_and(|g| g.main == FileId::from_raw(51))
    });
    // Shown again, the new main copy can't be ticked.
    wait_for("the rows again", || {
        has_named(&copies, "copy-51")
            && radio("copy-51").is_active()
            && !tick("copy-51").is_sensitive()
    });
    assert!(tick("copy-50").is_visible() && !make_main.is_sensitive());

    tick("copy-52").set_active(true);
    assert!(trash.is_sensitive());
    assert_eq!(trash.label().as_deref(), Some("Move 1 file to the trash…"));
    // Cancel leaves everything as it was.
    trash.emit_clicked();
    wait_for("the confirmation", || toplevel("trash-confirm").is_some());
    let confirm = toplevel("trash-confirm").unwrap();
    assert!(texts(&confirm).contains("/mnt/archive/Lossless"));
    assert!(texts(&confirm).contains("desktop Trash"));
    button(&confirm, "Cancel").emit_clicked();
    spin(50);
    assert!(toplevel("trash-confirm").is_none());
    assert_eq!(
        services
            .copies_of(TrackKey(5))
            .unwrap()
            .unwrap()
            .files
            .len(),
        4
    );
    // Confirmed, the archive copy is checked and reported as changed.
    trash.emit_clicked();
    wait_for("the confirmation again", || {
        toplevel("trash-confirm").is_some()
    });
    let confirm = toplevel("trash-confirm").unwrap();
    named::<gtk::Button>(&confirm, "trash-confirm-yes").emit_clicked();
    wait_for("the results", || has_named(&copies, "trash-results"));
    let results: gtk::Box = named(&copies, "trash-results");
    let text = texts(&results);
    assert!(text.contains("1 file left in place"), "{text}");
    assert!(text.contains("it changed"), "{text}");
    copies.close();

    // Track 2 has one copy; it goes, and no copy is left.
    let copies = window.open_copies(2).unwrap();
    wait_for("the rows of track 2", || has_named(&copies, "copy-21"));
    let tick: gtk::CheckButton = named(&named::<gtk::ListBoxRow>(&copies, "copy-21"), "trash-tick");
    tick.set_active(true);
    named::<gtk::Button>(&copies, "trash-copies").emit_clicked();
    wait_for("the confirmation", || toplevel("trash-confirm").is_some());
    let confirm = toplevel("trash-confirm").unwrap();
    named::<gtk::Button>(&confirm, "trash-confirm-yes").emit_clicked();
    wait_for("the results", || has_named(&copies, "trash-results"));
    assert!(
        texts(&named::<gtk::Box>(&copies, "trash-results")).contains("1 file moved to the trash")
    );
    assert!(services.copies_of(TrackKey(2)).unwrap().is_none());
    wait_for("no copy left", || texts(&copies).contains("no other copy"));
    window.destroy();
}

/// Find duplicates: the counts of the four ways, Mark by the keep rule
/// (never the main copy, nothing by hand), and files reached by two paths
/// listed but never markable.
fn copies_finder_marks(app: &gtk::Application) {
    let (window, _backend) = window(app);
    let finder = window.open_find_duplicates("identical-files").unwrap();
    let view: gtk::ListView = named(&finder, "duplicates-table");
    let rows = || view.model().map_or(0, |m| m.n_items());
    wait_for("the groups", || rows() > 0);
    let checks = || -> Vec<gtk::CheckButton> {
        descendants(view.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::CheckButton>().ok())
            .filter(|c| c.widget_name().starts_with("file-") && c.is_mapped())
            .collect()
    };
    wait_for("the rows drawn", || !checks().is_empty());
    let mark: gtk::Button = named(&finder, "mark");
    let trash: gtk::Button = named(&finder, "trash-duplicates");
    assert!(mark.is_sensitive());
    assert!(!trash.is_sensitive(), "nothing is marked at first");
    mark.emit_clicked();
    wait_for("the marks", || trash.is_sensitive());
    let label = trash.label().unwrap_or_default().to_string();
    assert!(
        label.starts_with("Move ") && label.ends_with(" to the trash"),
        "{label}"
    );
    for check in checks() {
        let id: i64 = check.widget_name()["file-".len()..].parse().unwrap();
        if id % 10 == 0 {
            assert!(
                !check.is_active() && !check.is_sensitive(),
                "the main copy {id} stays"
            );
        }
        if id % 10 == 1 {
            assert!(check.is_active(), "the copy {id} is marked");
        }
    }
    // Unticking a file updates the footer.
    let before = trash.label().unwrap_or_default().to_string();
    let ticked = checks().into_iter().find(|c| c.is_active()).unwrap();
    ticked.set_active(false);
    assert_ne!(trash.label().unwrap_or_default().to_string(), before);
    // By hand there is nothing to mark with.
    let keep: gtk::DropDown = named(&finder, "keep-rule");
    keep.set_selected(3);
    assert!(!mark.is_sensitive());
    keep.set_selected(0);
    assert!(mark.is_sensitive());
    // One file, two paths: listed, never markable.
    window.open_find_duplicates("same-file");
    wait_for("the same-file groups", || {
        !checks().is_empty() && !mark.is_sensitive()
    });
    spin(50);
    assert!(checks().iter().all(|c| !c.is_sensitive() && !c.is_active()));
    assert!(!trash.is_sensitive());
    window.destroy();
}

/// The copies window and Find duplicates draw no frames while nothing changes.
fn copies_windows_idle(app: &gtk::Application) {
    use std::cell::Cell;
    let (window, _backend) = window(app);
    let finder = window.open_find_duplicates("identical-files").unwrap();
    let view: gtk::ListView = named(&finder, "duplicates-table");
    wait_for("the groups", || {
        view.model().is_some_and(|m| m.n_items() > 0)
    });
    let copies = window.open_copies(5).unwrap();
    wait_for("the rows", || has_named(&copies, "copy-51"));
    spin(600);
    for shown in [&finder, &copies] {
        let frames = Rc::new(Cell::new(0u32));
        let clock = shown.frame_clock().expect("a frame clock");
        let f = frames.clone();
        let id = clock.connect_after_paint(move |_| f.set(f.get() + 1));
        spin(1200);
        clock.disconnect(id);
        assert_eq!(frames.get(), 0, "frames drawn while idle");
    }
    window.destroy();
}

fn label_texts(root: &impl IsA<gtk::Widget>) -> String {
    descendants(root.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Label>().ok())
        .filter(|l| l.is_mapped())
        .map(|l| l.text().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Organize files: the preview follows the pattern, a pattern that cannot be
/// read is reported, a run moves the ticked files, and undo and redo follow.
fn organizer_moves_and_undoes(app: &gtk::Application) {
    let (window, _backend) = window(app);
    let organizer = window.open_organizer().unwrap();
    let view: gtk::ListView = named(&organizer, "organize-table");
    let rows = || view.model().map_or(0, |m| m.n_items());
    wait_for("the preview", || rows() == 9);
    let pattern: gtk::Entry = named(&organizer, "organize-pattern");
    let run: gtk::Button = named(&organizer, "organize-run");
    pattern.set_text("{albumartist}/{year} {album}/[{disc}-]{track} {title}");
    wait_for("six files to move", || {
        run.label().as_deref() == Some("Move 6 files") && run.is_sensitive()
    });
    assert!(label_texts(&organizer).contains("Name taken"));
    assert!(label_texts(&organizer).contains("Already in place"));
    // A pattern that cannot be read.
    pattern.set_text("{album}/{nope}");
    let note: gtk::Label = named(&organizer, "organize-pattern-note");
    wait_for("the problem", || note.text().contains("nope"));
    assert!(!run.is_sensitive());
    pattern.set_text("{albumartist}/{year} {album}/[{disc}-]{track} {title}");
    wait_for("the preview again", || run.is_sensitive());
    run.emit_clicked();
    let undo: gtk::Button = named(&organizer, "organize-undo");
    wait_for("the run", || {
        window
            .toast_text()
            .is_some_and(|t| t.contains("6 files moved"))
    });
    wait_for("undo offered", || {
        undo.is_sensitive() && undo.label().as_deref() == Some("Undo the last run")
    });
    wait_for("nothing left to move", || {
        run.label().as_deref() == Some("Move 0 files")
    });
    undo.emit_clicked();
    wait_for("the undo", || {
        window
            .toast_text()
            .is_some_and(|t| t.contains("moved back"))
    });
    wait_for("redo offered", || {
        undo.label().as_deref() == Some("Redo the last run")
    });
    wait_for("six to move again", || {
        run.label().as_deref() == Some("Move 6 files")
    });
    organizer.close();
    window.destroy();
}

/// Missing files: found files are ticked and relinked; the others can be
/// removed, with Undo.
fn missing_files_relink(app: &gtk::Application) {
    let (window, _backend) = window(app);
    let missing = window.open_missing_files().unwrap();
    let view: gtk::ListView = named(&missing, "missing-table");
    let rows = || view.model().map_or(0, |m| m.n_items());
    wait_for("the missing tracks", || rows() == 11);
    let relink: gtk::Button = named(&missing, "missing-relink");
    wait_for("the found files", || {
        relink.label().as_deref() == Some("Relink 7 tracks")
    });
    let remove: gtk::Button = named(&missing, "missing-remove");
    assert_eq!(remove.label().as_deref(), Some("Remove 4 from the library"));
    relink.emit_clicked();
    wait_for("the relinked tracks gone", || rows() == 4);
    remove.emit_clicked();
    wait_for("the removed tracks gone", || rows() == 0);
    assert!(window.toast_text().is_some_and(|t| t.contains("removed")));
    gio::prelude::ActionGroupExt::activate_action(&window, "undo", None);
    wait_for("the undo", || rows() == 4);
    missing.close();
    window.destroy();
}

/// Verify files: a check of the whole library counts the results and lists
/// the problems with a sentence each.
fn verify_files_reports(app: &gtk::Application) {
    let (window, _backend) = window(app);
    let verify = window.open_verify_files().unwrap();
    let view: gtk::ListView = named(&verify, "verify-table");
    wait_for("the problems", || {
        view.model().is_some_and(|m| m.n_items() == 6)
    });
    wait_for("the end of the check", || {
        label_texts(&verify).contains("1,112 are fine")
    });
    let text = label_texts(&verify);
    assert!(text.contains("checksum"), "{text}");
    assert!(text.contains("Today at"), "{text}");
    let copy: gtk::Button = named(&verify, "verify-copy");
    assert!(copy.is_sensitive());
    verify.close();
    window.destroy();
}

type Test = (&'static str, fn(&gtk::Application));

/// Settings › Advanced: the library database with its backups, and the logs.
fn database_backups_and_logs(app: &gtk::Application) {
    let (window, _backend) = window(app);
    let settings = window.open_settings("advanced");
    wait_for("the advanced page", || {
        window.settings_page().as_deref() == Some("advanced")
    });
    let status: gtk::Label = named(&settings, "backups-status");
    assert!(
        status.text().contains("the last 7 are kept"),
        "{}",
        status.text()
    );
    assert!(status.text().contains("none yet"), "{}", status.text());
    let facts: gtk::Label = named(&settings, "database-facts");
    assert!(facts.text().contains("not checked yet"), "{}", facts.text());
    let back_up: gtk::Button = named(&settings, "back-up-now");
    back_up.emit_clicked();
    wait_for("the backup", || status.text().contains("latest today at"));
    assert!(
        facts.text().contains("no problems found"),
        "{}",
        facts.text()
    );
    assert!(back_up.is_sensitive());
    let _: gtk::Button = named(&settings, "restore-backup");
    let _: gtk::Button = named(&settings, "open-log-folder");
    let _ = button(&settings, "Copy diagnostics");
    settings.close();
    window.destroy();
}

const TESTS: &[Test] = &[
    ("database_backups_and_logs", database_backups_and_logs),
    ("settings_round_trip", settings_round_trip),
    ("the_full_output_confirmation", the_full_output_confirmation),
    (
        "outside_requests_pass_the_gate",
        outside_requests_pass_the_gate,
    ),
    (
        "the_tag_editor_shows_mixed_values",
        the_tag_editor_shows_mixed_values,
    ),
    ("the_first_run", the_first_run),
    ("the_output_switcher", the_output_switcher),
    ("formats_and_about", formats_and_about),
    ("keyboard_shortcuts", keyboard_shortcuts),
    ("dsp_editors_apply_live", dsp_editors_apply_live),
    ("m4_settings_controls", m4_settings_controls),
    ("formats_need_a_soundfont", formats_need_a_soundfont),
    ("copies_settings_rows", copies_settings_rows),
    (
        "copies_badge_opens_the_window",
        copies_badge_opens_the_window,
    ),
    (
        "copies_window_main_copy_and_trash",
        copies_window_main_copy_and_trash,
    ),
    ("copies_finder_marks", copies_finder_marks),
    ("copies_windows_idle", copies_windows_idle),
    ("organizer_moves_and_undoes", organizer_moves_and_undoes),
    ("missing_files_relink", missing_files_relink),
    ("verify_files_reports", verify_files_reports),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("dialogs: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("dialogs: skipped, no display");
        return ExitCode::SUCCESS;
    }
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.DialogTests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let failures = Rc::new(RefCell::new(Vec::new()));
    let f = failures.clone();
    application.connect_activate(move |application| {
        let _hold = application.hold();
        // Arguments after `--` keep the tests whose names contain one of them.
        let filters: Vec<String> = std::env::args()
            .skip(1)
            .filter(|a| !a.starts_with('-'))
            .collect();
        for (name, test) in TESTS {
            if !filters.is_empty() && !filters.iter().any(|f| name.contains(f.as_str())) {
                continue;
            }
            let result = catch_unwind(AssertUnwindSafe(|| test(application)));
            match result {
                Ok(()) => println!("test {name} ... ok"),
                Err(_) => {
                    println!("test {name} ... FAILED");
                    f.borrow_mut().push(*name);
                }
            }
            for window in gtk::Window::list_toplevels() {
                if let Ok(window) = window.downcast::<gtk::Window>() {
                    window.destroy();
                }
            }
            spin(20);
        }
        application.quit();
    });
    application.run_with_args(&["dialogs"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("dialogs: {} passed", TESTS.len());
        ExitCode::SUCCESS
    } else {
        println!("dialogs: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
