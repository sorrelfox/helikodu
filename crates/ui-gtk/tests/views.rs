// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The library views against the sample backend: Tracks, Artists, Genres,
//! Folders, playlists, search and the empty library, the context menu's
//! actions, and the frame times of the track table scrolling 100,000 tracks.
//!
//! Like `widgets.rs`, this program has its own main function and runs only
//! under the headless compositor script:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test views
//! # the frame times of a release build:
//! crates/ui-gtk/tests/headless.sh cargo test --release -p helikodu-ui-gtk --test views -- frame_times
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
use helikodu_ui_gtk::backend::{AlbumKey, Backend, TrackColumn, sample};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice};
use helikodu_ui_gtk::{MainWindow, WindowStyle, install_actions, prepare_display};

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

fn window_with(application: &gtk::Application, albums: usize) -> (MainWindow, Backend) {
    let backend = sample::backend(&sample::SampleOptions {
        albums,
        playing: None,
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(100);
    (window, backend)
}

fn window(application: &gtk::Application) -> (MainWindow, Backend) {
    window_with(application, 300)
}

fn tracks_load_lazily_and_sort(app: &gtk::Application) {
    let (window, backend) = window(app);
    let total = backend.library().counts().unwrap().tracks as u32;
    window.open_page("tracks");
    assert_eq!(window.visible_view().as_deref(), Some("tracks"));
    wait_for("the track keys", 2, || {
        window.track_table_stats().0 == total
    });
    wait_for("the first rows", 2, || window.track_table_stats().1 > 0);
    let (_, fetches) = window.track_table_stats();
    assert!(fetches <= 3, "rows come in batches ({fetches} fetches)");

    window.sort_tracks(&[(TrackColumn::Title, false)]);
    spin(300);
    let keys = window.track_keys(50);
    let rows = backend.library().track_rows(&keys).unwrap();
    let titles: Vec<String> = rows.iter().map(|r| r.title.to_lowercase()).collect();
    assert!(titles.windows(2).all(|w| w[0] <= w[1]), "sorted by title");

    window.sort_tracks(&[(TrackColumn::Artist, true), (TrackColumn::Length, false)]);
    spin(300);
    let rows = backend
        .library()
        .track_rows(&window.track_keys(200))
        .unwrap();
    assert!(
        rows.windows(2).all(|w| {
            let (a, b) = (w[0].artist.to_lowercase(), w[1].artist.to_lowercase());
            a > b || (a == b && w[0].duration <= w[1].duration)
        }),
        "sorted by artist descending, then length"
    );
    window.destroy();
}

fn artists_genres_and_folders_list(app: &gtk::Application) {
    let (window, _) = window(app);
    for page in ["artists", "genres", "folders"] {
        window.open_page(page);
        assert_eq!(window.visible_view().as_deref(), Some(page));
    }
    wait_for("the lists", 2, || {
        let (artists, genres, roots) = window.list_counts();
        artists > 0 && genres > 0 && roots == 3
    });
    window.go_back();
    assert_eq!(window.visible_view().as_deref(), Some("genres"));
    window.destroy();
}

fn playlists_edit_with_undo(app: &gtk::Application) {
    let (window, _) = window(app);
    wait_for("the playlists", 2, || window.playlist_names().len() == 3);
    app.activate_action("new-playlist", None);
    wait_for("a new playlist", 2, || {
        window.playlist_names().iter().any(|n| n == "New playlist")
    });
    assert_eq!(window.visible_view().as_deref(), Some("playlist"));

    assert!(window.open_first_playlist());
    wait_for("the entries", 2, || window.playlist_titles().len() == 48);
    let before = window.playlist_titles();
    let menu = window.demo_track_menu().expect("the context menu opens");
    wait_for("the menu", 2, || menu.is_visible());
    menu.popdown();
    spin(20);
    WidgetExt::activate_action(&window, "win.sel-remove", None).unwrap();
    wait_for("the removal", 2, || window.playlist_titles().len() == 45);
    assert!(window.toast_text().unwrap_or_default().contains("removed"));
    WidgetExt::activate_action(&window, "win.undo", None).unwrap();
    wait_for("the undo", 2, || window.playlist_titles() == before);
    window.destroy();
}

fn the_context_menu_queues(app: &gtk::Application) {
    let (window, _) = window(app);
    wait_for("the playlists", 2, || !window.playlist_names().is_empty());
    assert!(window.open_first_playlist());
    wait_for("the entries", 2, || !window.playlist_titles().is_empty());
    let menu = window.demo_track_menu().expect("the context menu opens");
    wait_for("the menu", 2, || menu.is_visible());
    menu.popdown();
    WidgetExt::activate_action(&window, "win.sel-enqueue", None).unwrap();
    wait_for("the toast", 2, || {
        window
            .toast_text()
            .is_some_and(|t| t.contains("added to the queue"))
    });
    let menu = window.demo_track_menu().expect("the context menu opens");
    wait_for("the menu", 2, || menu.is_visible());
    menu.popdown();
    WidgetExt::activate_action(&window, "win.sel-add-to-playlist", Some(&0u64.to_variant()))
        .unwrap();
    wait_for("a playlist from the selection", 2, || {
        window.playlist_names().iter().any(|n| n == "New playlist")
    });
    window.destroy();
}

fn search_and_no_results(app: &gtk::Application) {
    let (window, _) = window(app);
    window.open_page("genres");
    window.search_text("harbour");
    wait_for("the results", 2, || {
        window
            .search_results()
            .is_some_and(|(a, b, c)| a + b + c > 0)
    });
    assert_eq!(window.visible_view().as_deref(), Some("search"));
    window.search_text("qqzzxx");
    wait_for("no results", 2, || {
        window.search_results() == Some((0, 0, 0))
    });
    window.search_text("");
    spin(100);
    assert_eq!(
        window.visible_view().as_deref(),
        Some("genres"),
        "clearing the search returns"
    );
    WidgetExt::activate_action(&window, "win.search", None).unwrap();
    window.destroy();
}

fn menus_open(app: &gtk::Application) {
    let (window, _) = window(app);
    let main = window.demo_main_menu().expect("the main menu");
    spin(50);
    assert!(main.is_visible());
    main.popdown();
    let play = window.demo_play_menu();
    spin(50);
    assert!(play.is_visible());
    play.popdown();
    spin(50);
    window.destroy();
}

fn the_empty_library(app: &gtk::Application) {
    let (window, _) = window_with(app, 0);
    spin(200);
    assert_eq!(window.visible_view().as_deref(), Some("albums"));
    assert!(window.playlist_names().is_empty());
    assert_eq!(window.album_grid_stats().0, 0);
    window.destroy();
}

/// Percentiles of `values` (milliseconds): p50, p95, p99, max.
fn percentiles(values: &mut [f64]) -> (f64, f64, f64, f64) {
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    (at(0.5), at(0.95), at(0.99), values[values.len() - 1])
}

/// Scrolls the track table over 100,000 tracks for a few seconds and reports
/// the frame intervals and the main-thread work per frame.
fn frame_times(app: &gtk::Application) {
    let (window, backend) = window_with(app, 12_000);
    let total = backend.library().counts().unwrap().tracks as u32;
    assert!(total >= 100_000, "{total} tracks");
    window.open_page("tracks");
    wait_for("100,000 track keys", 30, || {
        window.track_table_stats().0 == total
    });
    spin(500);
    let adjustment = window.track_table_adjustment().unwrap();
    let clock = window.frame_clock().unwrap();
    for (mode, step) in [("slow", 12.0), ("fast", 900.0), ("jump", -1.0)] {
        let work = Rc::new(RefCell::new(Vec::new()));
        let intervals = Rc::new(RefCell::new(Vec::new()));
        let started = Rc::new(RefCell::new(None::<Instant>));
        let last = Rc::new(RefCell::new(None::<i64>));
        let s = started.clone();
        let before = clock.connect_update(move |_| {
            s.replace(Some(Instant::now()));
        });
        let (s, w) = (started.clone(), work.clone());
        let after = clock.connect_after_paint(move |_| {
            if let Some(start) = s.take() {
                w.borrow_mut().push(start.elapsed().as_secs_f64() * 1000.0);
            }
        });
        let (l, i, a) = (last.clone(), intervals.clone(), adjustment.clone());
        let seed = std::cell::Cell::new(0x2545_F491_4F6C_DD1Du64);
        let tick = window.add_tick_callback(move |_, clock| {
            let now = clock.frame_time();
            if let Some(previous) = l.replace(Some(now)) {
                i.borrow_mut().push((now - previous) as f64 / 1000.0);
            }
            let range = a.upper() - a.page_size();
            let next = if step < 0.0 {
                let mut x = seed.get();
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                seed.set(x);
                (x % 10_000) as f64 / 10_000.0 * range
            } else {
                let v = a.value() + step;
                if v > range { 0.0 } else { v }
            };
            a.set_value(next);
            glib::ControlFlow::Continue
        });
        spin(3000);
        tick.remove();
        clock.disconnect(before);
        clock.disconnect(after);
        let mut work = work.take();
        let mut intervals = intervals.take();
        assert!(intervals.len() > 30, "frames came ({})", intervals.len());
        let (w50, w95, w99, wmax) = percentiles(&mut work);
        let (i50, i95, i99, imax) = percentiles(&mut intervals);
        let late = intervals.iter().filter(|i| **i > 20.0).count();
        println!(
            "frame_times {mode}: {} frames; interval ms p50 {i50:.1} p95 {i95:.1} p99 {i99:.1} max {imax:.1} ({late} over 20 ms); work ms p50 {w50:.1} p95 {w95:.1} p99 {w99:.1} max {wmax:.1}",
            intervals.len()
        );
        let budget = if cfg!(debug_assertions) { 120.0 } else { 33.0 };
        if mode != "jump" {
            assert!(w95 < budget, "{mode} scrolling: p95 work {w95:.1} ms");
        }
    }
    let (_, fetches) = window.track_table_stats();
    println!("frame_times: {fetches} row batches fetched");
    window.destroy();
}

/// The widgets under `root`, depth first.
fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut all = Vec::new();
    let mut child = root.first_child();
    while let Some(widget) = child {
        all.push(widget.clone());
        all.extend(descendants(&widget));
        child = widget.next_sibling();
    }
    all
}

/// A context menu opens at its full size: no item is cut off and it does
/// not scroll while it fits on the screen, and its scrollbar would sit in
/// the gutter at the right of the items.
fn the_context_menu_fits(app: &gtk::Application) {
    let (window, _) = window(app);
    window.open_album(AlbumKey(1));
    spin(300);
    let menu = window
        .demo_album_track_menu()
        .expect("the album page's track menu opens");
    // Without the grab, which the test compositor may refuse to a window
    // that is not in front; the size is the same.
    menu.set_autohide(false);
    wait_for("the menu", 2, || menu.is_mapped());
    spin(100);
    let scroller = descendants(menu.upcast_ref())
        .into_iter()
        .find_map(|w| w.downcast::<gtk::ScrolledWindow>().ok())
        .expect("the menu's scrolled window");
    let adjustment = scroller.vadjustment();
    assert!(
        adjustment.upper() <= adjustment.page_size() + 0.5,
        "the menu scrolls: {} of {} shown",
        adjustment.page_size(),
        adjustment.upper()
    );
    let (_, natural, _, _) = menu.measure(gtk::Orientation::Vertical, menu.width());
    assert_eq!(menu.height(), natural, "the menu has its natural height");
    let items: Vec<gtk::Widget> = descendants(scroller.upcast_ref())
        .into_iter()
        .filter(|w| w.css_name() == "modelbutton" && w.is_mapped())
        .collect();
    assert!(items.len() >= 9, "every item is shown: {}", items.len());
    let bar = scroller.vscrollbar();
    let bar_width = bar.measure(gtk::Orientation::Horizontal, -1).1;
    for item in &items {
        let bounds = item.compute_bounds(&scroller).unwrap();
        assert!(
            bounds.y() + bounds.height() <= scroller.height() as f32 + 0.5,
            "an item is cut off"
        );
        let gutter = scroller.width() as f32 - (bounds.x() + bounds.width());
        assert!(
            gutter >= bar_width as f32,
            "the scrollbar ({bar_width} px) would cover the items (gutter {gutter})"
        );
    }
    menu.popdown();
    // The closed menu leaves its list in an idle callback; a list box that
    // still holds it when destroyed would try to remove it forever.
    wait_for("the menu to go", 2, || menu.parent().is_none());
    window.destroy();
}

/// A sidebar width saved too narrow is widened to the minimum, and the rows
/// keep their names and counts inside the sidebar.
fn the_sidebar_keeps_its_minimum(app: &gtk::Application) {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 300,
        playing: None,
    });
    let store = backend.ui_state();
    let mut state = store.load();
    state.panes.sidebar_width = 40;
    state.panes.sidebar_visible = true;
    state.panes.queue_visible = true;
    store.save(&state).unwrap();
    install_actions(app, &backend);
    let window = MainWindow::new(app, &backend, WindowStyle::Classic);
    window.set_default_size(960, 600);
    window.present();
    spin(300);
    for page in ["albums", "genres", "artists", "tracks"] {
        window.open_page(page);
        spin(300);
        let sidebar = descendants(window.upcast_ref())
            .into_iter()
            .find(|w| w.has_css_class("hk-sidebar"))
            .expect("the sidebar");
        let bounds = sidebar.compute_bounds(&window).unwrap();
        assert!(
            bounds.width() >= 180.0,
            "{page}: the sidebar is {} px wide",
            bounds.width()
        );
        assert!(
            bounds.x() >= 0.0,
            "{page}: the sidebar starts left of the window"
        );
        for label in descendants(&sidebar)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Label>().ok())
            .filter(|l| l.is_mapped() && !l.text().is_empty())
        {
            let b = label.compute_bounds(&window).unwrap();
            assert!(
                b.x() >= bounds.x() && b.x() + b.width() <= bounds.x() + bounds.width() + 0.5,
                "{page}: \"{}\" is cut off",
                label.text()
            );
        }
    }
    window.destroy();
}

