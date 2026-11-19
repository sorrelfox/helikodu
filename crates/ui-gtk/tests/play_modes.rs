// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The play modes against the sample backend: the Playback menu with the modes
//! in its labels, the deck's shuffle and repeat buttons, stop after, the A-B
//! loop chip and its shortcuts, the sleep timer popover and chip, the queue
//! panel's selection as the cursor, the tag editor's Playback group, the play
//! button's menu, the deck's icon sizes under two icon themes, and no frames
//! drawn while paused with the chips shown. `HELIKODU_PLAYMODE_SHOTS=DIR`
//! saves screenshots, dark and light, to `DIR`.
//!
//! GTK needs a display and the main thread, so this program has its own main
//! function and runs only under the headless compositor script:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test play_modes
//! ```
//!
//! Arguments that do not start with `-` select tests by name.

#![forbid(unsafe_code)]
// A panic is how a test fails; these are tests without the test harness.
#![allow(clippy::unwrap_used)]

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_ui_gtk::backend::{AlbumQuery, Backend, PlayerRequest, SleepWhen, TrackKey, sample};
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

fn wait_for(what: &str, done: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(2);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        spin(10);
    }
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

/// The button whose label reads `text`.
fn button(root: &impl IsA<gtk::Widget>, text: &str) -> gtk::Button {
    descendants(root.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.label().as_deref() == Some(text))
        .unwrap_or_else(|| panic!("no button {text}"))
}

fn toplevel(name: &str) -> Option<gtk::Window> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .find(|w| w.widget_name() == name && w.is_visible())
}

/// The window over the sample backend, playing the first album's third
/// track at 1:42.
fn playing_window(application: &gtk::Application) -> (MainWindow, Backend) {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 40,
        playing: Some((0, 2, 102)),
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(200);
    (window, backend)
}

fn has(labels: &[String], wanted: &str) -> bool {
    labels.iter().any(|l| l == wanted)
}

fn play_mode_menus_and_deck_buttons(app: &gtk::Application) {
    let (window, _) = playing_window(app);
    let labels = window.playback_menu_labels();
    for wanted in [
        "Shuffle: Off",
        "Repeat: Off",
        "Stop after: Off",
        "Play only: Everything",
        "Previous album",
        "Set the end here",
        "Continue from the selected track",
        "Sleep timer…",
    ] {
        assert!(
            has(&labels, wanted),
            "the Playback menu has {wanted}: {labels:?}"
        );
    }
    app.activate_action("shuffle", Some(&"albums".to_variant()));
    wait_for("shuffle by albums", || {
        has(&window.playback_menu_labels(), "Shuffle: Albums")
    });
    let (shuffle, repeat) = window.deck_mode_buttons();
    assert_eq!(shuffle.tooltip_text().as_deref(), Some("Shuffle: albums"));
    assert!(shuffle.has_css_class("active"));
    assert_eq!(
        app.lookup_action("shuffle").and_then(|a| a.state()),
        Some("albums".to_variant())
    );
    // A click switches off, the next one back to the mode used last.
    shuffle.emit_clicked();
    wait_for("shuffle off", || !shuffle.has_css_class("active"));
    assert_eq!(shuffle.tooltip_text().as_deref(), Some("Shuffle: off"));
    shuffle.emit_clicked();
    wait_for("shuffle by albums again", || {
        shuffle.tooltip_text().as_deref() == Some("Shuffle: albums")
    });

    app.activate_action("repeat", Some(&"one".to_variant()));
    wait_for("repeat one", || {
        repeat.tooltip_text().as_deref() == Some("Repeat: this track")
    });
    assert_eq!(
        repeat.icon_name().as_deref(),
        Some("helikodu-repeat-one-symbolic")
    );
    assert_eq!(
        shuffle.icon_name().as_deref(),
        Some("helikodu-shuffle-symbolic"),
        "both mode buttons use the bundled icons"
    );

    app.activate_action("stop-after-mode", Some(&"album".to_variant()));
    wait_for("stop after the album", || {
        has(&window.playback_menu_labels(), "Stop after: This album")
    });
    // The coarse stop-after action follows too.
    assert_eq!(
        app.lookup_action("stop-after").and_then(|a| a.state()),
        Some(true.to_variant())
    );
    app.activate_action("consume", None);
    wait_for("consume", || {
        app.lookup_action("consume").and_then(|a| a.state()) == Some(true.to_variant())
    });
    window.destroy();
}

