// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Renders the screenshots of the website (`docs/site`) over the sample
//! backend: the library views, the panel's tabs, Now Playing, the settings,
//! the tag editor, the tools and the mini player, in one colour scheme and at
//! the pixel scale asked for. `docs/site/shots.py` runs it once per scheme
//! under a headless compositor and turns the pictures into the files the
//! pages show. Run it by hand as the other examples:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo run -p helikodu-ui-gtk --example site_shots -- \
//!     --out target/site-shots --scheme dark --scale 2
//! ```
//!
//! Options: `--out DIR` (default `target/site-shots`), `--scheme dark|light`
//! (default dark), `--scale N` (default 1: N pixels for each logical pixel),
//! `--only NAME[,NAME…]` to render some of the pictures (the steps of the
//! others are left out) and `--store` for the pictures of app stores such as
//! Flathub: the windows drawn with their frame, the shadow and the rounded
//! corners. Each picture is written as `<name>.png`; the names
//! are printed as they are written.
//!
//! The windows are in the language of the environment where translations are
//! found: `HELIKODU_LOCALEDIR` names a folder of compiled translations and
//! `LANGUAGE` the language, as for the application.

#![forbid(unsafe_code)]

use std::cell::Cell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib, graphene};
use helikodu_model::settings::{FilterKind, ParametricFilter, VolumeControl};
use helikodu_ui_gtk::backend::services::AnalysisFacts;
use helikodu_ui_gtk::backend::{
    AlbumKey, AlbumQuery, Backend, MediaKind, OutputInfo, PlayerRequest, SignalLamp, SignalState,
    StageKind, TrackGrouping, TrackKey, TrackQuery, Update, sample,
};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice, set_choice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display};

/// Where the pictures go and how they are rendered.
struct Shots {
    out: PathBuf,
    scheme: SchemeChoice,
    scale: f32,
    only: Vec<String>,
    store: bool,
    failed: Cell<bool>,
}

impl Shots {
    fn wanted(&self, name: &str) -> bool {
        self.only.is_empty() || self.only.iter().any(|o| o == name)
    }

    /// Whether one of these pictures is wanted: whether their steps are worth
    /// taking.
    fn any(&self, names: &[&str]) -> bool {
        names.iter().any(|name| self.wanted(name))
    }

    /// Writes the texture of `node`, cut to `region` (in logical pixels).
    fn write(
        &self,
        name: &str,
        renderer: Option<gtk::gsk::Renderer>,
        snapshot: gtk::Snapshot,
        region: &graphene::Rect,
    ) {
        let result = (|| {
            let node = snapshot.to_node().ok_or("nothing was drawn")?;
            let renderer = renderer.ok_or("the window is not realised")?;
            let bounds = graphene::Rect::new(
                region.x() * self.scale,
                region.y() * self.scale,
                region.width() * self.scale,
                region.height() * self.scale,
            );
            renderer
                .render_texture(&node, Some(&bounds))
                .save_to_png(self.out.join(format!("{name}.png")))
                .map_err(|e| e.to_string())
        })();
        match result {
            Ok(()) => println!("{name}"),
            Err(error) => {
                eprintln!("site-shots: {name}: {error}");
                self.failed.set(true);
            }
        }
    }

    /// A snapshot that draws `widget` at the scale of the pictures.
    fn draw(&self, widget: &gtk::Widget) -> gtk::Snapshot {
        let snapshot = gtk::Snapshot::new();
        snapshot.scale(self.scale, self.scale);
        gtk::WidgetPaintable::new(Some(widget)).snapshot(
            &snapshot,
            f64::from(widget.width()),
            f64::from(widget.height()),
        );
        snapshot
    }

    /// The whole of `window`.
    fn window(&self, name: &str, window: &impl IsA<gtk::Window>) {
        if !self.wanted(name) {
            return;
        }
        let window = window.upcast_ref::<gtk::Window>();
        spin(400);
        let region = graphene::Rect::new(0.0, 0.0, window.width() as f32, window.height() as f32);
        let snapshot = self.draw(window.upcast_ref());
        if !self.store {
            self.write(name, window.renderer(), snapshot, &region);
            return;
        }
        // The window with its frame: what is drawn around it (the shadow),
        // as far as a shadow reaches.
        let Some(node) = snapshot.to_node() else {
            eprintln!("site-shots: {name}: nothing was drawn");
            self.failed.set(true);
            return;
        };
        let snapshot = gtk::Snapshot::new();
        self.frame(window, &snapshot);
        snapshot.append_node(&node);
        let Some(node) = snapshot.to_node() else {
            return;
        };
        let b = node.bounds();
        let drawn = graphene::Rect::new(
            b.x() / self.scale,
            b.y() / self.scale,
            b.width() / self.scale,
            b.height() / self.scale,
        );
        let framed = region
            .inset_r(-SHADOW, -SHADOW)
            .intersection(&drawn)
            .unwrap_or(region);
        let snapshot = gtk::Snapshot::new();
        snapshot.append_node(&node);
        self.write(name, window.renderer(), snapshot, &framed);
    }

