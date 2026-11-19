// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The facet browser of Tracks, Library statistics and the narrow window,
//! against the sample backend.
//!
//! Like `views.rs`, this program has its own main function and runs only under
//! the headless compositor script:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test insights
//! ```

#![forbid(unsafe_code)]
// A panic is how a test fails; these are tests without the test harness.
#![allow(clippy::unwrap_used)]

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_ui_gtk::backend::{AlbumQuery, Backend, FacetField, sample};
use helikodu_ui_gtk::style::{AccentChoice, SchemeChoice};
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

fn window_sized(application: &gtk::Application, width: i32) -> (MainWindow, Backend) {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 120,
        playing: None,
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(width, 900);
    window.present();
    spin(150);
    (window, backend)
}

fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(widget) = pending.pop() {
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            pending.push(c.clone());
        }
        out.push(widget);
    }
    out
}

/// Saves the window to `HELIKODU_INSIGHTS_SHOTS/name.png` when that is set.
fn shot(window: &MainWindow, name: &str) {
    let Some(dir) = std::env::var_os("HELIKODU_INSIGHTS_SHOTS") else {
        return;
    };
    spin(300);
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    snapshot::save_png(window, &path).unwrap();
}

/// Counts the frames the window paints in `ms` milliseconds.
fn frames_in(window: &MainWindow, ms: u64) -> u32 {
    let count = Rc::new(Cell::new(0));
    let clock = window.frame_clock().unwrap();
    let c = count.clone();
    let handler = clock.connect_after_paint(move |_| c.set(c.get() + 1));
    spin(ms);
    clock.disconnect(handler);
    count.get()
}

fn the_facet_browser_narrows_the_table(app: &gtk::Application) {
    let (window, _) = window_sized(app, 1440);
    window.open_page("tracks");
    wait_for("the tracks", 3, || window.tracks_shown().0 > 0);
    let all = window.tracks_shown().0;
    window.set_browsing(true);
    wait_for("the panes", 3, || {
        let rows = window.facet_rows();
        rows.len() == 3 && rows.iter().all(|p| !p.is_empty())
    });
    let rows = window.facet_rows();
    assert_eq!(rows[0][0].0, "All genres");
    assert_eq!(rows[0][0].1, all, "the All row counts every track");
    assert!(rows[0][0].2, "All is chosen at first");
    assert_eq!(rows[1][0].0, "All album artists");
    assert_eq!(rows[2][0].0, "All albums");

    // One genre.
    let genre = rows[0][1].clone();
    window.choose_facets(0, &[1]);
    wait_for("the table to narrow", 3, || {
        window.tracks_shown().0 == genre.1
    });
    let (_, count) = window.tracks_shown();
    assert!(
        count.contains(" of "),
        "the count says of how many: {count}"
    );
    wait_for("the panes to the right", 3, || {
        window.facet_rows()[1].first().map(|r| r.1) == Some(genre.1)
    });
    let rows = window.facet_rows();
    assert!(
        !rows[0][0].2 && rows[0][1].2,
        "the genre is chosen, not All"
    );
    let artists: usize = rows[1].iter().skip(1).map(|r| r.1).sum();
    assert!(
        artists >= genre.1,
        "every track of the genre has an album artist"
    );

    // Two genres, as with Ctrl.
    let second = rows[0][2].clone();
    window.choose_facets(0, &[1, 2]);
    wait_for("both genres", 3, || {
        window.tracks_shown().0 == genre.1 + second.1
    });

    // An album artist of those narrows further.
    let artist = window.facet_rows()[1][1].clone();
    window.choose_facets(1, &[1]);
    wait_for("the album artist", 3, || {
        window.tracks_shown().0 == artist.1
    });
    shot(&window, "facets");

    // Another field for the third pane.
    window.set_facet_field(2, FacetField::Year);
    wait_for("the years", 3, || {
        window.facet_rows()[2].first().map(|r| r.0.as_str()) == Some("All years")
    });

    // All genres again: the album artist stays chosen, as it is still listed.
    window.choose_facets(0, &[0]);
    wait_for("All genres", 3, || window.tracks_shown().0 == artist.1);
    assert!(window.facet_rows()[0][0].2);

    // Closed, the browser no longer narrows the table.
    window.set_browsing(false);
    wait_for("every track", 3, || window.tracks_shown().0 == all);
    window.destroy();
}