fn play_mode_loop_chip_and_shortcuts(app: &gtk::Application) {
    let (window, backend) = playing_window(app);
    assert_eq!(window.deck_chip_texts().0, None, "no loop, no chip");
    for (action, accel) in [
        ("app.loop-start", "<Control>bracketleft"),
        ("app.loop-end", "<Control>bracketright"),
        ("app.loop-clear", "<Control>backslash"),
    ] {
        let accels = app.accels_for_action(action);
        assert!(
            accels.iter().any(|a| a.as_str() == accel),
            "{action} has {accel}"
        );
    }
    app.activate_action("loop-start", None);
    wait_for("the loop's start", || window.deck_chip_texts().0.is_some());
    backend.send(PlayerRequest::Seek(Duration::from_secs(150)));
    app.activate_action("loop-end", None);
    wait_for("the loop chip", || {
        window.deck_chip_texts().0.as_deref() == Some("A 1:42 – B 2:30")
    });
    // Dragging a flag moves the loop.
    backend.send(PlayerRequest::SetLoop(Some((
        Duration::from_secs(60),
        Duration::from_secs(150),
    ))));
    wait_for("the moved loop", || {
        window.deck_chip_texts().0.as_deref() == Some("A 1:00 – B 2:30")
    });
    app.activate_action("loop-clear", None);
    wait_for("no loop", || window.deck_chip_texts().0.is_none());
    window.destroy();
}

fn play_mode_sleep_timer(app: &gtk::Application) {
    let (window, _) = playing_window(app);
    assert_eq!(window.deck_chip_texts().1, None, "no timer, no chip");
    WidgetExt::activate_action(&window, "win.sleep-timer", None).unwrap();
    spin(100);
    let popover = window.open_sleep_timer().unwrap();
    wait_for("the sleep popover", || popover.is_visible());
    let start = button(&popover, "Start");
    assert!(start.is_visible(), "without a timer the footer holds Start");
    button(&popover, "45 min").emit_clicked();
    start.emit_clicked();
    wait_for("the sleep chip", || {
        window
            .deck_chip_texts()
            .1
            .is_some_and(|t| t == "45:00" || t.starts_with("44:5"))
    });
    assert!(!start.is_visible());
    button(&popover, "+10 min").emit_clicked();
    wait_for("ten more minutes", || {
        window
            .deck_chip_texts()
            .1
            .is_some_and(|t| t.starts_with("55:") || t.starts_with("54:"))
    });
    button(&popover, "Turn off").emit_clicked();
    wait_for("no timer", || window.deck_chip_texts().1.is_none());
    popover.popdown();
    window.destroy();
}

/// A restored queue, paused on a track far down: the window selects that row
/// and scrolls to it, without playing or making it the cursor.
fn play_mode_start_selects_current(app: &gtk::Application) {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 40,
        playing: None,
    });
    backend.send(PlayerRequest::PlayTracks {
        tracks: (1..=80).map(TrackKey).collect(),
        start: 60,
        title: "Restored".into(),
        shuffle: false,
    });
    backend.send(PlayerRequest::PlayPause);
    install_actions(app, &backend);
    let window = MainWindow::new(app, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 700);
    window.present();
    let list = window.queue_list();
    let current = || {
        descendants(list.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::ListBoxRow>().ok())
            .find(|r| r.has_css_class("current"))
    };
    wait_for("the current row selected", || {
        current().is_some_and(|row| list.selected_row().as_ref() == Some(&row))
    });
    let adjustment = list
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_downcast::<gtk::ScrolledWindow>()
        .unwrap()
        .vadjustment();
    wait_for("the current row in view", || {
        let Some(bounds) = current().and_then(|row| row.compute_bounds(&list)) else {
            return false;
        };
        let top = f64::from(bounds.y());
        adjustment.value() > 0.0
            && top >= adjustment.value()
            && top + f64::from(bounds.height()) <= adjustment.value() + adjustment.page_size()
    });
    spin(100);
    assert!(!window.is_playing(), "selecting does not play");
    assert_eq!(
        window.queue_cursor(),
        None,
        "the selection at the start is no cursor"
    );
    window.destroy();
}

