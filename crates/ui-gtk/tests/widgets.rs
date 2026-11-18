// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The main window, its views and the theme against the sample backend.
//!
//! GTK needs a display and runs on the main thread only, so this test program
//! has its own main function and runs only when `HELIKODU_GTK_TESTS` is set,
//! as `tests/headless.sh` does with a headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test widgets
//! ```
//!
//! Without it, the program says it skipped and succeeds, so that it never
//! opens windows on the developer's desktop. Arguments that do not start
//! with `-` select the tests whose names contain one of them:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test widgets -- signal
//! ```
//!
//! With `HELIKODU_SIGNAL_SHOTS=DIR`, the signal path tests also save the
//! window, dark and light, to PNG files in `DIR`.

#![forbid(unsafe_code)]
// A panic is how a test fails; these are tests without the test harness.
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_ui_gtk::backend::{
    AlbumKey, AlbumQuery, PlaybackState, PlayerRequest, SignalLamp, Update, sample,
};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice, set_choice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display, snapshot};

/// Runs the main loop for `ms` milliseconds.
fn spin(ms: u64) {
    let context = glib::MainContext::default();
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs the main loop until `done` holds, for at most two seconds.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(2);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        spin(10);
    }
}

fn window(
    application: &gtk::Application,
    style: WindowStyle,
) -> (MainWindow, helikodu_ui_gtk::backend::Backend) {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 300,
        playing: None,
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, style);
    window.set_default_size(1440, 900);
    window.present();
    spin(100);
    (window, backend)
}

fn first_album(backend: &helikodu_ui_gtk::backend::Backend) -> AlbumKey {
    backend
        .library()
        .album_keys(&AlbumQuery::default())
        .unwrap()[0]
}

fn the_album_grid_loads_lazily(app: &gtk::Application) {
    let (window, _) = window(app, WindowStyle::Integrated);
    assert_eq!(window.visible_view().as_deref(), Some("albums"));
    wait_for("the album keys", || window.album_grid_stats().0 == 300);
    wait_for("the first rows", || window.album_grid_stats().1 > 0);
    let (_, fetches) = window.album_grid_stats();
    assert!(
        fetches <= 3,
        "rows are fetched in batches, not one by one ({fetches} fetches)"
    );
    window.destroy();
}

fn the_album_page_and_the_history(app: &gtk::Application) {
    let (window, backend) = window(app, WindowStyle::Integrated);
    window.open_album(first_album(&backend));
    spin(50);
    assert_eq!(window.visible_view().as_deref(), Some("album"));
    window.go_back();
    assert_eq!(window.visible_view().as_deref(), Some("albums"));
    window.destroy();
}

fn the_panes_toggle(app: &gtk::Application) {
    let (window, _) = window(app, WindowStyle::Classic);
    assert_eq!(window.panes_shown(), (true, true));
    WidgetExt::activate_action(&window, "win.toggle-sidebar", None).unwrap();
    WidgetExt::activate_action(&window, "win.toggle-panel", None).unwrap();
    assert_eq!(window.panes_shown(), (false, false));
    WidgetExt::activate_action(&window, "win.toggle-sidebar", None).unwrap();
    assert_eq!(window.panes_shown(), (true, false));
    window.destroy();
}

fn the_scheme_follows_the_choice(app: &gtk::Application) {
    let (window, _) = window(app, WindowStyle::Integrated);
    set_choice(SchemeChoice::Light, AccentChoice::Blue);
    wait_for("the light scheme", || window.has_css_class("hk-light"));
    assert!(!window.has_css_class("hk-dark"));
    set_choice(SchemeChoice::Dark, AccentChoice::Amber);
    wait_for("the dark scheme", || window.has_css_class("hk-dark"));
    window.destroy();
}

fn playing_and_toasts(app: &gtk::Application) {
    let (window, backend) = window(app, WindowStyle::Integrated);
    backend.send(PlayerRequest::PlayAlbum {
        album: first_album(&backend),
        start: 1,
        shuffle: false,
    });
    spin(50);
    app.activate_action("next", None);
    app.activate_action("play-pause", None);
    spin(50);
    window.toast("Something happened", false);
    assert_eq!(window.toast_text().as_deref(), Some("Something happened"));
    window.destroy();
}