    /// A part of `window`: `width` pixels from `x`, and the lowest `height`
    /// pixels, which hold the player deck and what is above it.
    fn foot(&self, name: &str, window: &impl IsA<gtk::Window>, x: f32, width: f32, height: f32) {
        if !self.wanted(name) {
            return;
        }
        spin(400);
        let window = window.upcast_ref::<gtk::Window>();
        let (w, h) = (window.width() as f32, window.height() as f32);
        let region = graphene::Rect::new(
            x.min(w),
            (h - height).max(0.0),
            width.min(w - x.min(w)),
            height.min(h),
        );
        self.write(
            name,
            window.renderer(),
            self.draw(window.upcast_ref()),
            &region,
        );
    }

    /// Draws the frame of `window` as GTK draws it on the desktop: the
    /// shadow and the outline of `window.csd` in GTK's own theme
    /// (`Default-light.css` and `Default-dark.css`), around the rounded
    /// corners. A window's paintable is cut to the window, so the frame,
    /// which lies outside, is not in it.
    fn frame(&self, window: &gtk::Window, snapshot: &gtk::Snapshot) {
        if !window.has_css_class("csd") || window.is_maximized() || window.is_fullscreen() {
            return;
        }
        let (top, bottom) = if window.has_css_class("hk-mini-window") {
            (14.0, 14.0)
        } else {
            (8.0, 0.0)
        };
        let outline = gtk::gsk::RoundedRect::new(
            graphene::Rect::new(0.0, 0.0, window.width() as f32, window.height() as f32),
            graphene::Size::new(top, top),
            graphene::Size::new(top, top),
            graphene::Size::new(bottom, bottom),
            graphene::Size::new(bottom, bottom),
        );
        let line = if self.scheme == SchemeChoice::Dark {
            0.75
        } else {
            0.23
        };
        snapshot.save();
        snapshot.scale(self.scale, self.scale);
        // `box-shadow: 0 3px 9px 1px rgba(0, 0, 0, 0.5), 0 0 0 1px …`: the
        // last shadow is drawn first.
        snapshot.append_outset_shadow(
            &outline,
            &gtk::gdk::RGBA::new(0.0, 0.0, 0.0, line),
            0.0,
            0.0,
            1.0,
            0.0,
        );
        snapshot.append_outset_shadow(
            &outline,
            &gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.5),
            0.0,
            3.0,
            1.0,
            9.0,
        );
        snapshot.restore();
    }

    /// An open popover alone, cut to its frame.
    fn popover(&self, name: &str, popover: &gtk::Popover) {
        if !self.wanted(name) {
            return;
        }
        spin(400);
        let widget = popover.upcast_ref::<gtk::Widget>();
        let region = popover
            .child()
            .and_then(|c| c.compute_bounds(widget))
            .map_or_else(
                || graphene::Rect::new(0.0, 0.0, widget.width() as f32, widget.height() as f32),
                |b| {
                    graphene::Rect::new(b.x() - 1.0, b.y() - 1.0, b.width() + 2.0, b.height() + 2.0)
                },
            );
        self.write(name, popover.renderer(), self.draw(widget), &region);
    }

    /// `window` with the open `popover` drawn over it where it is on screen.
    fn window_with(&self, name: &str, window: &MainWindow, popover: &gtk::Popover) {
        if !self.wanted(name) {
            return;
        }
        spin(400);
        let Some(popup) = popover.surface().and_downcast::<gtk::gdk::Popup>() else {
            eprintln!("site-shots: {name}: the popover is not shown");
            self.failed.set(true);
            return;
        };
        let (wx, wy) = window.surface_transform();
        let (px, py) = popover.surface_transform();
        let x = f64::from(popup.position_x()) + px - wx;
        let y = f64::from(popup.position_y()) + py - wy;
        let snapshot = self.draw(window.upcast_ref());
        snapshot.save();
        snapshot.translate(&graphene::Point::new(x as f32, y as f32));
        gtk::WidgetPaintable::new(Some(popover)).snapshot(
            &snapshot,
            f64::from(popover.width()),
            f64::from(popover.height()),
        );
        snapshot.restore();
        let region = graphene::Rect::new(0.0, 0.0, window.width() as f32, window.height() as f32);
        self.write(name, window.renderer(), snapshot, &region);
    }
}