fn play_mode_queue_cursor(app: &gtk::Application) {
    let (window, backend) = playing_window(app);
    let list = window.queue_list();
    let entry_row = || {
        descendants(list.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::ListBoxRow>().ok())
            .filter(|r| r.widget_name().parse::<u64>().is_ok())
            .last()
            .unwrap()
    };
    let row = entry_row();
    let entry = row.widget_name().to_string();
    list.select_row(Some(&row));
    spin(50);
    // The queue is shown anew; the selection comes back from the player.
    backend.send(PlayerRequest::Enqueue {
        tracks: vec![TrackKey(1)],
        next: true,
    });
    wait_for("the cursor again", || {
        list.selected_row()
            .is_some_and(|r| r.widget_name() == entry.as_str())
    });
    window.destroy();
}

fn play_mode_follow_playing_is_remembered(app: &gtk::Application) {
    let (window, backend) = playing_window(app);
    let state = |w: &MainWindow| {
        w.lookup_action("follow-playing")
            .and_then(|a| a.state())
            .and_then(|s| s.get::<bool>())
    };
    assert_eq!(state(&window), Some(false));
    WidgetExt::activate_action(&window, "win.follow-playing", None).unwrap();
    assert_eq!(state(&window), Some(true));
    assert!(backend.ui_state().load().follow_playing);
    window.destroy();
    // The next window starts with it on.
    let again = MainWindow::new(app, &backend, WindowStyle::Integrated);
    assert_eq!(state(&again), Some(true));
    again.destroy();
}

fn play_mode_favourite_heart(app: &gtk::Application) {
    let (window, _) = playing_window(app);
    wait_for("the playlists", || !window.playlist_counts().is_empty());
    let favourites = || window.playlist_counts()[0].clone();
    // Favourites is first and built in.
    assert!(favourites().2, "{:?}", window.playlist_counts());
    let before = favourites().1;
    // The library's answer about the playing track has come.
    spin(100);
    let shown = window.deck_favourite();
    window.toggle_deck_favourite();
    wait_for("the heart", || window.deck_favourite() != shown);
    // Taken out (every entry of it) or added once.
    let changed = |now: usize, from: usize, adding: bool| {
        if adding { now == from + 1 } else { now < from }
    };
    wait_for("the count", || changed(favourites().1, before, !shown));
    let after = favourites().1;
    // And back.
    window.toggle_deck_favourite();
    wait_for("the heart again", || window.deck_favourite() == shown);
    wait_for("the count again", || changed(favourites().1, after, shown));
    window.destroy();
}

fn play_mode_track_flags(app: &gtk::Application) {
    let (window, backend) = playing_window(app);
    let library = backend.library();
    let album = library.album_keys(&AlbumQuery::default()).unwrap()[0];
    let track = library
        .album(album)
        .unwrap()
        .unwrap()
        .tracks()
        .next()
        .unwrap()
        .key;
    window.edit_tags(&[track]);
    wait_for("the tag editor", || toplevel("tag-editor").is_some());
    let editor = toplevel("tag-editor").unwrap();
    let group = || {
        descendants(editor.upcast_ref())
            .into_iter()
            .find(|w| w.widget_name() == "track-playback")
    };
    wait_for("the Playback group", || group().is_some());
    let group = group().unwrap();
    // The switch of the row that reads `text`.
    let switch = |text: &str| {
        descendants(&group)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Switch>().ok())
            .find(|s| {
                // The nearest ancestor with a label is the switch's row.
                let mut row = s.parent();
                while let Some(r) = row {
                    let labels: Vec<String> = descendants(&r)
                        .iter()
                        .filter_map(|w| w.downcast_ref::<gtk::Label>())
                        .map(|l| l.label().to_string())
                        .collect();
                    if !labels.is_empty() {
                        return labels.iter().any(|l| l == text);
                    }
                    row = r.parent();
                }
                false
            })
            .unwrap_or_else(|| panic!("no switch {text}"))
    };
    let switches = [
        switch("Leave out when shuffling"),
        switch("Play only when I start it myself"),
    ];
    assert_ne!(switches[0], switches[1]);
    wait_for("the flags", || switches[0].is_sensitive());
    switches[0].set_active(true);
    let services = backend.services();
    wait_for("the stored flag", || {
        services
            .track_flags(track)
            .is_some_and(|f| f.skip_in_shuffle && !f.manual_only)
    });
    editor.close();
    spin(50);
    window.destroy();
}

