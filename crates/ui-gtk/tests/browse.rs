// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Composers, Years, the audiobook page and folders outside the library, against
//! the sample backend.
//!
//! Like `views.rs`, this program has its own main function and runs only under
//! the headless compositor script:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test browse
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
use helikodu_ui_gtk::backend::{AlbumKey, Backend, MediaKind, PlayerRequest, sample};
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

fn window(application: &gtk::Application) -> (MainWindow, Backend) {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 300,
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

/// The texts of the mapped labels of the window.
fn shown_texts(window: &MainWindow) -> Vec<String> {
    descendants(window.upcast_ref())
        .into_iter()
        .filter(gtk::Widget::is_mapped)
        .filter_map(|w| w.downcast::<gtk::Label>().ok())
        .map(|l| l.text().to_string())
        .collect()
}

fn composers_and_years_list_and_play(app: &gtk::Application) {
    let (window, _) = window(app);
    for page in ["composers", "years"] {
        window.open_page(page);
        assert_eq!(window.visible_view().as_deref(), Some(page));
    }
    wait_for("the lists", 2, || {
        let (composers, years) = window.browse_counts();
        composers > 0 && years > 0
    });
    let year = window.browse_choose(true, 1);
    assert!(year.parse::<i32>().is_ok(), "a year row: {year}");
    let decade = window.browse_choose(true, 0);
    assert!(decade.ends_with('s'), "the first row is a decade: {decade}");
    window.open_page("composers");
    let composer = window.browse_choose(false, 0);
    assert!(!composer.is_empty());
    let play = descendants(window.upcast_ref())
        .into_iter()
        .filter(gtk::Widget::is_mapped)
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| {
            descendants(b.upcast_ref())
                .iter()
                .filter_map(|w| w.downcast_ref::<gtk::Label>())
                .any(|l| l.text() == "Play all")
        })
        .expect("Play all");
    assert!(!window.is_playing());
    play.emit_clicked();
    wait_for("playback", 2, || window.is_playing());
    window.destroy();
}

fn an_audiobook_shows_its_chapters(app: &gtk::Application) {
    let (window, backend) = window(app);
    let library = backend.library();
    library
        .set_album_kind(AlbumKey(2), MediaKind::Audiobook)
        .unwrap();
    window.open_album(AlbumKey(2));
    wait_for("the audiobook page", 2, || {
        let texts = shown_texts(&window);
        texts.iter().any(|t| t == "Audiobook")
            && texts.iter().any(|t| t == "Start again")
            && texts.iter().any(|t| t == "Starts at")
    });
    let texts = shown_texts(&window);
    assert!(texts.iter().any(|t| t.contains("% heard")), "{texts:?}");
    assert!(
        texts.iter().any(|t| t == "0:00:00"),
        "the first chapter starts at 0"
    );
    assert!(!texts.iter().any(|t| t == "Shuffle"));
    // Music again.
    library
        .set_album_kind(AlbumKey(2), MediaKind::Music)
        .unwrap();
    window.open_album(AlbumKey(1));
    window.open_album(AlbumKey(2));
    wait_for("the album page", 2, || {
        let texts = shown_texts(&window);
        texts.iter().any(|t| t == "Album") && texts.iter().any(|t| t == "Shuffle")
    });
    window.destroy();
}

fn folders_outside_the_library(app: &gtk::Application) {
    let (window, _) = window(app);
    window.open_page("folders");
    wait_for("the tree", 2, || window.list_counts().2 == 3);
    wait_for("the sections", 2, || {
        let texts = shown_texts(&window);
        texts.iter().any(|t| t == "Library folders")
            && texts.iter().any(|t| t == "This computer")
            && texts.iter().any(|t| t == "File system")
    });
    window.destroy();
}

fn a_year_plays(app: &gtk::Application) {
    let (window, backend) = window(app);
    let library = backend.library();
    let years = library.years().unwrap();
    let year = years[0].year;
    backend.send(PlayerRequest::PlayYears {
        years: helikodu_ui_gtk::backend::YearRange::of(year),
        shuffle: false,
    });
    wait_for("playback", 2, || window.is_playing());
    window.destroy();
}

type Test = (&'static str, fn(&gtk::Application));

const TESTS: &[Test] = &[
    (
        "composers_and_years_list_and_play",
        composers_and_years_list_and_play,
    ),
    (
        "an_audiobook_shows_its_chapters",
        an_audiobook_shows_its_chapters,
    ),
    ("folders_outside_the_library", folders_outside_the_library),
    ("a_year_plays", a_year_plays),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("browse: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("browse: skipped, no display");
        return ExitCode::SUCCESS;
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    prepare_display(SchemeChoice::Dark, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.BrowseTests")
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
    application.run_with_args(&["browse"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("browse: {} passed", ran.borrow());
        ExitCode::SUCCESS
    } else {
        println!("browse: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