fn statistics_count_and_open_tracks(app: &gtk::Application) {
    let (window, backend) = window_sized(app, 1440);
    WidgetExt::activate_action(&window, "win.show-statistics", None).unwrap();
    assert_eq!(window.visible_view().as_deref(), Some("statistics"));
    wait_for("the figures", 3, || window.statistics().is_some());
    let stats = window.statistics().unwrap();
    let counts = backend.library().counts().unwrap();
    assert_eq!(stats.tracks, counts.tracks);
    assert_eq!(stats.albums, counts.albums);
    assert_eq!(
        stats.lossless_cd + stats.lossless_hi_res + stats.lossy,
        stats.tracks
    );
    let texts = window.statistics_texts();
    for wanted in [
        "Tracks",
        "Albums",
        "Artists",
        "Playing time",
        "Size",
        "Formats",
        "Quality",
        "Listening",
        "Most played artists",
        "Most played albums",
        "FLAC",
        "Lossless, hi-res",
    ] {
        assert!(
            texts.iter().any(|t| t == wanted),
            "{wanted} is shown: {texts:?}"
        );
    }
    assert!(texts[0].starts_with("counted today at "), "{}", texts[0]);
    shot(&window, "statistics");
    // Nothing is drawn while the figures stand still.
    spin(200);
    assert_eq!(frames_in(&window, 600), 0, "no frames while idle");
    // A row of Formats opens Tracks with its query.
    let flac = descendants(window.upcast_ref())
        .into_iter()
        .filter(|w| w.has_css_class("hk-stat-row"))
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| {
            descendants(b.upcast_ref())
                .iter()
                .filter_map(|w| w.downcast_ref::<gtk::Label>())
                .any(|l| l.text() == "FLAC")
        })
        .expect("the FLAC row");
    flac.emit_clicked();
    assert_eq!(window.visible_view().as_deref(), Some("tracks"));
    wait_for("the FLAC tracks", 3, || {
        let (n, count) = window.tracks_shown();
        n > 0 && n < stats.tracks && count.contains(" of ")
    });
    window.destroy();
}

fn a_narrow_window_lays_the_panes_over_the_content(app: &gtk::Application) {
    let (window, _) = window_sized(app, 640);
    wait_for("the narrow layout", 3, || {
        window.narrow_state() == (true, true, true)
    });
    assert_eq!(
        window.narrow_header(),
        (true, false),
        "sidebar button, no forward"
    );
    assert_eq!(window.narrow_overlay_open(), (false, false));
    assert!(window.compact_output_text().is_some());
    assert_eq!(window.search_placeholder(), "Search");
    shot(&window, "narrow");
    // The sidebar opens over the content, with the layer behind it.
    WidgetExt::activate_action(&window, "win.toggle-sidebar", None).unwrap();
    spin(50);
    assert_eq!(window.narrow_overlay_open(), (true, true));
    shot(&window, "narrow-sidebar");
    // A click beside it closes it.
    window.click_beside_overlay();
    spin(50);
    assert_eq!(window.narrow_overlay_open(), (false, false));
    // Choosing a view closes the queue opened over the content.
    WidgetExt::activate_action(&window, "win.toggle-panel", None).unwrap();
    spin(50);
    assert_eq!(window.narrow_overlay_open(), (true, true));
    window.open_page("tracks");
    spin(50);
    assert_eq!(window.narrow_overlay_open(), (false, false));
    assert_eq!(window.visible_view().as_deref(), Some("tracks"));
    // Nothing is drawn while idle.
    spin(300);
    assert_eq!(frames_in(&window, 600), 0, "no frames while idle");
    window.destroy();

    // A wide window keeps both panes in the row.
    let (wide, _) = window_sized(app, 1440);
    spin(100);
    assert_eq!(wide.narrow_state(), (false, false, false));
    assert_eq!(wide.narrow_header(), (false, true));
    assert_eq!(wide.search_placeholder(), "Search artists, albums, tracks");
    assert_eq!(wide.panes_shown(), (true, true));
    wide.destroy();
}