/// The labels of `menu` and its sections and submenus, depth first.
fn menu_labels(menu: &gio::MenuModel, out: &mut Vec<String>) {
    for i in 0..menu.n_items() {
        if let Some(label) = menu
            .item_attribute_value(i, gio::MENU_ATTRIBUTE_LABEL, None)
            .and_then(|v| v.get::<String>())
        {
            out.push(label);
        }
        for link in [gio::MENU_LINK_SECTION, gio::MENU_LINK_SUBMENU] {
            if let Some(inner) = menu.item_link(i, link) {
                menu_labels(&inner, out);
            }
        }
    }
}

/// The submenu of `menu` (or of one of its sections) labelled `label`, in the
/// program's language; a `{}` in `label` matches the rest of the label.
fn submenu_of(menu: &gio::MenuModel, label: &str) -> Option<gio::MenuModel> {
    // The label in the program's language; a `{}` stands for the mode's name.
    let translated = gettextrs::gettext(label);
    let (start, whole) = match translated.split_once("{}") {
        Some((start, _)) => (start.to_owned(), false),
        None => (translated, true),
    };
    for i in 0..menu.n_items() {
        let name = menu
            .item_attribute_value(i, gio::MENU_ATTRIBUTE_LABEL, None)
            .and_then(|v| v.get::<String>());
        let matches = name.as_deref().is_some_and(|n| {
            if whole {
                n == start
            } else {
                n.starts_with(&start)
            }
        });
        if matches {
            return menu.item_link(i, gio::MENU_LINK_SUBMENU);
        }
        if let Some(found) = menu
            .item_link(i, gio::MENU_LINK_SECTION)
            .and_then(|s| submenu_of(&s, label))
        {
            return Some(found);
        }
    }
    None
}

/// The play button's menu: Stop, the Stop after submenu, Sleep timer… and
/// Jump to the playing track; Stop after carries the mode as a radio choice.
fn play_mode_play_button_menu(app: &gtk::Application) {
    let (window, _) = playing_window(app);
    let popover = window
        .demo_play_menu()
        .downcast::<gtk::PopoverMenu>()
        .unwrap();
    let model = popover.menu_model().unwrap();
    let mut labels = Vec::new();
    menu_labels(&model, &mut labels);
    for wanted in [
        "Stop",
        "Stop after",
        "Off",
        "This track",
        "This album",
        "The queue",
        "Sleep timer…",
        "Jump to the playing track",
    ] {
        assert!(
            has(&labels, wanted),
            "the play menu has {wanted}: {labels:?}"
        );
    }
    assert!(
        !has(&labels, "Stop after this track"),
        "the check item gave way to the submenu"
    );
    popover.popdown();
    // The stop-after mark and the play button's name follow the mode.
    app.activate_action("stop-after-mode", Some(&"queue".to_variant()));
    let (shuffle, _) = window.deck_mode_buttons();
    let play = shuffle
        .parent()
        .and_then(|row| {
            descendants(&row)
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Button>().ok())
                .find(|b| b.has_css_class("hk-play"))
        })
        .unwrap();
    wait_for("the play button's note", || {
        play.tooltip_text()
            .is_some_and(|t| t.ends_with("stops at the end of the queue"))
    });
    app.activate_action("stop-after-mode", Some(&"off".to_variant()));
    wait_for("no note", || {
        play.tooltip_text().is_some_and(|t| !t.contains("stops"))
    });
    window.destroy();
}

/// Every deck icon is drawn at 16 px, the mode buttons with the bundled
/// icons, under the default theme and under Breeze.
fn play_mode_deck_icon_sizes(app: &gtk::Application) {
    let (window, _) = playing_window(app);
    let settings = gtk::Settings::default().unwrap();
    let theme = settings.gtk_icon_theme_name();
    for name in ["Adwaita", "breeze"] {
        settings.set_gtk_icon_theme_name(Some(name));
        spin(150);
        let (shuffle, repeat) = window.deck_mode_buttons();
        let row = shuffle.parent().unwrap();
        let images: Vec<gtk::Image> = descendants(&row)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Image>().ok())
            .filter(|i| i.is_mapped())
            .collect();
        assert!(images.len() >= 5, "the transport's icons under {name}");
        for image in &images {
            // The icon's size, whatever room the button gives the image.
            let (width, _, _, _) = image.measure(gtk::Orientation::Horizontal, -1);
            let (height, _, _, _) = image.measure(gtk::Orientation::Vertical, -1);
            assert_eq!(
                (width, height),
                (16, 16),
                "{:?} under {name}",
                image.icon_name()
            );
        }
        for (button, icon) in [
            (&shuffle, "helikodu-shuffle-symbolic"),
            (&repeat, "helikodu-repeat-symbolic"),
        ] {
            assert_eq!(button.icon_name().as_deref(), Some(icon), "under {name}");
        }
    }
    settings.set_gtk_icon_theme_name(theme.as_deref());
    window.destroy();
}

