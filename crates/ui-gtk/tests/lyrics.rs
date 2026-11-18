// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Lyrics tab of the panel and Now Playing against the sample backend:
//! synced lines that follow the position and seek when activated, plain
//! lyrics, the empty state, Now Playing opening and closing, and no frames
//! drawn while nothing changes; the analyzer, which moves only while sound
//! comes, and the waveform of the seek bar.
//!
//! Like `widgets.rs`, this program has its own main function and runs only
//! under the headless compositor script; names on the command line pick tests:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test lyrics
//! ```
//!
//! With `HELIKODU_LYRICS_SHOTS=<folder>` it also writes the tab and the view,
//! dark and light, as PNG files there.

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
use helikodu_model::audio::ChannelPosition;
use helikodu_ui_gtk::backend::{
    AnalyzerFrame, Backend, ChannelLevel, PlayerRequest, Update, sample,
};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice, set_choice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display, snapshot};

/// Runs the main loop for `ms` milliseconds.
fn spin(ms: u64) {
    let context = glib::MainContext::default();
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Runs the main loop until `done` holds, for at most `seconds`.
fn wait_for(what: &str, seconds: u64, done: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(seconds);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        spin(10);
    }
}

/// A window over the sample library playing the third track of the first
/// album (synced lyrics) at 1:42, with the Lyrics tab shown.
fn window(application: &gtk::Application) -> (MainWindow, Backend) {
    let backend = sample::backend(&sample::SampleOptions {
        playing: Some((0, 2, 102)),
        ..sample::SampleOptions::default()
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    window.show_lyrics_tab();
    spin(150);
    (window, backend)
}

fn synced_lines_follow_and_seek(app: &gtk::Application) {
    let (window, backend) = window(app);
    wait_for("the lyrics", 2, || window.lyrics_tab_texts().0.len() == 15);
    let (lines, current, source) = window.lyrics_tab_texts();
    assert_eq!(lines[4], "Salt on the window, salt on my hands");
    assert_eq!(current, Some(4));
    assert_eq!(source, "Synced · .lrc file");
    // A seek moves the current line; before the first line there is none.
    backend.send(PlayerRequest::Seek(Duration::from_secs(60)));
    wait_for("the second line", 2, || {
        window.lyrics_tab_texts().1 == Some(1)
    });
    backend.send(PlayerRequest::Seek(Duration::from_secs(5)));
    wait_for("no line", 2, || window.lyrics_tab_texts().1.is_none());
    // Activating a line seeks to it.
    assert!(window.activate_lyric_line(9));
    wait_for("the activated line", 2, || {
        window.lyrics_tab_texts().1 == Some(9)
    });
    window.close();
}

fn plain_and_missing_lyrics(app: &gtk::Application) {
    let (window, backend) = window(app);
    wait_for("the lyrics", 2, || window.lyrics_tab_texts().0.len() == 15);
    // Track 2 has plain lyrics: text without a current line or seeking.
    backend.send(PlayerRequest::Previous);
    wait_for("plain lyrics", 2, || {
        window.lyrics_tab_texts().2 == "Not synced · from the tags of the file"
    });
    let (lines, current, _) = window.lyrics_tab_texts();
    assert_eq!(lines.len(), 15);
    assert_eq!(current, None);
    assert!(!window.activate_lyric_line(0));
    // Track 4 has none.
    backend.send(PlayerRequest::Next);
    backend.send(PlayerRequest::Next);
    wait_for("no lyrics", 2, || {
        window.lyrics_tab_texts().2 == "No lyrics"
    });
    assert!(window.lyrics_tab_texts().0.is_empty());
    window.close();
}

fn now_playing_opens_and_closes(app: &gtk::Application) {
    let (window, backend) = window(app);
    assert!(!window.now_playing_shown());
    WidgetExt::activate_action(&window, "win.now-playing", None).unwrap();
    assert!(window.now_playing_shown());
    wait_for("the view's lines", 2, || {
        window.now_playing_texts().len() > 20
    });
    let texts = window.now_playing_texts();
    assert_eq!(texts[2], "lyrics");
    assert!(
        texts[3].starts_with("Synced · .lrc file · offset 0.0 s"),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t == "> Salt on the window, salt on my hands"),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("Source: ") && t.ends_with("[active]"))
    );
    assert!(texts.iter().any(|t| t.starts_with("DSP: ")));
    // The offset, from the view's actions.
    WidgetExt::activate_action(&window, "win.lyrics-sooner", None).unwrap();
    wait_for("the offset", 2, || {
        window.now_playing_texts()[3].ends_with("offset +0.1 s")
    });
    // A line of the view seeks.
    assert!(window.activate_lyric_line(2));
    wait_for("the seek", 2, || {
        window
            .now_playing_texts()
            .iter()
            .any(|t| t == "> I count the boats, I count the gulls")
    });
    // The queue segment and back: nothing breaks without lyrics either.
    backend.send(PlayerRequest::Next);
    wait_for("the next track", 2, || {
        window.now_playing_texts()[2] == "none"
    });
    WidgetExt::activate_action(&window, "win.now-playing", None).unwrap();
    assert!(!window.now_playing_shown());
    window.close();
}