/// The size of the main window, in logical pixels.
const MAIN_SIZE: (i32, i32) = (1440, 900);

/// How far around a window its shadow may reach, in logical pixels.
const SHADOW: f32 = 64.0;

/// Runs the main loop for `ms` milliseconds.
fn spin(ms: u64) {
    let context = glib::MainContext::default();
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs the main loop until `done` holds, for at most `ms` milliseconds;
/// whether it holds.
fn wait(ms: u64, done: impl Fn() -> bool) -> bool {
    let end = Instant::now() + Duration::from_millis(ms);
    while !done() {
        if Instant::now() >= end {
            return false;
        }
        spin(20);
    }
    true
}

/// Scrolls the page that holds the widget named `name` of `root` so that the
/// widget is near its top.
fn scroll_to(root: &impl IsA<gtk::Widget>, name: &str) {
    let Some(target) = named::<gtk::Widget>(root, name) else {
        return;
    };
    let mut scroller = target.parent();
    while let Some(widget) = scroller.clone() {
        if widget.is::<gtk::ScrolledWindow>() {
            break;
        }
        scroller = widget.parent();
    }
    let Some(scroller) = scroller.and_downcast::<gtk::ScrolledWindow>() else {
        return;
    };
    if let Some(point) = scroller
        .child()
        .and_then(|body| target.compute_point(&body, &graphene::Point::zero()))
    {
        scroller
            .vadjustment()
            .set_value(f64::from(point.y()) - 120.0);
        spin(300);
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

/// The shown window named `name`.
fn toplevel(name: &str) -> Option<gtk::Window> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .find(|w| w.widget_name() == name && w.is_visible())
}

/// The widget named `name` under `root`.
fn named<T: IsA<gtk::Widget>>(root: &impl IsA<gtk::Widget>, name: &str) -> Option<T> {
    descendants(root.upcast_ref())
        .into_iter()
        .find(|w| w.widget_name() == name)
        .and_then(|w| w.downcast::<T>().ok())
}

/// The toggle button of `root` labelled `text`: a tab or a segment.
fn toggle(root: &impl IsA<gtk::Widget>, text: &str) -> Option<gtk::ToggleButton> {
    descendants(root.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::ToggleButton>().ok())
        .find(|b| b.label().as_deref() == Some(text))
}

/// The shown button of `root` that says `text`.
fn button(root: &impl IsA<gtk::Widget>, text: &str) -> Option<gtk::Button> {
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
}

fn main_window(
    application: &gtk::Application,
    backend: &Backend,
    (width, height): (i32, i32),
) -> MainWindow {
    install_actions(application, backend);
    let window = MainWindow::new(application, backend, WindowStyle::Integrated);
    window.set_default_size(width, height);
    window.present();
    spin(900);
    window
}

/// The sample library with the third track of its first album playing at
/// 1:42: the track with synced lyrics.
fn playing(albums: usize) -> sample::SampleOptions {
    sample::SampleOptions {
        albums,
        playing: Some((0, 2, 102)),
    }
}

/// The library views, the panel's tabs, Now Playing and the deck's modes.
fn shoot_library(application: &gtk::Application, shots: &Shots) {
    const PICTURES: &[&str] = &[
        "albums",
        "accent-amber",
        "accent-blue",
        "accent-green",
        "accent-violet",
        "home",
        "album",
        "artists",
        "composers",
        "years",
        "folders",
        "tracks",
        "facets",
        "labels-popover",
        "tracks-grouped",
        "labels",
        "search",
        "query",
        "query-completion",
        "playlist",
        "smart-playlist-view",
        "smart-playlist",
        "history",
        "statistics",
        "lyrics",
        "signal-path",
        "now-playing",
        "analyzer",
        "output-switcher",
        "play-modes",
    ];
    if !shots.any(PICTURES) {
        return;
    }
    let backend = sample::backend(&playing(sample::SampleOptions::default().albums));
    let main = main_window(application, &backend, MAIN_SIZE);
    let library = backend.library();
    let albums = library
        .album_keys(&AlbumQuery::default())
        .unwrap_or_default();

    shots.window("albums", &main);
    // The middle of the deck in each accent colour: the seek bar and the
    // play button.
    for (accent, name) in [
        (AccentChoice::Amber, "accent-amber"),
        (AccentChoice::Blue, "accent-blue"),
        (AccentChoice::Green, "accent-green"),
        (AccentChoice::Violet, "accent-violet"),
    ] {
        if shots.wanted(name) {
            set_choice(shots.scheme, accent);
            spin(300);
            shots.foot(name, &main, 510.0, 420.0, 105.0);
        }
    }
    set_choice(shots.scheme, AccentChoice::Amber);

    if shots.wanted("home") {
        main.open_page("home");
        spin(600);
        shots.window("home", &main);
    }
    if shots.wanted("album")
        && let Some(&first) = albums.first()
    {
        main.open_album(first);
        spin(600);
        shots.window("album", &main);
    }
    if shots.wanted("artists") {
        main.open_page("artists");
        spin(600);
        shots.window("artists", &main);
    }
    if shots.wanted("composers") {
        main.open_page("composers");
        spin(300);
        main.browse_choose(false, 0);
        spin(600);
        shots.window("composers", &main);
    }
    if shots.wanted("years") {
        main.open_page("years");
        spin(300);
        main.browse_choose(true, 0);
        spin(600);
        shots.window("years", &main);
    }
    if shots.wanted("folders") {
        main.open_page("folders");
        for _ in 0..4 {
            main.demo_folder_step();
            spin(400);
        }
        shots.window("folders", &main);
    }
    if shots.any(&["tracks", "facets"]) {
        main.open_page("tracks");
        spin(600);
        main.demo_select_tracks(3, 3);
        shots.window("tracks", &main);
        if shots.wanted("facets") {
            main.set_browsing(true);
            spin(600);
            main.choose_facets(0, &[5]);
            spin(400);
            shots.window("facets", &main);
            main.set_browsing(false);
        }
    }
    // The Labels popover over an album's page, for three of its tracks: the
    // album's labels are given in each way, to some of the tracks, to the
    // album and to its artist.
    if shots.wanted("labels-popover") {
        main.open_album(AlbumKey(3));
        spin(600);
        let keys: Vec<TrackKey> = library
            .album(AlbumKey(3))
            .ok()
            .flatten()
            .map(|album| album.tracks().take(3).map(|t| t.key).collect())
            .unwrap_or_default();
        main.demo_labels_popover(keys);
        if wait(3000, || main.labels_popover_rows().len() > 2)
            && let Some(popover) = main.labels_popover()
        {
            main.labels_popover_type("night", false);
            spin(300);
            shots.window_with("labels-popover", &main, &popover);
            popover.popdown();
        } else {
            eprintln!("site-shots: labels-popover: the popover did not open");
            shots.failed.set(true);
        }
        spin(200);
    }
    // The track table grouped by one of the user's fields.
    if shots.wanted("tracks-grouped") {
        main.open_page("tracks");
        spin(400);
        main.group_tracks(Some(TrackGrouping::Field("decade".into())));
        wait(5000, || {
            main.track_groups()
                .iter()
                .any(|(value, _, _)| value.ends_with("0s"))
        });
        spin(400);
        shots.window("tracks-grouped", &main);
        main.group_tracks(None);
        spin(300);
    }
    if shots.wanted("labels") {
        main.open_page("labels");
        if wait(3000, || !main.label_list().is_empty()) {
            let late = main
                .label_list()
                .iter()
                .position(|(name, _)| name == "late night")
                .unwrap_or(0);
            main.choose_label(late);
            wait(3000, || {
                main.label_detail_texts()
                    .iter()
                    .any(|t| t.starts_with("track:"))
            });
            spin(400);
            shots.window("labels", &main);
        } else {
            eprintln!("site-shots: labels: the sample has no labels");
            shots.failed.set(true);
        }
    }
    if shots.any(&["search", "query", "query-completion"]) {
        main.search_text("harbour");
        spin(500);
        shots.window("search", &main);
        main.search_text("genre:jazz -genre:fusion");
        spin(500);
        shots.window("query", &main);
        if shots.wanted("query-completion")
            && let Some(popover) = main.demo_query_completion("genre:jazz a")
        {
            spin(300);
            shots.window_with("query-completion", &main, &popover);
            popover.popdown();
        }
        main.search_text("");
        gtk::prelude::GtkWindowExt::set_focus(&main, gtk::Widget::NONE);
    }
    if shots.wanted("playlist") && main.open_first_playlist() {
        spin(500);
        shots.window("playlist", &main);
    }
    if shots.wanted("smart-playlist-view") && main.open_playlist_named("Unrated hi-res", false) {
        spin(500);
        shots.window("smart-playlist-view", &main);
    }
    if shots.wanted("smart-playlist") && main.open_playlist_named("Unrated hi-res", true) {
        spin(500);
        shots.window("smart-playlist", &main);
    }
    if shots.wanted("history") {
        let _ = WidgetExt::activate_action(&main, "win.show-history", None);
        spin(500);
        shots.window("history", &main);
    }
    if shots.wanted("statistics") {
        let _ = WidgetExt::activate_action(&main, "win.show-statistics", None);
        spin(500);
        shots.window("statistics", &main);
    }
    if shots.any(&["lyrics", "signal-path"]) {
        main.open_page("albums");
        main.show_lyrics_tab();
        spin(500);
        shots.window("lyrics", &main);
        main.show_signal_path();
        spin(500);
        shots.window("signal-path", &main);
    }
    if shots.any(&["now-playing", "analyzer"]) {
        main.set_now_playing(true);
        spin(700);
        shots.window("now-playing", &main);
        main.show_now_playing_analyzer();
        spin(700);
        shots.window("analyzer", &main);
        main.set_now_playing(false);
        spin(300);
    }
    if shots.wanted("output-switcher")
        && let Some(popover) = main.open_output_switcher()
    {
        spin(300);
        shots.popover("output-switcher", &popover);
        popover.popdown();
    }
    // The deck with its modes on: albums shuffled, an A-B loop and a sleep
    // timer.
    if shots.wanted("play-modes") {
        main.open_page("albums");
        application.activate_action("shuffle", Some(&"albums".to_variant()));
        application.activate_action("repeat", Some(&"album".to_variant()));
        backend.send(PlayerRequest::SetLoop(Some((
            Duration::from_secs(60),
            Duration::from_secs(150),
        ))));
        if let Some(popover) = main.open_sleep_timer() {
            spin(300);
            if let Some(minutes) = button(&popover, "45 min") {
                minutes.emit_clicked();
            }
            if let Some(start) = button(&popover, "Start") {
                start.emit_clicked();
            }
            spin(200);
            popover.popdown();
        }
        spin(300);
        shots.foot("play-modes", &main, 0.0, 1440.0, 300.0);
    }
    main.destroy();
}

/// The settings pages, the tag editor, the mini player and the question
/// before playing at full output level.
fn shoot_windows(application: &gtk::Application, shots: &Shots) {
    const SETTINGS: &[&str] = &[
        "output",
        "playback",
        "loudness",
        "appearance",
        "shortcuts",
        "library",
    ];
    const EDITOR: &[&str] = &["tag-editor", "tag-tools", "file-info-analysis"];
    const MINI: &[&str] = &["mini-player", "mini-player-queue", "mini-player-lyrics"];
    let settings_wanted = SETTINGS
        .iter()
        .any(|page| shots.wanted(&format!("settings-{page}")));
    let settings_wanted = settings_wanted || shots.wanted("settings-analysis");
    if !(settings_wanted || shots.any(EDITOR) || shots.any(MINI) || shots.wanted("volume-confirm"))
    {
        return;
    }
    let (backend, updates) = sample::backend_with_updates(&playing(120));
    let main = main_window(application, &backend, MAIN_SIZE);
    let library = backend.library();
    let albums = library
        .album_keys(&AlbumQuery::default())
        .unwrap_or_default();

    // The settings that the pages show at their best: the leveller at work,
    // the managed folder on and silence trimmed.
    if settings_wanted {
        let music = glib::home_dir().join("Music");
        let _ = backend
            .services()
            .set_managed_folder(&music.join("Library"));
        let _ = backend.services().update_settings(&mut |s| {
            s.loudness.leveller = true;
        });
        let mut path = sample::signal_path(SignalLamp::LosslessProcessed);
        if let Some(stage) = path
            .stages
            .iter_mut()
            .find(|s| s.kind == StageKind::Loudness)
        {
            stage.detail = "leveller (no loudness values)".into();
        }
        let _ = updates.try_send(Update::SignalPath(Some(Box::new(path))));
        spin(300);
    }
    for page in SETTINGS {
        let name = format!("settings-{page}");
        if shots.wanted(&name) {
            let settings = main.open_settings(page);
            settings.set_default_size(1080, 780);
            spin(400);
            // Scrolled to what is new on the page.
            match *page {
                "playback" => scroll_to(&settings, "silence-note"),
                "library" => {
                    wait(3000, || {
                        named::<gtk::Widget>(&settings, "managed-on").is_some()
                    });
                }
                _ => {}
            }
            shots.window(&name, &settings);
        }
    }
    // The Library page further down, at the track analysis.
    if shots.wanted("settings-analysis") {
        let settings = main.open_settings("library");
        settings.set_default_size(1080, 780);
        spin(400);
        scroll_to(&settings, "track-analysis");
        shots.window("settings-analysis", &settings);
    }
    if let Some(settings) = toplevel("settings") {
        settings.close();
    }

    if shots.any(EDITOR) {
        let tracks: Vec<TrackKey> = albums
            .get(3)
            .and_then(|key| library.album(*key).ok().flatten())
            .map(|album| album.tracks().take(3).map(|t| t.key).collect())
            .unwrap_or_default();
        main.edit_tags(&tracks);
        spin(400);
        if let Some(editor) = toplevel("tag-editor") {
            shots.window("tag-editor", &editor);
            if shots.wanted("tag-tools") {
                let _ = WidgetExt::activate_action(
                    &editor,
                    "tageditor.tool",
                    Some(&"from-names".to_variant()),
                );
                spin(500);
                if let Some(tools) = toplevel("tag-tools") {
                    // A pattern the sample's file names fit.
                    if let Some(pattern) = descendants(tools.upcast_ref())
                        .into_iter()
                        .filter_map(|w| w.downcast::<gtk::Entry>().ok())
                        .find(|e| e.text().contains("{track}"))
                    {
                        pattern.set_text("{track} {title}");
                        spin(300);
                    }
                    shots.window("tag-tools", &tools);
                    tools.close();
                }
            }
            editor.close();
        }
        // File info of a track that was analysed, with its loudness curve.
        if shots.wanted("file-info-analysis")
            && let Some(track) = analysed_track(&backend)
        {
            spin(300);
            main.edit_tags(&[track]);
            if wait(3000, || toplevel("tag-editor").is_some())
                && let Some(editor) = toplevel("tag-editor")
            {
                if let Some(tab) = toggle(&editor, "File info") {
                    tab.set_active(true);
                }
                spin(400);
                shots.window("file-info-analysis", &editor);
                editor.close();
            }
        }
    }

    if shots.any(MINI) {
        gio::prelude::ActionGroupExt::activate_action(application, "mini-player", None);
        spin(500);
        if let Some(mini) = main.mini_player_window() {
            main.set_mini_player_drawer(false, "queue");
            spin(300);
            shots.window("mini-player", &mini);
            main.set_mini_player_drawer(true, "queue");
            spin(300);
            shots.window("mini-player-queue", &mini);
            main.set_mini_player_drawer(true, "lyrics");
            spin(300);
            shots.window("mini-player-lyrics", &mini);
            main.set_mini_player_drawer(false, "queue");
        }
        gio::prelude::ActionGroupExt::activate_action(application, "mini-player", None);
        spin(300);
    }

    // The question before playback to a device at a fixed, full volume.
    if shots.wanted("volume-confirm")
        && let Some(&album) = albums.get(3)
    {
        backend.send(PlayerRequest::Stop);
        let _ = backend
            .services()
            .update_settings(&mut |s| s.output.volume = VolumeControl::Fixed);
        backend.send(PlayerRequest::PlayAlbum {
            album,
            start: 0,
            shuffle: false,
        });
        spin(300);
        if let Some(confirm) = toplevel("volume-confirm") {
            shots.window("volume-confirm", &confirm);
            confirm.close();
        }
    }
    main.destroy();
}

/// A sample track of more than 200 seconds whose analysis is done.
fn analysed_track(backend: &Backend) -> Option<TrackKey> {
    let keys = backend.library().track_keys(&TrackQuery::default()).ok()?;
    let services = backend.services();
    keys.chunks(20)
        .filter_map(|chunk| services.edit_tracks(chunk).ok())
        .flatten()
        .find(|t| {
            matches!(t.facts.analysis, AnalysisFacts::Analysed(_))
                && t.facts.duration.is_some_and(|d| d.as_secs() > 200)
        })
        .map(|t| t.key)
}

/// Settings › DSP and EQ with each editor, and the signal path while the
/// equalizer and the limiter work.
fn shoot_dsp(application: &gtk::Application, shots: &Shots) {
    const STAGES: [(&str, &str); 4] = [
        ("equalizer", "eq-parametric"),
        ("crossfeed", "crossfeed"),
        ("channel-mix", "channel-mix"),
        ("limiter", "limiter"),
    ];
    const GRAPHIC: [(&str, &str); 2] = [
        ("Graphic, 10 bands", "eq-graphic-10"),
        ("Graphic, 31 bands", "eq-graphic-31"),
    ];
    if !(shots.wanted("signal-path-dsp")
        || STAGES
            .iter()
            .chain(&GRAPHIC)
            .any(|(_, name)| shots.wanted(name)))
    {
        return;
    }
    let (backend, updates) = sample::backend_with_updates(&playing(120));
    let main = main_window(application, &backend, MAIN_SIZE);
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
    spin(300);
    // What the engine reports then: the same file and device, processed.
    let path = sample::signal_path(SignalLamp::LosslessProcessed);
    let _ = updates.try_send(Update::Output(OutputInfo {
        device: "Studio Interface".to_owned(),
        signal: SignalState::of(&path),
        sample_rate: Some(96_000),
        bits: Some(24),
    }));
    let _ = updates.try_send(Update::SignalPath(Some(Box::new(path))));
    main.show_signal_path();
    spin(500);
    shots.window("signal-path-dsp", &main);

    for (stage, name) in STAGES {
        if shots.wanted(name) {
            let settings = main.open_dsp_stage(stage);
            settings.set_default_size(1080, 780);
            shots.window(name, &settings);
        }
    }
    for (mode, name) in GRAPHIC {
        if shots.wanted(name) {
            let settings = main.open_dsp_stage("equalizer");
            settings.set_default_size(1080, 780);
            if let Some(mode) = toggle(&settings, mode) {
                mode.set_active(true);
            }
            shots.window(name, &settings);
        }
    }
    if let Some(settings) = toplevel("settings") {
        settings.close();
    }
    main.destroy();
}

/// The tools of the Tools and Library menus, and the copies of a file.
fn shoot_tools(application: &gtk::Application, shots: &Shots) {
    const PICTURES: &[&str] = &[
        "converter",
        "sync-export",
        "cover-manager",
        "organizer",
        "missing-files",
        "verify-files",
        "copies",
        "duplicates",
        "import",
        "managed-import",
        "fields",
    ];
    if !shots.any(PICTURES) {
        return;
    }
    let backend = sample::backend(&sample::SampleOptions {
        albums: 120,
        playing: None,
    });
    let main = main_window(application, &backend, MAIN_SIZE);

    for (action, name) in [
        ("convert", "converter"),
        ("sync-export", "sync-export"),
        ("cover-manager", "cover-manager"),
    ] {
        if !shots.wanted(name) {
            continue;
        }
        gio::prelude::ActionGroupExt::activate_action(application, action, None);
        spin(600);
        if let Some(tool) = toplevel(name) {
            shots.window(name, &tool);
            tool.close();
        }
    }

    if shots.wanted("organizer")
        && let Some(organizer) = main.open_organizer()
    {
        spin(300);
        if let Some(row) = named::<gtk::ListBoxRow>(&organizer, "pattern-2") {
            row.activate();
            if let Some(list) = row.parent().and_downcast::<gtk::ListBox>() {
                list.select_row(Some(&row));
            }
        }
        spin(500);
        shots.window("organizer", &organizer);
        organizer.close();
    }
    if shots.wanted("missing-files")
        && let Some(missing) = main.open_missing_files()
    {
        spin(500);
        shots.window("missing-files", &missing);
        missing.close();
    }
    if shots.wanted("verify-files")
        && let Some(verify) = main.open_verify_files()
    {
        spin(800);
        shots.window("verify-files", &verify);
        verify.close();
    }

    if shots.wanted("copies")
        && let Some(copies) = main.open_copies(5)
    {
        spin(400);
        shots.window("copies", &copies);
        copies.close();
    }
    if shots.wanted("duplicates")
        && let Some(finder) = main.open_find_duplicates("identical-files")
    {
        spin(500);
        if let Some(mark) = named::<gtk::Button>(&finder, "mark") {
            mark.emit_clicked();
        }
        shots.window("duplicates", &finder);
        finder.close();
    }

    // Import from another player, with the plan read from Rhythmbox.
    if shots.wanted("import")
        && let Some(import) = main.open_import(Some("rhythmbox"))
    {
        let ready = wait(8000, || {
            named::<gtk::Button>(&import, "import-apply").is_some_and(|b| b.is_sensitive())
        });
        if !ready {
            eprintln!("site-shots: import: the plan was not read");
            shots.failed.set(true);
        }
        shots.window("import", &import);
        import.close();
    }
    // The plan of adding music to the managed folder.
    if shots.wanted("managed-import") {
        let home = glib::home_dir();
        let _ = backend
            .services()
            .set_managed_folder(&home.join("Music").join("Library"));
        spin(300);
        let sources = vec![
            home.join("Downloads").join("bandcamp"),
            PathBuf::from("/run/media/user/USB-STICK"),
            home.join("Music").join("Incoming").join("new"),
        ];
        if let Some(plan) = main.open_managed_add(sources) {
            let ready = wait(8000, || {
                named::<gtk::Button>(&plan, "managed-run").is_some_and(|b| b.is_sensitive())
            });
            if !ready {
                eprintln!("site-shots: managed-import: the plan was not made");
                shots.failed.set(true);
            }
            shots.window("managed-import", &plan);
            plan.close();
        }
    }
    // Your fields with a computed field chosen and its example values.
    if shots.wanted("fields")
        && let Some(fields) = main.open_user_fields()
    {
        wait(5000, || {
            named::<gtk::Entry>(&fields, "fields-name").is_some_and(|e| e.text() == "decade")
                && named::<gtk::Box>(&fields, "fields-values")
                    .is_some_and(|b| b.first_child().is_some())
        });
        shots.window("fields", &fields);
        fields.close();
    }
    main.destroy();
}

/// An audiobook's page, the first run and the narrow window.
fn shoot_other(application: &gtk::Application, shots: &Shots) {
    if shots.wanted("audiobook") {
        let backend = sample::backend(&sample::SampleOptions {
            albums: 300,
            playing: None,
        });
        let main = main_window(application, &backend, MAIN_SIZE);
        if backend
            .library()
            .set_album_kind(AlbumKey(2), MediaKind::Audiobook)
            .is_ok()
        {
            // Without the queue panel, so that the page with its extra column
            // has the width it needs.
            let _ = WidgetExt::activate_action(&main, "win.toggle-panel", None);
            main.open_album(AlbumKey(2));
            backend.send(PlayerRequest::PlayAlbum {
                album: AlbumKey(2),
                start: 3,
                shuffle: false,
            });
            spin(700);
            shots.window("audiobook", &main);
        }
        main.destroy();
    }

    if shots.wanted("first-run") {
        let backend = sample::first_run_backend(&playing(120));
        let main = main_window(application, &backend, MAIN_SIZE);
        shots.window("first-run", &main);
        main.destroy();
    }

    if shots.any(&["narrow", "narrow-sidebar"]) {
        let backend = sample::backend(&playing(sample::SampleOptions::default().albums));
        let main = main_window(application, &backend, (640, 900));
        shots.window("narrow", &main);
        let _ = WidgetExt::activate_action(&main, "win.toggle-sidebar", None);
        spin(400);
        shots.window("narrow-sidebar", &main);
        main.destroy();
    }
}

struct Options {
    out: PathBuf,
    scheme: SchemeChoice,
    scale: f32,
    only: Vec<String>,
    store: bool,
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        out: PathBuf::from("target/site-shots"),
        scheme: SchemeChoice::Dark,
        scale: 1.0,
        only: Vec::new(),
        store: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => options.out = PathBuf::from(value()?),
            "--scheme" => {
                options.scheme = match value()?.as_str() {
                    "dark" => SchemeChoice::Dark,
                    "light" => SchemeChoice::Light,
                    other => return Err(format!("--scheme is dark or light, not {other}")),
                }
            }
            "--scale" => {
                options.scale = value()?
                    .parse()
                    .ok()
                    .filter(|s| (1.0..=4.0).contains(s))
                    .ok_or("--scale is a number from 1 to 4")?;
            }
            "--only" => options.only = value()?.split(',').map(str::to_owned).collect(),
            "--store" => options.store = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

fn main() -> ExitCode {
    let options = match options() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("site-shots: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = std::fs::create_dir_all(&options.out) {
        eprintln!(
            "site-shots: cannot create {}: {error}",
            options.out.display()
        );
        return ExitCode::FAILURE;
    }
    helikodu_ui_gtk::i18n::init();
    if gtk::init().is_err() {
        eprintln!("site-shots: no display; run it under crates/ui-gtk/tests/headless.sh");
        return ExitCode::FAILURE;
    }
    prepare_display(options.scheme, AccentChoice::Amber);
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_decoration_layout(Some("icon:minimize,maximize,close"));
    }
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.SiteShots")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let shots = Rc::new(Shots {
        out: options.out,
        scheme: options.scheme,
        scale: options.scale,
        only: options.only,
        store: options.store,
        failed: Cell::new(false),
    });
    let s = shots.clone();
    application.connect_activate(move |application| {
        let _hold = application.hold();
        set_choice(s.scheme, AccentChoice::Amber);
        shoot_library(application, &s);
        shoot_windows(application, &s);
        shoot_dsp(application, &s);
        shoot_tools(application, &s);
        shoot_other(application, &s);
        application.quit();
    });
    application.run_with_args(&["site-shots"]);
    if shots.failed.get() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