/// Paused, with the loop chip and a running sleep timer shown, the window
/// draws nothing; playing, the sleep chip counts down.
fn play_mode_chips_draw_nothing_while_paused(app: &gtk::Application) {
    let (window, backend) = playing_window(app);
    backend.send(PlayerRequest::SetLoop(Some((
        Duration::from_secs(69),
        Duration::from_secs(153),
    ))));
    backend.send(PlayerRequest::StartSleepTimer {
        length: Duration::from_secs(30 * 60),
        when: SleepWhen::AtOnce,
    });
    wait_for("both chips", || {
        let (a, b) = window.deck_chip_texts();
        a.is_some() && b.is_some()
    });
    let before = window.deck_chip_texts().1.unwrap();
    // The countdown runs on a timer of whole seconds: 1 to 2 s to the first.
    let end = Instant::now() + Duration::from_secs(3);
    while window.deck_chip_texts().1.as_ref() == Some(&before) {
        assert!(
            Instant::now() < end,
            "the sleep chip counts down while playing"
        );
        spin(50);
    }

    backend.send(PlayerRequest::PlayPause);
    spin(600);
    let paused = window.deck_chip_texts().1.unwrap();
    let frames = Rc::new(Cell::new(0u32));
    let clock = window.frame_clock().expect("a frame clock");
    let f = frames.clone();
    let id = clock.connect_after_paint(move |_| f.set(f.get() + 1));
    spin(2200);
    clock.disconnect(id);
    assert_eq!(frames.get(), 0, "frames drawn while paused");
    assert_eq!(window.deck_chip_texts().1.unwrap(), paused);
    // Playing again, the chip catches up at once.
    backend.send(PlayerRequest::PlayPause);
    wait_for("the chip caught up", || {
        window.deck_chip_texts().1.is_some_and(|t| t != paused)
    });
    backend.send(PlayerRequest::CancelSleepTimer);
    window.destroy();
}

/// Writes the deck with modes, the menus and the chips, dark and light, to
/// the folder `HELIKODU_PLAYMODE_SHOTS` names; `HELIKODU_ICON_THEME` picks
/// the icon theme.
fn play_mode_screenshots(app: &gtk::Application) {
    let Some(folder) = std::env::var_os("HELIKODU_PLAYMODE_SHOTS").map(PathBuf::from) else {
        println!("play_mode_screenshots: skipped; set HELIKODU_PLAYMODE_SHOTS");
        return;
    };
    std::fs::create_dir_all(&folder).unwrap();
    let settings = gtk::Settings::default().unwrap();
    let theme = std::env::var("HELIKODU_ICON_THEME").ok();
    if let Some(theme) = &theme {
        settings.set_gtk_icon_theme_name(Some(theme));
    }
    let tag = theme.map(|t| format!("-{t}")).unwrap_or_default();
    let (window, backend) = playing_window(app);
    app.activate_action("shuffle", Some(&"albums".to_variant()));
    app.activate_action("repeat", Some(&"one".to_variant()));
    app.activate_action("stop-after-mode", Some(&"album".to_variant()));
    backend.send(PlayerRequest::SetLoop(Some((
        Duration::from_secs(69),
        Duration::from_secs(153),
    ))));
    backend.send(PlayerRequest::StartSleepTimer {
        length: Duration::from_secs(30 * 60),
        when: SleepWhen::AtOnce,
    });
    // Paused, so that the shots do not move.
    backend.send(PlayerRequest::PlayPause);
    let shot_window = |name: &str| {
        spin(500);
        let path = folder.join(format!("{name}{tag}.png"));
        snapshot::save_png(&window, &path).unwrap();
        println!("wrote {}", path.display());
    };
    let shot_popover = |popover: &gtk::Popover, name: &str| {
        spin(500);
        let path = folder.join(format!("{name}{tag}.png"));
        snapshot::save_widget_png(popover, &path).unwrap();
        println!("wrote {}", path.display());
    };
    let menu_popover = |model: &gio::MenuModel| {
        let popover = gtk::PopoverMenu::from_model(Some(model));
        popover.add_css_class("hk-main-menu");
        popover.set_parent(&window);
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(40, 40, 1, 1)));
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_has_arrow(false);
        popover.popup();
        popover
    };
    let playback = window.playback_menu().unwrap();
    for (scheme, suffix) in [(SchemeChoice::Dark, "dark"), (SchemeChoice::Light, "light")] {
        set_choice(scheme, AccentChoice::Amber);
        shot_window(&format!("deck-modes-{suffix}"));
        let popover = window.open_sleep_timer().unwrap();
        shot_window(&format!("sleep-loop-{suffix}"));
        shot_popover(&popover, &format!("sleep-popover-{suffix}"));
        popover.popdown();
        let popover = window.demo_play_menu();
        shot_popover(&popover, &format!("play-menu-{suffix}"));
        popover.popdown();
        let popover = menu_popover(&playback);
        shot_popover(popover.upcast_ref(), &format!("playback-menu-{suffix}"));
        popover.popdown();
        popover.unparent();
        for (label, name) in [
            ("Shuffle: {}", "shuffle"),
            ("Repeat: {}", "repeat"),
            ("Stop after: {}", "stop-after"),
            ("Play only: {}", "play-only"),
            ("Loop a section", "loop"),
            ("Queue", "queue"),
        ] {
            let model = submenu_of(&playback, label).unwrap_or_else(|| panic!("no {label}"));
            let popover = menu_popover(&model);
            shot_popover(popover.upcast_ref(), &format!("menu-{name}-{suffix}"));
            popover.popdown();
            popover.unparent();
        }
    }
    set_choice(SchemeChoice::Dark, AccentChoice::Amber);
    backend.send(PlayerRequest::CancelSleepTimer);
    window.destroy();
}