/// Paused, with the view and the tab open, the window draws nothing.
fn nothing_is_drawn_while_paused(app: &gtk::Application) {
    let (window, backend) = window(app);
    wait_for("the lyrics", 2, || window.lyrics_tab_texts().0.len() == 15);
    backend.send(PlayerRequest::PlayPause);
    WidgetExt::activate_action(&window, "win.now-playing", None).unwrap();
    spin(600);
    let frames = Rc::new(Cell::new(0u32));
    let clock = window.frame_clock().expect("a frame clock");
    let f = frames.clone();
    let id = clock.connect_after_paint(move |_| f.set(f.get() + 1));
    spin(1200);
    clock.disconnect(id);
    assert_eq!(frames.get(), 0, "frames drawn while paused");
    window.close();
}

/// A measurement of the analyzer: every band at `band` dB, both channels at
/// `rms` with peaks at `peak`.
fn frame(band: f32, rms: f32, peak: f32, live: bool) -> Update {
    let channel = |position| ChannelLevel {
        position,
        rms_db: rms,
        peak_db: peak,
    };
    Update::Analyzer(std::sync::Arc::new(AnalyzerFrame {
        bands: vec![band; 48],
        channels: vec![
            channel(ChannelPosition::FrontLeft),
            channel(ChannelPosition::FrontRight),
        ],
        live,
    }))
}

/// Counts the frames the window draws in `ms` milliseconds.
fn frames_in(window: &MainWindow, ms: u64) -> u32 {
    let frames = Rc::new(Cell::new(0u32));
    let clock = window.frame_clock().expect("a frame clock");
    let f = frames.clone();
    let id = clock.connect_after_paint(move |_| f.set(f.get() + 1));
    spin(ms);
    clock.disconnect(id);
    frames.get()
}