fn a_hidden_sidebar_leaves_its_room_to_the_panel(app: &gtk::Application) {
    let toggle = |window: &MainWindow, action: &str| {
        WidgetExt::activate_action(window, action, None).unwrap();
        spin(50);
    };
    // At 1000 px the panel goes over the content while the sidebar is shown
    // (1000 - 228 - 320 < 560) and comes back beside it when the sidebar is
    // hidden (1000 - 320 >= 560).
    let (window, _) = window_sized(app, 1000);
    wait_for("the narrow layout", 3, || {
        window.narrow_state() == (false, true, false)
    });
    assert_eq!(window.panes_shown(), (true, false));
    toggle(&window, "win.toggle-sidebar");
    assert_eq!(window.narrow_state(), (false, false, false));
    assert_eq!(window.panes_shown(), (false, true));
    assert_eq!(window.narrow_overlay_open(), (false, false));
    // Shown again, the sidebar takes the room back: the panel goes over the
    // content, closed.
    toggle(&window, "win.toggle-sidebar");
    assert_eq!(window.narrow_state(), (false, true, false));
    assert_eq!(window.panes_shown(), (true, false));
    assert_eq!(window.narrow_overlay_open(), (false, false));
    // The panel opened over the content stays open when hiding the sidebar
    // brings it into the row.
    toggle(&window, "win.toggle-panel");
    assert_eq!(window.narrow_overlay_open(), (true, true));
    toggle(&window, "win.toggle-sidebar");
    assert_eq!(window.narrow_state(), (false, false, false));
    assert_eq!(window.panes_shown(), (false, true));
    assert_eq!(window.narrow_overlay_open(), (false, false));
    // The panel hidden in the row stays hidden over the content and back.
    toggle(&window, "win.toggle-panel");
    assert_eq!(window.panes_shown(), (false, false));
    toggle(&window, "win.toggle-sidebar");
    assert_eq!(window.narrow_state(), (false, true, false));
    toggle(&window, "win.toggle-sidebar");
    assert_eq!(window.narrow_state(), (false, false, false));
    assert_eq!(window.panes_shown(), (false, false));
    toggle(&window, "win.toggle-panel");
    assert_eq!(window.panes_shown(), (false, true));

    // Narrower than the content beside the panel alone: the panel stays over
    // the content whether the sidebar is shown or not.
    window.set_default_size(820, 900);
    wait_for("the panel over the content", 3, || {
        window.narrow_state() == (false, true, false)
    });
    toggle(&window, "win.toggle-sidebar");
    assert_eq!(window.narrow_state(), (false, true, false));
    assert_eq!(window.panes_shown(), (true, false));
    assert_eq!(window.narrow_overlay_open(), (false, false));

    // Narrower still, the sidebar goes over the content too, and F9 opens it
    // there whether or not it was shown before.
    window.set_default_size(700, 900);
    wait_for("both panes over the content", 3, || {
        window.narrow_state() == (true, true, true)
    });
    toggle(&window, "win.toggle-sidebar");
    assert_eq!(window.narrow_state(), (true, true, true));
    assert_eq!(window.narrow_overlay_open(), (true, true));
    toggle(&window, "win.toggle-sidebar");
    assert_eq!(window.narrow_overlay_open(), (false, false));

    // Wide again, both panes are back in the row as they were.
    window.set_default_size(1440, 900);
    wait_for("the wide layout", 3, || {
        window.narrow_state() == (false, false, false)
    });
    assert_eq!(window.panes_shown(), (true, true));
    window.destroy();
}

/// Calls `method` of the application's interface on `application` and waits
/// for the reply.
/// The widgets of `window` given less width than they need, which are cut
/// off: their type, style classes and the widths. Icons that ask for a little
/// more than their size are left out.
fn cut_off(window: &MainWindow) -> Vec<String> {
    descendants(window.upcast_ref())
        .into_iter()
        .filter(|w| w.is_drawable() && w.width() > 0)
        .filter(|w| {
            !matches!(
                w.type_().name(),
                "GtkImage" | "GtkGizmo" | "GtkBuiltinIcon" | "GtkWindowControls"
            )
        })
        .filter_map(|w| {
            let (min, ..) = w.measure(gtk::Orientation::Horizontal, -1);
            let bounds = w.compute_bounds(&w)?;
            let have = bounds.width() as i32 + w.margin_start() + w.margin_end();
            (min > have + 1)
                .then(|| format!("{} {:?}: {min} > {have}", w.type_().name(), w.css_classes()))
        })
        .collect()
}