type Test = (&'static str, fn(&gtk::Application));

const TESTS: &[Test] = &[
    (
        "play_mode_menus_and_deck_buttons",
        play_mode_menus_and_deck_buttons,
    ),
    (
        "play_mode_loop_chip_and_shortcuts",
        play_mode_loop_chip_and_shortcuts,
    ),
    ("play_mode_sleep_timer", play_mode_sleep_timer),
    ("play_mode_queue_cursor", play_mode_queue_cursor),
    (
        "play_mode_start_selects_current",
        play_mode_start_selects_current,
    ),
    ("play_mode_track_flags", play_mode_track_flags),
    ("play_mode_play_button_menu", play_mode_play_button_menu),
    ("play_mode_deck_icon_sizes", play_mode_deck_icon_sizes),
    (
        "play_mode_chips_draw_nothing_while_paused",
        play_mode_chips_draw_nothing_while_paused,
    ),
    ("play_mode_screenshots", play_mode_screenshots),
    (
        "play_mode_follow_playing_is_remembered",
        play_mode_follow_playing_is_remembered,
    ),
    ("play_mode_favourite_heart", play_mode_favourite_heart),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("play_modes: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    // Translated pictures only when asked for: the checks expect English.
    if std::env::var_os("HELIKODU_LOCALEDIR").is_some() {
        helikodu_ui_gtk::i18n::init();
    }
    if gtk::init().is_err() {
        println!("play_modes: skipped, no display");
        return ExitCode::SUCCESS;
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let chosen = |name: &str| filter.is_empty() || filter.iter().any(|f| name.contains(f.as_str()));
    let selected = TESTS.iter().filter(|(name, _)| chosen(name)).count();
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.PlayModeTests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let failures = Rc::new(RefCell::new(Vec::new()));
    let f = failures.clone();
    let names: Vec<&'static str> = TESTS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| chosen(name))
        .collect();
    application.connect_activate(move |application| {
        let _hold = application.hold();
        for (name, test) in TESTS {
            if !names.contains(name) {
                continue;
            }
            match catch_unwind(AssertUnwindSafe(|| test(application))) {
                Ok(()) => println!("test {name} ... ok"),
                Err(_) => {
                    println!("test {name} ... FAILED");
                    f.borrow_mut().push(*name);
                }
            }
        }
        application.quit();
    });
    application.run_with_args(&["play_modes"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("play_modes: {selected} passed");
        ExitCode::SUCCESS
    } else {
        println!("play_modes: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