/// The analyzer shows what it is sent while its page is on screen: bars and
/// peak lines, the channel levels, "CLIP" at full scale. After the sound
/// stops the bars fall back, the peak lines follow after their hold, and then
/// nothing is drawn.
fn analyzer_moves_only_while_sound_comes(app: &gtk::Application) {
    let (backend, updates) = sample::backend_with_updates(&sample::SampleOptions {
        playing: Some((0, 2, 102)),
        ..sample::SampleOptions::default()
    });
    install_actions(app, &backend);
    let window = MainWindow::new(app, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(150);
    WidgetExt::activate_action(&window, "win.now-playing", None).unwrap();
    // Not on screen: what comes is not shown.
    updates.try_send(frame(-10.0, -12.0, -3.0, true)).unwrap();
    spin(100);
    let rest = |levels: &[(f32, f32)]| {
        levels
            .iter()
            .all(|&(bar, peak)| bar <= -60.0 && peak <= -60.0)
    };
    assert!(rest(&window.now_playing_spectrum()));

    // On screen, the sample player sends one measurement at once.
    window.show_now_playing_analyzer();
    wait_for("the first measurement", 2, || {
        !rest(&window.now_playing_spectrum())
    });
    let rows = window.now_playing_levels();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        ("\u{2212}14.2".to_owned(), "PEAK \u{2212}3.1 dB".to_owned())
    );

    updates.try_send(frame(-6.0, -9.0, 0.0, true)).unwrap();
    spin(50);
    let (bars, rows) = (window.now_playing_spectrum(), window.now_playing_levels());
    let moving = window.now_playing_analyzer_moving();
    assert!(
        bars.iter().all(|&(bar, peak)| bar == -6.0 && peak == -6.0),
        "{bars:?}"
    );
    assert_eq!(rows[1].1, "CLIP");
    assert!(moving);

    // The sound stops: everything comes to rest, then no frame is drawn.
    updates.try_send(frame(-96.0, -96.0, -96.0, false)).unwrap();
    spin(300);
    let bars = window.now_playing_spectrum();
    assert!(
        bars[0].0 < -6.0 && bars[0].0 > -60.0,
        "falls back: {:?}",
        bars[0]
    );
    assert_eq!(bars[0].1, -6.0, "the peak line holds");
    wait_for("the analyzer to rest", 6, || {
        !window.now_playing_analyzer_moving()
    });
    let (bars, rows) = (window.now_playing_spectrum(), window.now_playing_levels());
    assert!(rest(&bars));
    assert_eq!(rows[0].0, "\u{2212}\u{221e}");
    assert_eq!(frames_in(&window, 800), 0, "frames drawn while at rest");

    // Leaving the view forgets what it showed, and nothing more is shown.
    WidgetExt::activate_action(&window, "win.now-playing", None).unwrap();
    updates.try_send(frame(-10.0, -12.0, -3.0, true)).unwrap();
    spin(100);
    assert!(rest(&window.now_playing_spectrum()));
    window.close();
}

/// The seek bar draws the waveform of the playing track once it has one, and
/// the flat band again when nothing plays.
fn seek_bar_waveform(app: &gtk::Application) {
    let (backend, updates) = sample::backend_with_updates(&sample::SampleOptions {
        playing: Some((0, 2, 102)),
        ..sample::SampleOptions::default()
    });
    install_actions(app, &backend);
    let window = MainWindow::new(app, &backend, WindowStyle::Integrated);
    window.present();
    spin(150);
    assert!(!window.deck_has_waveform());
    let peaks: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
    updates
        .try_send(Update::Waveform(Some(std::sync::Arc::from(peaks))))
        .unwrap();
    wait_for("the waveform", 2, || window.deck_has_waveform());
    updates.try_send(Update::NowPlaying(None)).unwrap();
    wait_for("the flat band", 2, || !window.deck_has_waveform());
    window.close();
}

/// Writes the tab and the view, dark and light, when asked to.
/// The deck's artist and album links open their pages, the title opens the
/// album at the playing track, and right click on the title opens the
/// track's menu.
fn deck_links_navigate(app: &gtk::Application) {
    let (window, _backend) = window(app);
    wait_for("the deck's links", 2, || window.deck_links().len() >= 3);
    let links = window.deck_links();
    let find = |prefix: &str| {
        links
            .iter()
            .find(|n| n.starts_with(prefix))
            .cloned()
            .unwrap_or_else(|| panic!("no link {prefix}: {links:?}"))
    };
    let artist = find("Go to artist ");
    let album = find("Go to album ");
    let title = find("Show ");
    assert!(window.click_deck_link(&artist));
    wait_for("the artist page", 2, || {
        window.page_text().starts_with("Artist(")
    });
    assert!(window.click_deck_link(&album));
    wait_for("the album page", 2, || {
        window.page_text().starts_with("Album(")
    });
    window.go_back();
    assert!(window.click_deck_link(&title));
    wait_for("the playing track selected", 2, || {
        window
            .album_selected_track()
            .is_some_and(|t| title == format!("Show {t} in its album"))
    });
    assert!(window.page_text().starts_with("Album("));
    let menu = window.demo_deck_title_menu().unwrap();
    wait_for("the track menu", 2, || menu.is_visible());
    menu.popdown();
    spin(50);
    window.close();
}

