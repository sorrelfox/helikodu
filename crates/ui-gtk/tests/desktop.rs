// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Home, the mini player, the tray menu, pausing on lock and sleep,
//! Settings › Shortcuts and the settings search, against the sample backend.
//! The D-Bus services themselves are tested in `helikodu-desktop`; here the
//! window is fed what they report.
//!
//! Like `widgets.rs`, this program runs only under the headless compositor:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test desktop
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
use helikodu_ui_gtk::backend::{AlbumQuery, Backend, PlayerRequest, sample};
use helikodu_ui_gtk::snapshot;
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice, set_choice};
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

fn window(application: &gtk::Application) -> (MainWindow, Backend) {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 40,
        playing: None,
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(100);
    (window, backend)
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

fn mapped_texts(root: &impl IsA<gtk::Widget>) -> Vec<String> {
    descendants(root.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Label>().ok())
        .filter(|l| l.is_mapped())
        .map(|l| l.text().to_string())
        .collect()
}

fn button(root: &impl IsA<gtk::Widget>, text: &str) -> gtk::Button {
    descendants(root.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| {
            b.is_mapped()
                && (b.label().as_deref() == Some(text)
                    || descendants(b.upcast_ref()).iter().any(|w| {
                        w.downcast_ref::<gtk::Label>()
                            .is_some_and(|l| l.text() == text)
                    }))
        })
        .unwrap_or_else(|| panic!("no button {text}"))
}

fn play_first_album(backend: &Backend) {
    let key = backend
        .library()
        .album_keys(&AlbumQuery::default())
        .unwrap()[0];
    backend.send(PlayerRequest::PlayAlbum {
        album: key,
        start: 0,
        shuffle: false,
    });
}

fn accels(app: &gtk::Application, action: &str) -> Vec<String> {
    app.accels_for_action(action)
        .iter()
        .map(ToString::to_string)
        .collect()
}

fn home(app: &gtk::Application) {
    let (window, backend) = window(app);
    window.open_page("home");
    assert_eq!(window.visible_view().as_deref(), Some("home"));
    wait_for("Home", || {
        window
            .home_texts()
            .is_some_and(|(added, _, _, rediscover)| added > 0 && !rediscover.is_empty())
    });
    spin(200);
    let (added, played, most, rediscover) = window.home_texts().unwrap();
    // One row of tiles: as many as fit in the window, never more.
    assert!((4..=8).contains(&added), "{added} tiles");
    assert!(played <= added, "{played} played tiles");
    assert!(most.len() <= 5 && rediscover.len() <= 5);
    println!("  most played: {most:?}\n  rediscover: {rediscover:?}");
    // Playing a Most played row marks it: the indicator in place of the
    // rank, moving while playing and still while paused.
    let list = descendants(window.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::ListBox>().ok())
        // Rediscover opens on a single click; Most played plays on a double one.
        .find(|l| {
            l.has_css_class("hk-home-list") && l.is_mapped() && !l.activates_on_single_click()
        })
        .expect("the Most played list");
    let first = list.row_at_index(0).expect("a Most played row");
    list.emit_by_name::<()>("row-activated", &[&first]);
    wait_for("the Most played mark", || {
        window.playing_marks(false) == (1, vec![true])
    });
    backend.send(PlayerRequest::PlayPause);
    wait_for("the paused mark", || {
        window.playing_marks(false) == (1, vec![false])
    });
    // "History" opens the play history; Home stays selected.
    gio::prelude::ActionGroupExt::activate_action(&window, "show-history", None);
    spin(50);
    assert_eq!(window.visible_view().as_deref(), Some("history"));
    window.close();
}

fn mini_player(app: &gtk::Application) {
    let (window, backend) = window(app);
    gio::prelude::ActionGroupExt::activate_action(app, "mini-player", None);
    spin(100);
    assert!(
        !window.is_visible(),
        "the mini player takes the window's place"
    );
    let mini = window.mini_player_window().unwrap();
    assert!(mini.is_visible());
    let (width, _) = (mini.width(), mini.height());
    assert_eq!(width, 440);
    assert_eq!(window.mini_player_texts().unwrap().0, "Nothing playing");
    play_first_album(&backend);
    wait_for("the playing track", || {
        window
            .mini_player_texts()
            .is_some_and(|(title, ..)| title != "Nothing playing")
    });
    window.set_mini_player_drawer(true, "queue");
    spin(300);
    let (_, artist, note, rows) = window.mini_player_texts().unwrap();
    assert!(!artist.is_empty());
    assert!(note.contains("upcoming"), "{note}");
    assert!(rows.len() > 2, "{rows:?}");
    assert!(mini.height() > 400, "the drawer opens under the player");
    assert_eq!(
        window.mini_player_playing_marks(),
        (1, vec![true]),
        "the playing row of the queue is marked"
    );
    // The drawer is remembered.
    let state = backend.ui_state().load().mini_player;
    assert!(state.drawer);
    assert_eq!(state.tab, "queue");
    window.set_mini_player_drawer(true, "lyrics");
    spin(50);
    assert_eq!(backend.ui_state().load().mini_player.tab, "lyrics");
    // Ctrl+Shift+M again: back to the full window.
    assert_eq!(accels(app, "app.mini-player"), ["<Shift><Control>m"]);
    gio::prelude::ActionGroupExt::activate_action(app, "mini-player", None);
    spin(100);
    assert!(window.is_visible());
    assert!(!mini.is_visible());
    window.close();
}

fn tray_menu(app: &gtk::Application) {
    let (window, backend) = window(app);
    let labels = window.tray_menu_labels();
    assert_eq!(
        labels,
        [
            "Nothing playing",
            "-",
            "Play",
            "Next",
            "Previous",
            "Stop after this track",
            "-",
            "Hide the window",
            "Mini player",
            "-",
            "Quit"
        ]
    );
    play_first_album(&backend);
    wait_for("Pause in the menu", || {
        window.tray_menu_labels().get(3).map(String::as_str) == Some("Pause")
    });
    let labels = window.tray_menu_labels();
    assert!(labels[1].contains(" · "), "{labels:?}");
    window.demo_tray_action("window");
    spin(50);
    assert!(!window.is_visible());
    assert!(
        window
            .tray_menu_labels()
            .contains(&"Show the window".to_owned())
    );
    window.demo_tray_action("window");
    spin(50);
    assert!(window.is_visible());

    // The tray settings show only while a tray shows the icon.
    let settings = window.open_settings("appearance");
    spin(100);
    assert!(!mapped_texts(&settings).contains(&"Show an icon in the tray".to_owned()));
    window.demo_tray_host(true);
    spin(50);
    let texts = mapped_texts(&settings);
    assert!(texts.contains(&"Show an icon in the tray".to_owned()));
    assert!(texts.contains(&"Ctrl+Q always quits.".to_owned()));
    window.demo_tray_host(false);
    spin(50);
    assert!(!mapped_texts(&settings).contains(&"Show an icon in the tray".to_owned()));
    settings.close();

    // Closing keeps playing in the tray only when asked for and while a tray
    // shows the icon; the tray going away brings the window back.
    let services = backend.services();
    services
        .update_settings(&mut |s| {
            s.appearance.tray_icon = true;
            s.appearance.close_to_tray = true;
        })
        .unwrap();
    window.demo_tray_host(true);
    window.close();
    spin(50);
    assert!(!window.is_visible());
    assert!(window.kept_in_tray());
    window.demo_tray_host(false);
    spin(50);
    assert!(window.is_visible());
    services
        .update_settings(&mut |s| {
            s.appearance.tray_icon = false;
            s.appearance.close_to_tray = false;
        })
        .unwrap();
    window.close();
}

fn pause_on_lock_and_sleep(app: &gtk::Application) {
    let (window, backend) = window(app);
    let playing =
        |_: &Backend| window.tray_menu_labels().get(3).map(String::as_str) == Some("Pause");
    play_first_album(&backend);
    wait_for("playback", || playing(&backend));
    // Off by default: the lock changes nothing.
    window.demo_session_change("lock");
    spin(100);
    assert!(playing(&backend));
    window.demo_session_change("unlock");
    backend
        .services()
        .update_settings(&mut |s| s.playback.pause_on_lock = true)
        .unwrap();
    window.demo_session_change("lock");
    wait_for("the pause on lock", || !playing(&backend));
    window.demo_session_change("unlock");
    spin(50);
    assert!(!playing(&backend), "unlocking does not play again");
    backend.send(PlayerRequest::PlayPause);
    wait_for("playback again", || playing(&backend));
    // On by default: sleep pauses.
    window.demo_session_change("sleep");
    wait_for("the pause before sleep", || !playing(&backend));
    window.close();
}

fn shortcuts_page(app: &gtk::Application) {
    let (window, backend) = window(app);
    let settings = window.open_settings("shortcuts");
    spin(100);
    assert_eq!(window.settings_page().as_deref(), Some("shortcuts"));
    let texts = mapped_texts(&settings);
    for expected in [
        "Play or pause",
        "Shuffle",
        "Mini player",
        "Space",
        "Not set",
    ] {
        assert!(
            texts.contains(&expected.to_owned()),
            "{expected} in {texts:?}"
        );
    }
    // A free key is taken at once.
    assert!(accels(app, "app.toggle-shuffle").is_empty());
    assert!(window.demo_record_shortcut("app.toggle-shuffle", "<Control>h"));
    spin(50);
    assert_eq!(accels(app, "app.toggle-shuffle"), ["<Control>h"]);
    assert!(settings_keys(&backend, "app.toggle-shuffle") == Some(vec!["<Control>h".to_owned()]));
    // A key in use asks first.
    assert!(window.demo_record_shortcut("app.toggle-repeat", "<Control>j"));
    spin(50);
    assert!(mapped_texts(&settings).contains(&"Ctrl+J is already in use".to_owned()));
    assert!(accels(app, "app.toggle-repeat").is_empty());
    button(&settings, "Use it here").emit_clicked();
    spin(50);
    assert_eq!(accels(app, "app.toggle-repeat"), ["<Control>j"]);
    assert!(accels(app, "win.jump-to-playing").is_empty());
    // Backspace removes a shortcut; Esc keeps it.
    assert!(window.demo_record_shortcut("app.next", "Escape"));
    assert_eq!(accels(app, "app.next"), ["<Control>Right"]);
    assert!(window.demo_record_shortcut("app.next", "BackSpace"));
    spin(50);
    assert!(accels(app, "app.next").is_empty());
    window.demo_reset_shortcuts();
    spin(50);
    assert_eq!(accels(app, "app.next"), ["<Control>Right"]);
    assert_eq!(accels(app, "win.jump-to-playing"), ["<Control>j"]);
    assert!(accels(app, "app.toggle-shuffle").is_empty());
    settings.close();
    window.close();
}

fn settings_keys(backend: &Backend, action: &str) -> Option<Vec<String>> {
    backend
        .services()
        .settings()
        .shortcuts
        .keys
        .get(action)
        .cloned()
}

fn settings_search(app: &gtk::Application) {
    let (window, _) = window(app);
    let settings = window.open_settings("playback");
    spin(100);
    let entry = descendants(settings.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::SearchEntry>().ok())
        .find(|e| e.placeholder_text().as_deref() == Some("Search settings"))
        .unwrap();
    assert!(entry.is_sensitive());
    entry.set_text("tags");
    wait_for("the results", || {
        mapped_texts(&settings).contains(&"Results for “tags”".to_owned())
    });
    let texts = mapped_texts(&settings);
    assert!(
        texts.iter().any(|t| t.ends_with("· Esc goes back")),
        "{texts:?}"
    );
    assert!(texts.contains(&"Open the page".to_owned()));
    // A result works where it is shown: it is the setting itself.
    assert!(texts.iter().any(|t| t.contains("ratings")), "{texts:?}");
    entry.set_text("zzzz nothing");
    wait_for("no results", || {
        mapped_texts(&settings)
            .iter()
            .any(|t| t.starts_with("No settings match"))
    });
    entry.set_text("");
    wait_for("the page again", || {
        window.settings_page().as_deref() == Some("playback")
    });
    // "Open the page" goes to the result's page.
    entry.set_text("ID3");
    wait_for("the results", || {
        mapped_texts(&settings).contains(&"Open the page".to_owned())
    });
    button(&settings, "Open the page").emit_clicked();
    spin(50);
    assert_eq!(window.settings_page().as_deref(), Some("advanced"));
    assert!(entry.text().is_empty());
    settings.close();
    window.close();
}

/// Writes Home, the mini player with and without its drawer, Settings ›
/// Shortcuts and a settings search, dark and light, to the folder
/// `HELIKODU_DESKTOP_SHOTS` names.
fn desktop_screenshots(app: &gtk::Application) {
    let Some(folder) = std::env::var_os("HELIKODU_DESKTOP_SHOTS").map(std::path::PathBuf::from)
    else {
        println!("desktop_screenshots: skipped; set HELIKODU_DESKTOP_SHOTS");
        return;
    };
    std::fs::create_dir_all(&folder).unwrap();
    let (window, backend) = window(app);
    play_first_album(&backend);
    spin(300);
    backend.send(PlayerRequest::PlayPause);
    let shot = |target: &gtk::Window, name: &str| {
        spin(500);
        let path = folder.join(format!("{name}.png"));
        snapshot::save_png(target, &path).unwrap();
        println!("wrote {}", path.display());
    };
    for (scheme, suffix) in [(SchemeChoice::Dark, "dark"), (SchemeChoice::Light, "light")] {
        set_choice(scheme, AccentChoice::Amber);
        window.present();
        window.open_page("home");
        shot(window.upcast_ref(), &format!("home-{suffix}"));
        gio::prelude::ActionGroupExt::activate_action(app, "mini-player", None);
        let mini = window.mini_player_window().unwrap();
        window.set_mini_player_drawer(false, "queue");
        shot(&mini, &format!("mini-{suffix}"));
        window.set_mini_player_drawer(true, "queue");
        shot(&mini, &format!("mini-queue-{suffix}"));
        window.set_mini_player_drawer(true, "lyrics");
        shot(&mini, &format!("mini-lyrics-{suffix}"));
        window.set_mini_player_drawer(false, "queue");
        gio::prelude::ActionGroupExt::activate_action(app, "mini-player", None);
        spin(100);
        let settings = window.open_settings("shortcuts");
        window.demo_record_shortcut("app.toggle-shuffle", "<Control>h");
        window.demo_record_shortcut("app.toggle-repeat", "<Control>j");
        shot(&settings, &format!("settings-shortcuts-{suffix}"));
        window.demo_reset_shortcuts();
        let entry = descendants(settings.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::SearchEntry>().ok())
            .find(|e| e.placeholder_text().as_deref() == Some("Search settings"))
            .unwrap();
        entry.set_text("tags");
        shot(&settings, &format!("settings-search-{suffix}"));
        settings.close();
    }
    set_choice(SchemeChoice::Dark, AccentChoice::Amber);
    window.close();
}

type Test = fn(&gtk::Application);

const TESTS: &[(&str, Test)] = &[
    ("home", home),
    ("mini_player", mini_player),
    ("tray_menu", tray_menu),
    ("pause_on_lock_and_sleep", pause_on_lock_and_sleep),
    ("shortcuts_page", shortcuts_page),
    ("settings_search", settings_search),
    ("desktop_screenshots", desktop_screenshots),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("desktop: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("desktop: skipped, no display");
        return ExitCode::SUCCESS;
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.DesktopTests")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let failures = Rc::new(RefCell::new(Vec::new()));
    let ran = Rc::new(RefCell::new(0));
    let (f, r) = (failures.clone(), ran.clone());
    application.connect_activate(move |application| {
        let _hold = application.hold();
        for (name, test) in TESTS {
            if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
                continue;
            }
            *r.borrow_mut() += 1;
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
    application.run_with_args(&["desktop"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("desktop: {} passed", ran.borrow());
        ExitCode::SUCCESS
    } else {
        println!("desktop: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