fn the_panes_are_remembered(app: &gtk::Application) {
    let (window, backend) = window(app, WindowStyle::Integrated);
    WidgetExt::activate_action(&window, "win.toggle-panel", None).unwrap();
    let state = backend.ui_state().load();
    assert!(!state.panes.queue_visible);
    assert!(state.panes.sidebar_visible);
    window.destroy();
}

fn play_modes_and_the_queue(app: &gtk::Application) {
    let (window, backend) = window(app, WindowStyle::Integrated);
    backend.send(PlayerRequest::PlayAlbum {
        album: first_album(&backend),
        start: 0,
        shuffle: false,
    });
    spin(50);
    app.activate_action("stop-after", None);
    spin(50);
    let state = app
        .lookup_action("stop-after")
        .and_then(|a| a.state())
        .and_then(|s| s.get::<bool>());
    assert_eq!(
        state,
        Some(true),
        "the stop-after action follows the player"
    );
    app.activate_action("clear-queue", None);
    spin(50);
    window.destroy();
}

fn the_window_renders_to_png(app: &gtk::Application) {
    let (window, _) = window(app, WindowStyle::Integrated);
    spin(300);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join("widgets-main.png");
    snapshot::save_png(&window, &path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 10_000);
    window.destroy();
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

/// Saves the window to `name`-dark.png and `name`-light.png in the folder
/// `HELIKODU_SIGNAL_SHOTS` names, if it is set.
fn shots(window: &MainWindow, name: &str) {
    let Some(dir) = std::env::var_os("HELIKODU_SIGNAL_SHOTS").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for (scheme, suffix) in [(SchemeChoice::Dark, "dark"), (SchemeChoice::Light, "light")] {
        set_choice(scheme, AccentChoice::Amber);
        spin(400);
        snapshot::save_png(window, &dir.join(format!("{name}-{suffix}.png"))).unwrap();
    }
    set_choice(SchemeChoice::Dark, AccentChoice::Amber);
}

/// The same for a popover, which has a surface of its own.
fn popover_shots(popover: &gtk::Popover, name: &str) {
    let Some(dir) = std::env::var_os("HELIKODU_SIGNAL_SHOTS").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for (scheme, suffix) in [(SchemeChoice::Dark, "dark"), (SchemeChoice::Light, "light")] {
        set_choice(scheme, AccentChoice::Amber);
        spin(400);
        snapshot::save_widget_png(popover, &dir.join(format!("{name}-{suffix}.png"))).unwrap();
    }
    set_choice(SchemeChoice::Dark, AccentChoice::Amber);
}

/// The window over the sample backend playing its first album, and the
/// sender through which the test plays the engine's part.
fn playing_window(application: &gtk::Application) -> (MainWindow, async_channel::Sender<Update>) {
    let (backend, updates) = sample::backend_with_updates(&sample::SampleOptions {
        albums: 40,
        playing: Some((0, 2, 102)),
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(200);
    (window, updates)
}

fn has(texts: &[String], wanted: &str) -> bool {
    texts.iter().any(|t| t == wanted)
}

fn the_signal_path_tab_follows_each_lamp(app: &gtk::Application) {
    let (window, updates) = playing_window(app);
    window.show_signal_path();
    spin(100);
    // The sample player reports a bit-perfect path when it starts.
    let texts = window.signal_path_texts();
    assert_eq!(texts[0], "Bit-perfect", "{texts:?}");
    assert_eq!(texts[1], "The samples reach the device unchanged.");
    assert!(
        has(&texts, "Source: FLAC · 96 kHz · 24-bit [End]"),
        "{texts:?}"
    );
    assert!(has(&texts, "Loudness: Off [Hollow]"), "{texts:?}");
    assert!(has(&texts, "Volume: fixed 100 % [Hollow]"), "{texts:?}");
    assert!(
        has(&texts, "Output format: 24-bit · untouched [Hollow]"),
        "{texts:?}"
    );
    assert!(has(&texts, "Device: Studio Interface [End]"), "{texts:?}");
    assert!(has(&texts, "Buffer: 184 of 200 ms"), "{texts:?}");
    assert!(has(&texts, "Underruns: 0"), "{texts:?}");
    assert_eq!(
        window.output_button_text(),
        ("Studio Interface".into(), "Bit-perfect · 96 / 24".into())
    );
    shots(&window, "signal-bit-perfect");

    // Each lamp the engine reports, with the format the device was opened
    // with on the output button.
    for (lamp, state, button, active) in [
        (
            SignalLamp::LosslessProcessed,
            "DSP active",
            "DSP active · 96 / 32",
            "Equalizer: EQ 5 filters · preamp -4.5 dB [Active]",
        ),
        (
            SignalLamp::Resampled,
            "DSP active",
            "DSP active · 48 / 32",
            "Resampler: resample 96 → 48 kHz (high) [Active]",
        ),
        (
            SignalLamp::Lossy,
            "Converted",
            "Converted · 44.1 / 16",
            "Output format: TPDF dither 16-bit · f32le → s16le [Active]",
        ),
    ] {
        let path = sample::signal_path(lamp);
        updates
            .try_send(Update::SignalPath(Some(Box::new(path))))
            .unwrap();
        let mut stats = sample::playback_stats();
        stats.buffered_ms = 192;
        stats.underruns = 3;
        updates.try_send(Update::Stats(stats)).unwrap();
        spin(100);
        let texts = window.signal_path_texts();
        assert_eq!(texts[0], state, "{lamp:?}: {texts:?}");
        assert!(has(&texts, active), "{lamp:?}: {texts:?}");
        assert!(has(&texts, "Buffer: 192 of 200 ms"), "{texts:?}");
        assert!(has(&texts, "Underruns: 3"), "{texts:?}");
        assert_eq!(window.output_button_text().1, button, "{lamp:?}");
        if lamp == SignalLamp::Resampled {
            assert_eq!(
                texts[1],
                "6 stages are active. Processing runs in 32-bit float."
            );
            assert!(has(&texts, "Device: Built-in Audio [End]"), "{texts:?}");
            shots(&window, "signal-dsp");
        }
        if lamp == SignalLamp::Lossy {
            assert!(texts[1].ends_with("fewer bits than the source has."));
            assert!(
                has(&texts, "Source: MP3 · 44.1 kHz · 32-bit float [End]"),
                "{texts:?}"
            );
        }
    }

    // PipeWire carries 24 bits exactly: the label says so.
    let mut path = sample::signal_path(SignalLamp::BitPerfect);
    path.device.exact_bits = Some(24);
    updates
        .try_send(Update::SignalPath(Some(Box::new(path))))
        .unwrap();
    spin(100);
    assert_eq!(window.signal_path_texts()[0], "Bit-perfect, 24-bit");
    assert_eq!(
        window.output_button_text().1,
        "Bit-perfect, 24-bit · 96 / 24"
    );

    // Nothing heard: the lamp is idle and only the device has a value.
    updates.try_send(Update::SignalPath(None)).unwrap();
    updates
        .try_send(Update::State(helikodu_ui_gtk::backend::PlaybackState::Idle))
        .unwrap();
    spin(100);
    let texts = window.signal_path_texts();
    assert_eq!(texts[0], "Idle");
    assert_eq!(texts[1], "Nothing is playing.");
    assert!(has(&texts, "Loudness:  [Hollow]"), "{texts:?}");
    assert!(has(&texts, "Buffer: "), "{texts:?}");
    window.destroy();
}

fn the_signal_path_tab_is_reachable_by_keyboard(app: &gtk::Application) {
    let (window, _) = playing_window(app);
    window.show_signal_path();
    spin(100);
    let all = descendants(window.upcast_ref());
    let tab = all
        .iter()
        .filter_map(|w| w.downcast_ref::<gtk::ToggleButton>())
        .find(|b| b.label().as_deref() == Some("Signal path"))
        .unwrap();
    assert!(tab.is_visible() && tab.is_focusable() && tab.is_active());
    let lyrics = all
        .iter()
        .filter_map(|w| w.downcast_ref::<gtk::ToggleButton>())
        .find(|b| b.label().as_deref() == Some("Lyrics"))
        .unwrap();
    assert!(!lyrics.is_visible(), "Lyrics comes later");
    // Ten rows, the stages that have a setting can be activated.
    let list = all
        .iter()
        .filter_map(|w| w.downcast_ref::<gtk::ListBox>())
        .find(|l| l.has_css_class("hk-path-list"))
        .unwrap();
    let rows: Vec<gtk::ListBoxRow> = (0..10).filter_map(|i| list.row_at_index(i)).collect();
    assert_eq!(rows.len(), 10);
    assert!(list.row_at_index(10).is_none());
    assert!(!rows[0].is_activatable(), "Source has no setting");
    assert!(
        rows[1..]
            .iter()
            .all(gtk::prelude::ListBoxRowExt::is_activatable)
    );
    assert!(rows.iter().all(|r| r.is_focusable()));
    let footer: Vec<String> = all
        .iter()
        .filter_map(|w| w.downcast_ref::<gtk::Button>())
        .filter(|b| b.is_mapped())
        .filter_map(|b| b.label().map(|l| l.to_string()))
        .collect();
    assert!(footer.iter().any(|l| l == "Audio settings"), "{footer:?}");
    assert!(footer.iter().any(|l| l == "Copy diagnostics"), "{footer:?}");
    window.destroy();
}

fn the_output_switcher_shows_capabilities(app: &gtk::Application) {
    let (window, _) = playing_window(app);
    let popover = window.open_output_switcher().unwrap();
    let labels = || -> Vec<String> {
        descendants(popover.upcast_ref())
            .iter()
            .filter_map(|w| w.downcast_ref::<gtk::Label>().map(|l| l.text().to_string()))
            .collect()
    };
    wait_for("the devices", || has(&labels(), "Studio Interface"));
    let texts = labels();
    assert!(
        has(&texts, "USB · UP TO 384 kHz / 32-BIT · EXCLUSIVE"),
        "rate, bits and exclusive use: {texts:?}"
    );
    assert!(
        has(&texts, "HDMI · UP TO 48 kHz / 32-BIT · EXCLUSIVE"),
        "{texts:?}"
    );
    let studio = descendants(popover.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| {
            b.has_css_class("hk-device-row") && b.tooltip_text().is_some_and(|t| t.contains("384"))
        })
        .unwrap();
    let tooltip = studio.tooltip_text().unwrap();
    assert!(tooltip.contains("Exclusive use: yes"), "{tooltip}");
    assert!(tooltip.contains("s16le, s32le"), "{tooltip}");
    // The readout: the lamp's state and the path in two lines.
    assert!(has(&texts, "Bit-perfect"), "{texts:?}");
    assert!(
        has(&texts, "FLAC · 96 kHz · 24-bit → Studio Interface"),
        "{texts:?}"
    );
    assert!(has(&texts, "96 kHz / 24-bit, no processing"), "{texts:?}");
    spin(200);
    popover_shots(&popover, "output-switcher");
    popover.popdown();
    window.destroy();
}

/// Counts the frames the window paints in `ms` milliseconds.
fn frames_in(window: &MainWindow, ms: u64) -> u32 {
    let count = Rc::new(std::cell::Cell::new(0_u32));
    let clock = window.frame_clock().unwrap();
    let c = count.clone();
    let handler = clock.connect_after_paint(move |_| c.set(c.get() + 1));
    spin(ms);
    clock.disconnect(handler);
    count.get()
}

/// Saves the window to `name`.png in the folder `HELIKODU_INDICATOR_SHOTS`
/// names, if it is set.
fn indicator_shot(window: &MainWindow, name: &str) {
    if let Some(dir) = std::env::var_os("HELIKODU_INDICATOR_SHOTS").map(PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        snapshot::save_png(window, &dir.join(format!("{name}.png"))).unwrap();
    }
}

fn the_playing_indicator_moves_only_while_playing(app: &gtk::Application) {
    let (backend, updates) = sample::backend_with_updates(&sample::SampleOptions {
        albums: 40,
        playing: Some((0, 2, 102)),
    });
    install_actions(app, &backend);
    let window = MainWindow::new(app, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(200);
    window.open_album(first_album(&backend));
    updates
        .send_blocking(Update::State(PlaybackState::Playing))
        .unwrap();
    spin(300);
    assert!(
        frames_in(&window, 500) > 5,
        "the bars move while playback runs"
    );
    for i in 0..3 {
        indicator_shot(&window, &format!("indicator-playing-{i}"));
        spin(90);
    }
    updates
        .send_blocking(Update::State(PlaybackState::Paused))
        .unwrap();
    spin(300);
    indicator_shot(&window, "indicator-paused");
    assert_eq!(
        frames_in(&window, 1500),
        0,
        "a paused player draws no frames"
    );
    window.destroy();
}

fn dropdowns_look_like_menus(app: &gtk::Application) {
    let (window, _) = window(app, WindowStyle::Integrated);
    for page in ["appearance", "advanced"] {
        let settings = window.open_settings(page);
        spin(300);
        let dropdown = descendants(settings.upcast_ref())
            .into_iter()
            .filter(WidgetExt::is_mapped)
            .find_map(|w| w.downcast::<gtk::DropDown>().ok())
            .unwrap();
        let toggle = descendants(dropdown.upcast_ref())
            .into_iter()
            .find_map(|w| w.downcast::<gtk::ToggleButton>().ok())
            .unwrap();
        toggle.set_active(true);
        spin(400);
        let popover = descendants(dropdown.upcast_ref())
            .into_iter()
            .find_map(|w| w.downcast::<gtk::Popover>().ok())
            .unwrap();
        assert!(popover.is_visible(), "the list is open");
        assert!(popover.has_css_class("menu"));
        let list = descendants(popover.upcast_ref())
            .into_iter()
            .find(|w| w.css_name() == "listview")
            .unwrap();
        assert!(
            popover.child().unwrap().height() - list.height() <= 14,
            "no empty strip under the rows"
        );
        popover_shots(&popover, &format!("dropdown-{page}"));
        popover.popdown();
        settings.destroy();
    }
    window.destroy();
}

type Test = (&'static str, fn(&gtk::Application));

const TESTS: &[Test] = &[
    ("the_album_grid_loads_lazily", the_album_grid_loads_lazily),
    (
        "the_album_page_and_the_history",
        the_album_page_and_the_history,
    ),
    ("the_panes_toggle", the_panes_toggle),
    (
        "the_scheme_follows_the_choice",
        the_scheme_follows_the_choice,
    ),
    ("playing_and_toasts", playing_and_toasts),
    ("the_panes_are_remembered", the_panes_are_remembered),
    ("play_modes_and_the_queue", play_modes_and_the_queue),
    ("the_window_renders_to_png", the_window_renders_to_png),
    (
        "the_signal_path_tab_follows_each_lamp",
        the_signal_path_tab_follows_each_lamp,
    ),
    (
        "the_signal_path_tab_is_reachable_by_keyboard",
        the_signal_path_tab_is_reachable_by_keyboard,
    ),
    (
        "the_output_switcher_shows_capabilities",
        the_output_switcher_shows_capabilities,
    ),
    (
        "the_playing_indicator_moves_only_while_playing",
        the_playing_indicator_moves_only_while_playing,
    ),
    ("dropdowns_look_like_menus", dropdowns_look_like_menus),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("widgets: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("widgets: skipped, no display");
        return ExitCode::SUCCESS;
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let selected = TESTS
        .iter()
        .filter(|(name, _)| filter.is_empty() || filter.iter().any(|f| name.contains(f.as_str())))
        .count();
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.Tests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let failures = Rc::new(RefCell::new(Vec::new()));
    let f = failures.clone();
    application.connect_activate(move |application| {
        let _hold = application.hold();
        for (name, test) in TESTS {
            if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
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
        }
        application.quit();
    });
    application.run_with_args(&["widgets"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("widgets: {selected} passed");
        ExitCode::SUCCESS
    } else {
        println!("widgets: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