/// Every scrolled list leaves room at the right of its rows for the
/// scrollbar, so it never covers a count.
fn scrollbars_leave_room(app: &gtk::Application) {
    let (window, _) = window(app);
    for page in ["albums", "genres", "artists", "folders"] {
        window.open_page(page);
        spin(400);
        for scroller in descendants(window.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::ScrolledWindow>().ok())
            .filter(|s| s.is_mapped() && s.vscrollbar_policy() != gtk::PolicyType::Never)
        {
            let Some(row) = descendants(scroller.upcast_ref())
                .into_iter()
                .find(|w| w.css_name() == "row" && w.is_mapped())
            else {
                continue;
            };
            let bar = scroller.vscrollbar();
            let bar_width = bar.measure(gtk::Orientation::Horizontal, -1).1;
            assert!(
                bar_width <= 8,
                "{page}: the scrollbar is {bar_width} px wide"
            );
            let bounds = row.compute_bounds(&scroller).unwrap();
            let gutter = scroller.width() as f32 - (bounds.x() + bounds.width());
            assert!(
                gutter >= bar_width as f32,
                "{page}: a list's rows reach under its scrollbar (gutter {gutter}, scrollbar {bar_width})"
            );
        }
    }
    window.destroy();
}

/// The Genres page's album grid shows the covers, not the placeholders.
fn genre_grid_shows_covers(app: &gtk::Application) {
    let folder = std::env::temp_dir().join(format!("helikodu-views-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let backend = sample::covered_backend(
        &sample::SampleOptions {
            albums: 300,
            playing: None,
        },
        &folder,
    )
    .unwrap();
    install_actions(app, &backend);
    let window = MainWindow::new(app, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(200);
    window.open_page("genres");
    wait_for("the genre's covers", 5, || {
        let (tiles, covered) = window.genre_cover_stats();
        tiles > 0 && covered == tiles
    });
    window.destroy();
    let _ = std::fs::remove_dir_all(&folder);
}

/// The integrated style decorates the window itself (GTK then asks the
/// compositor for no title bar); the classic style leaves it to the desktop.
fn window_styles_ask_for_their_decorations(app: &gtk::Application) {
    let (window, _) = window(app);
    assert!(
        window.titlebar().is_some(),
        "integrated: a titlebar of its own"
    );
    assert!(window.is_decorated());
    assert!(
        window.has_css_class("csd"),
        "integrated: client-side decorations"
    );
    window.destroy();
    let backend = sample::backend(&sample::SampleOptions {
        albums: 10,
        playing: None,
    });
    let window = MainWindow::new(app, &backend, WindowStyle::Classic);
    assert!(
        window.titlebar().is_none(),
        "classic: the desktop's title bar"
    );
    window.destroy();
}

type Test = (&'static str, fn(&gtk::Application));

const TESTS: &[Test] = &[
    ("tracks_load_lazily_and_sort", tracks_load_lazily_and_sort),
    (
        "artists_genres_and_folders_list",
        artists_genres_and_folders_list,
    ),
    ("playlists_edit_with_undo", playlists_edit_with_undo),
    ("the_context_menu_queues", the_context_menu_queues),
    ("search_and_no_results", search_and_no_results),
    ("menus_open", menus_open),
    ("the_empty_library", the_empty_library),
    ("the_context_menu_fits", the_context_menu_fits),
    (
        "the_sidebar_keeps_its_minimum",
        the_sidebar_keeps_its_minimum,
    ),
    ("scrollbars_leave_room", scrollbars_leave_room),
    ("genre_grid_shows_covers", genre_grid_shows_covers),
    (
        "window_styles_ask_for_their_decorations",
        window_styles_ask_for_their_decorations,
    ),
    ("frame_times", frame_times),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("views: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("views: skipped, no display");
        return ExitCode::SUCCESS;
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.ViewTests")
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
    application.run_with_args(&["views"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("views: {} passed", ran.borrow());
        ExitCode::SUCCESS
    } else {
        println!("views: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