fn screenshots(app: &gtk::Application) {
    let Some(folder) = std::env::var_os("HELIKODU_LYRICS_SHOTS").map(PathBuf::from) else {
        println!("screenshots: skipped; set HELIKODU_LYRICS_SHOTS");
        return;
    };
    std::fs::create_dir_all(&folder).unwrap();
    let (window, backend) = window(app);
    let shot = |name: &str| {
        spin(900);
        let path = folder.join(format!("{name}.png"));
        snapshot::save_png(&window, &path).unwrap();
        println!("wrote {}", path.display());
    };
    for (scheme, suffix) in [(SchemeChoice::Dark, "dark"), (SchemeChoice::Light, "light")] {
        set_choice(scheme, AccentChoice::Amber);
        shot(&format!("lyrics-{suffix}"));
        WidgetExt::activate_action(&window, "win.now-playing", None).unwrap();
        shot(&format!("now-playing-{suffix}"));
        WidgetExt::activate_action(&window, "win.now-playing", None).unwrap();
    }
    set_choice(SchemeChoice::Dark, AccentChoice::Amber);
    backend.send(PlayerRequest::Previous);
    shot("lyrics-plain-dark");
    backend.send(PlayerRequest::Next);
    backend.send(PlayerRequest::Next);
    shot("lyrics-none-dark");
    WidgetExt::activate_action(&window, "win.now-playing", None).unwrap();
    shot("now-playing-none-dark");
    set_choice(SchemeChoice::Light, AccentChoice::Amber);
    WidgetExt::activate_action(&window, "win.now-playing", None).unwrap();
    shot("lyrics-none-light");
    set_choice(SchemeChoice::Dark, AccentChoice::Amber);
    backend.send(PlayerRequest::Previous);
    WidgetExt::activate_action(&window, "win.now-playing", None).unwrap();
    window.show_now_playing_analyzer();
    shot("now-playing-analyzer-dark");
    window.close();
}

type Test = (&'static str, fn(&gtk::Application));

const TESTS: &[Test] = &[
    ("synced_lines_follow_and_seek", synced_lines_follow_and_seek),
    ("plain_and_missing_lyrics", plain_and_missing_lyrics),
    ("now_playing_opens_and_closes", now_playing_opens_and_closes),
    (
        "nothing_is_drawn_while_paused",
        nothing_is_drawn_while_paused,
    ),
    ("deck_links_navigate", deck_links_navigate),
    (
        "analyzer_moves_only_while_sound_comes",
        analyzer_moves_only_while_sound_comes,
    ),
    ("seek_bar_waveform", seek_bar_waveform),
    ("screenshots", screenshots),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("lyrics: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    // Translated pictures only when asked for: the checks expect English.
    if std::env::var_os("HELIKODU_LOCALEDIR").is_some() {
        helikodu_ui_gtk::i18n::init();
    }
    if gtk::init().is_err() {
        println!("lyrics: skipped, no display");
        return ExitCode::SUCCESS;
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_decoration_layout(Some("icon:minimize,maximize,close"));
    }
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.LyricsTests")
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
    application.run_with_args(&["lyrics"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("lyrics: {} passed", ran.borrow());
        ExitCode::SUCCESS
    } else {
        println!("lyrics: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