fn pages_fit_the_narrowest_content(app: &gtk::Application) {
    // 788 - 228 = 560: the narrowest content beside the sidebar, the panel
    // over it.
    let (window, backend) = window_sized(app, 788);
    wait_for("the narrow layout", 3, || {
        window.narrow_state() == (false, true, false)
    });
    let check = |page: &str| {
        spin(400);
        let cut = cut_off(&window);
        assert!(cut.is_empty(), "{page} is cut off: {cut:#?}");
    };
    for page in [
        "home",
        "albums",
        "artists",
        "genres",
        "composers",
        "years",
        "folders",
    ] {
        window.open_page(page);
        check(page);
    }
    let albums = backend
        .library()
        .album_keys(&AlbumQuery::default())
        .unwrap();
    window.open_album(albums[0]);
    check("album");
    window.search_text("harbour");
    check("search");
    window.search_text("genre:jazz -genre:fusion");
    check("query");
    window.search_text("");
    window.destroy();
}

fn call(
    application: &gtk::Application,
    method: &str,
    args: Option<glib::Variant>,
) -> Result<glib::Variant, glib::Error> {
    let connection = application.dbus_connection().unwrap();
    let path = application.dbus_object_path().unwrap();
    let result = Rc::new(RefCell::new(None));
    let r = result.clone();
    connection.call(
        application.application_id().as_deref(),
        &path,
        helikodu_desktop::app_api::INTERFACE,
        method,
        args.as_ref(),
        None,
        gio::DBusCallFlags::NONE,
        5000,
        gio::Cancellable::NONE,
        move |reply| {
            r.replace(Some(reply));
        },
    );
    wait_for(method, 5, || result.borrow().is_some());
    result.take().unwrap()
}

fn json_reply(reply: &glib::Variant) -> serde_json::Value {
    let (text,) = reply.get::<(String,)>().unwrap();
    serde_json::from_str(&text).unwrap()
}

fn the_dbus_interface_answers(_app: &gtk::Application) {
    // An application of its own that owns its name on the session bus.
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.ApiTests")
        .build();
    application.register(gio::Cancellable::NONE).unwrap();
    let (window, _) = window_sized(&application, 1440);
    let status = json_reply(&call(&application, "GetStatus", None).unwrap());
    assert_eq!(status["state"], "stopped");
    assert_eq!(status["api_version"], 1);
    let found = call(
        &application,
        "PlayQuery",
        Some(("year>2020".to_owned(), false).to_variant()),
    )
    .unwrap();
    let (count,) = found.get::<(u32,)>().unwrap();
    assert!(count > 0, "the query finds tracks");
    wait_for("playback", 3, || window.is_playing());
    let status = json_reply(&call(&application, "GetStatus", None).unwrap());
    assert_eq!(status["state"], "playing");
    assert!(status["track"]["title"].is_string());
    let queue = json_reply(&call(&application, "GetQueue", None).unwrap());
    assert_eq!(
        queue["tracks"].as_array().unwrap().len() as u64,
        u64::from(count)
    );
    call(&application, "Rate", Some((0u64, 3i32).to_variant())).unwrap();
    let error = call(
        &application,
        "SetRepeat",
        Some(("sometimes".to_owned(),).to_variant()),
    )
    .unwrap_err();
    assert!(error.message().contains("no repeat mode"), "{error}");
    let playlists = json_reply(&call(&application, "ListPlaylists", None).unwrap());
    assert!(!playlists.as_array().unwrap().is_empty());
    call(&application, "Pause", None).unwrap();
    wait_for("the pause", 3, || !window.is_playing());
    window.destroy();
}

type Test = fn(&gtk::Application);

const TESTS: &[(&str, Test)] = &[
    (
        "the_facet_browser_narrows_the_table",
        the_facet_browser_narrows_the_table,
    ),
    (
        "statistics_count_and_open_tracks",
        statistics_count_and_open_tracks,
    ),
    (
        "a_narrow_window_lays_the_panes_over_the_content",
        a_narrow_window_lays_the_panes_over_the_content,
    ),
    (
        "a_hidden_sidebar_leaves_its_room_to_the_panel",
        a_hidden_sidebar_leaves_its_room_to_the_panel,
    ),
    ("the_dbus_interface_answers", the_dbus_interface_answers),
    (
        "pages_fit_the_narrowest_content",
        pages_fit_the_narrowest_content,
    ),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("insights: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("insights: skipped, no display");
        return ExitCode::SUCCESS;
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.InsightsTests")
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
    application.run_with_args(&["insights"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("insights: {} passed", ran.borrow());
        ExitCode::SUCCESS
    } else {
        println!("insights: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
