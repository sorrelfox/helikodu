// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Generator playlists and tracks that sound alike against the sample backend:
//! the generator in the sidebar and the New playlist menu, its page with the
//! steps line and Make again, the editor with the tracks
//! left after each step, moving and adding steps, a step that cannot run, and
//! the page of tracks that sound alike with "Show 50 more", "Save as
//! playlist", the context menu's "Show Similar Tracks" and the not-yet page;
//! the completion under the steps' query entries, a generator's place among
//! the playlists of its folder, and both tables in the narrow window.
//!
//! Like `views.rs`, this program has its own main function and runs only under
//! the headless compositor script:
//!
//! ```sh
//! crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test generators
//! ```
//!
//! With `HELIKODU_GENERATOR_SHOTS=DIR` it saves the screens to `DIR`.

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
use helikodu_ui_gtk::backend::{
    AlbumKey, ArtistKey, Backend, PlaylistType, SimilarSeed, TrackKey, sample,
};
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

fn window_with(application: &gtk::Application, albums: usize) -> (MainWindow, Backend) {
    let backend = sample::backend(&sample::SampleOptions {
        albums,
        playing: None,
    });
    install_actions(application, &backend);
    let window = MainWindow::new(application, &backend, WindowStyle::Integrated);
    window.set_default_size(1440, 900);
    window.present();
    spin(150);
    (window, backend)
}

fn window(application: &gtk::Application) -> (MainWindow, Backend) {
    window_with(application, 300)
}

/// Saves the window to `HELIKODU_GENERATOR_SHOTS/name.png` when that is set.
fn shot(window: &MainWindow, name: &str) {
    let Some(dir) = std::env::var_os("HELIKODU_GENERATOR_SHOTS") else {
        return;
    };
    spin(300);
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    snapshot::save_png(window, &path).unwrap();
}

/// Opens the sample's generator playlist and waits for its tracks.
fn open_forgotten(window: &MainWindow) {
    wait_for("the playlists", 3, || {
        window
            .playlist_names()
            .iter()
            .any(|n| n == "Forgotten favourites")
    });
    assert!(window.open_playlist_named("Forgotten favourites", false));
    wait_for("the generator's tracks", 3, || {
        window.generator_state().tracks == 50
    });
}

fn the_generator_page_shows_its_steps(app: &gtk::Application) {
    let (window, backend) = window(app);
    open_forgotten(&window);
    assert_eq!(window.visible_view().as_deref(), Some("generator-playlist"));
    let state = window.generator_state();
    let chips: Vec<&str> = state.chips.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        chips,
        [
            "From the library",
            "Rating is at least 4 stars",
            "Not in “Heard this week”",
            "Longest unplayed first",
            "At most 2 per artist",
            "At most 1 per album",
            "50 tracks",
        ]
    );
    assert!(state.chips.iter().all(|(_, problem)| !problem));
    assert!(
        state.summary.starts_with("50 tracks · "),
        "{}",
        state.summary
    );
    assert!(
        state.summary.ends_with("made just now"),
        "{}",
        state.summary
    );
    assert!(
        state.status.ends_with("made again each time it is opened"),
        "{}",
        state.status
    );
    assert_eq!(
        state.headers,
        ["#", "Title", "Artist", "Album", "Last played ↑", "Length"]
    );
    // It is listed among the playlists with its own kind and count.
    let row = backend
        .library()
        .playlists()
        .unwrap()
        .into_iter()
        .find(|p| p.name == "Forgotten favourites")
        .unwrap();
    assert_eq!(row.kind, PlaylistType::Generator);
    assert_eq!(row.items, 50);
    shot(&window, "generator");

    // Make again keeps the count and reports nothing while the page is open.
    window.generator_make_again();
    wait_for("the new result", 3, || {
        window.generator_state().summary.ends_with("made just now")
    });
    assert_eq!(window.generator_state().tracks, 50);
    assert_eq!(window.toast_text(), None);
    window.close();
}

fn the_editor_counts_and_moves_steps(app: &gtk::Application) {
    let (window, _) = window(app);
    open_forgotten(&window);
    window.generator_edit(true);
    wait_for("the counts", 3, || {
        let state = window.generator_state();
        state.steps.len() == 7 && state.steps.iter().all(|(_, n)| !n.is_empty())
    });
    let state = window.generator_state();
    assert!(state.editing);
    let kinds: Vec<&str> = state.steps.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "From", "Where", "Exclude", "Sort by", "At most", "At most", "Limit"
        ]
    );
    assert_eq!(state.steps.last().unwrap().1, "50");
    assert!(state.results.starts_with("50 tracks"), "{}", state.results);
    assert!(state.results.ends_with("As the steps are now"));
    shot(&window, "generator-edit");

    // Alt+Down on the limit does nothing; Alt+Up moves it above the caps.
    window.generator_move_step(5, false);
    window.generator_move_step(5, true);
    wait_for("the moved step", 3, || {
        window
            .generator_state()
            .draft
            .contains("limit 50\nper-album 1")
    });
    // Each step works on what the one before left: the limit first leaves
    // fewer after the album cap.
    wait_for("the new counts", 3, || {
        let steps = window.generator_state().steps;
        steps.len() == 7 && steps[5].1 == "50" && !steps[6].1.is_empty()
    });
    // A new step goes at the end.
    window.generator_add_step(4);
    wait_for("the added step", 3, || {
        window.generator_state().steps.len() == 8
    });
    assert_eq!(window.generator_state().steps[7].0, "Limit");
    // Done editing keeps the steps.
    window.generator_edit(false);
    wait_for("the stored steps", 3, || {
        let state = window.generator_state();
        !state.editing && state.chips.len() == 8
    });
    window.close();
}

fn a_missing_playlist_keeps_the_last_result(app: &gtk::Application) {
    let (window, backend) = window(app);
    open_forgotten(&window);
    let heard = backend
        .library()
        .playlists()
        .unwrap()
        .into_iter()
        .find(|p| p.name == "Heard this week")
        .unwrap();
    backend.library().delete_playlist(heard.key).unwrap();
    window.generator_make_again();
    wait_for("the warning", 3, || {
        window.generator_state().chips.iter().any(|(_, p)| *p)
    });
    let state = window.generator_state();
    assert_eq!(state.tracks, 50, "the last result stays");
    assert!(state.summary.ends_with("could not be made again"));
    assert!(state.chips[2].1);
    assert_eq!(state.chips[2].0, "Not in a playlist that no longer exists");
    window.generator_edit(true);
    wait_for("the notice", 3, || {
        window.generator_state().notice.is_some()
    });
    let state = window.generator_state();
    assert_eq!(
        state.notice.as_deref(),
        Some("The playlist “Heard this week” no longer exists")
    );
    assert!(state.results.contains("As made"), "{}", state.results);
    shot(&window, "generator-error");
    window.close();
}

fn a_new_generator_opens_in_the_editor(app: &gtk::Application) {
    let (window, _) = window(app);
    wait_for("the playlists", 3, || !window.playlist_names().is_empty());
    let menu = window.demo_new_menu();
    let mut labels = Vec::new();
    for section in 0..menu.n_items() {
        let inner = menu.item_link(section, "section").unwrap();
        for i in 0..inner.n_items() {
            labels.push(
                inner
                    .item_attribute_value(i, "label", Some(glib::VariantTy::STRING))
                    .and_then(|v| v.get::<String>())
                    .unwrap(),
            );
        }
    }
    assert_eq!(
        labels,
        ["Playlist", "Smart Playlist", "Generator Playlist", "Folder"]
    );
    window.demo_new_generator();
    wait_for("the new generator", 3, || {
        window
            .playlist_names()
            .iter()
            .any(|n| n == "New generator playlist")
    });
    wait_for("the editor", 3, || {
        let state = window.generator_state();
        state.editing && state.steps.len() == 1
    });
    assert_eq!(window.generator_state().steps[0].0, "From");
    assert_eq!(window.generator_state().draft, "from library\n");
    window.close();
}

fn similar_tracks_list_and_grow(app: &gtk::Application) {
    let (window, backend) = window(app);
    window.demo_show_similar(SimilarSeed::Track(TrackKey(2)));
    assert_eq!(window.visible_view().as_deref(), Some("similar"));
    wait_for("the similar tracks", 3, || {
        window.similar_state().tracks == 50
    });
    let state = window.similar_state();
    assert!(state.more);
    assert!(state.buttons_sensitive);
    assert!(
        state.status.starts_with("The 50 closest tracks · "),
        "{}",
        state.status
    );
    assert!(
        state
            .subtitle
            .ends_with("the 50 tracks that sound most like it"),
        "{}",
        state.subtitle
    );
    assert_eq!(
        state.headers,
        ["#", "Title", "Artist", "Album", "How alike ↓", "Length"]
    );
    assert!(
        state
            .words
            .iter()
            .all(|w| { ["Very close", "Close", "Somewhat alike"].contains(&w.as_str()) })
    );
    assert_eq!(state.words.first().map(String::as_str), Some("Very close"));
    shot(&window, "similar");
    window.similar_more();
    wait_for("50 more", 3, || window.similar_state().tracks > 50);
    let all = window.similar_state().tracks;
    // Save as playlist makes a normal playlist of the listed tracks.
    window.similar_save();
    wait_for("the saved playlist", 3, || {
        window
            .playlist_names()
            .iter()
            .any(|n| n.starts_with("Sounds like "))
    });
    let saved = backend
        .library()
        .playlists()
        .unwrap()
        .into_iter()
        .find(|p| p.name.starts_with("Sounds like "))
        .unwrap();
    assert_eq!(saved.kind, PlaylistType::Static);
    assert_eq!(saved.items, all);
    // An album and an artist are seeds too.
    window.demo_show_similar(SimilarSeed::Album(AlbumKey(3)));
    wait_for("the album's list", 3, || {
        window.similar_state().subtitle.starts_with("Album by ")
    });
    window.demo_show_similar(SimilarSeed::Artist(ArtistKey(2)));
    wait_for("the artist's list", 3, || {
        window.similar_state().subtitle.starts_with("Artist")
    });
    window.close();
}

fn the_context_menu_shows_similar_tracks(app: &gtk::Application) {
    let (window, _) = window(app);
    wait_for("the playlists", 3, || !window.playlist_names().is_empty());
    let (shown, enabled) = window.demo_similar_item(&[TrackKey(3), TrackKey(4)]);
    assert!(shown);
    assert!(!enabled, "insensitive with several tracks");
    let (shown, enabled) = window.demo_similar_item(&[TrackKey(3)]);
    assert!(shown && enabled);
    WidgetExt::activate_action(&window, "win.sel-similar", None).unwrap();
    wait_for("the similar page", 3, || {
        window.visible_view().as_deref() == Some("similar")
    });
    window.close();
}

fn similar_tracks_not_yet(app: &gtk::Application) {
    let (window, _) = window(app);
    // One sample track in seven is not analysed yet, one could not be read.
    window.demo_show_similar(SimilarSeed::Track(TrackKey(8)));
    wait_for("the not-yet page", 3, || {
        window.similar_state().not_yet.is_some()
    });
    let state = window.similar_state();
    assert_eq!(
        state.not_yet.as_deref(),
        Some("This track has not been analysed yet")
    );
    assert!(!state.buttons_sensitive);
    assert!(state.progress);
    assert_eq!(state.settings_button, "Open the analysis settings");
    shot(&window, "similar-not-yet");
    window.demo_show_similar(SimilarSeed::Track(TrackKey(7)));
    wait_for("the unreadable track", 3, || {
        window.similar_state().not_yet.as_deref() == Some("This track could not be analysed")
    });
    window.close();
    // A small library has too few tracks analysed.
    let (window, _) = window_with(app, 12);
    window.demo_show_similar(SimilarSeed::Track(TrackKey(2)));
    wait_for("too few", 3, || {
        window.similar_state().not_yet.as_deref() == Some("Too few tracks are analysed so far")
    });
    window.close();
}

fn step_queries_complete(app: &gtk::Application) {
    let (window, _) = window(app);
    open_forgotten(&window);
    window.generator_edit(true);
    wait_for("the steps", 3, || window.generator_state().steps.len() == 7);
    // The Where step's entry lists the fields that start with the typed word.
    wait_for("the completion", 3, || {
        window
            .demo_generator_completion(0, "rating>=4 pla")
            .iter()
            .any(|n| n == "plays")
    });
    // A Where step added at the end has it too.
    window.generator_add_step(0);
    wait_for("the added step", 3, || {
        window.generator_state().steps.len() == 8
    });
    wait_for("its completion", 3, || {
        window
            .demo_generator_completion(1, "rat")
            .iter()
            .any(|n| n == "rating")
    });
    window.close();
}

fn generators_take_their_place_in_folders(app: &gtk::Application) {
    let (window, backend) = window(app);
    open_forgotten(&window);
    let key_of = |name: &str| {
        backend
            .library()
            .playlists()
            .unwrap()
            .into_iter()
            .find(|p| p.name == name)
            .unwrap()
    };
    WidgetExt::activate_action(&window, "win.new-playlist-folder", Some(&0u64.to_variant()))
        .unwrap();
    wait_for("the folder", 3, || {
        window.playlist_names().iter().any(|n| n == "New folder")
    });
    let folder = key_of("New folder").key;
    let generator = key_of("Forgotten favourites").key;
    let heard = key_of("Heard this week").key;
    let moved = |key: helikodu_ui_gtk::backend::PlaylistKey| {
        WidgetExt::activate_action(
            &window,
            "win.playlist-move",
            Some(&format!("{}/{}", key.0, folder.0).to_variant()),
        )
        .unwrap();
    };
    // Dropped on a folder, the generator goes after what is there, as a
    // playlist does, and keeps its place when another follows it.
    moved(generator);
    wait_for("the generator in the folder", 3, || {
        key_of("Forgotten favourites").parent == Some(folder)
    });
    moved(heard);
    wait_for("the playlist in the folder", 3, || {
        key_of("Heard this week").parent == Some(folder)
    });
    let names = window.playlist_names();
    let at = |name: &str| names.iter().position(|n| n == name).unwrap();
    assert_eq!(at("Forgotten favourites"), at("New folder") + 1);
    assert_eq!(at("Heard this week"), at("Forgotten favourites") + 1);
    // Pinned in its folder, and still pinned at the top again.
    WidgetExt::activate_action(&window, "win.playlist-pin", Some(&generator.0.to_variant()))
        .unwrap();
    wait_for("the pin", 3, || {
        window
            .pin_names()
            .iter()
            .any(|n| n == "Forgotten favourites")
    });
    WidgetExt::activate_action(
        &window,
        "win.playlist-move",
        Some(&format!("{}/0", generator.0).to_variant()),
    )
    .unwrap();
    wait_for("the generator at the top", 3, || {
        key_of("Forgotten favourites").parent.is_none()
    });
    assert_eq!(
        window.playlist_names().last().map(String::as_str),
        Some("Forgotten favourites")
    );
    assert!(
        window
            .pin_names()
            .iter()
            .any(|n| n == "Forgotten favourites")
    );
    window.close();
}

fn narrow_tables_keep_their_columns(app: &gtk::Application) {
    let backend = sample::backend(&sample::SampleOptions {
        albums: 300,
        playing: None,
    });
    install_actions(app, &backend);
    let window = MainWindow::new(app, &backend, WindowStyle::Integrated);
    window.set_default_size(640, 900);
    window.present();
    wait_for("the narrow window", 3, || window.narrow_state().2);
    open_forgotten(&window);
    wait_for("the narrow columns", 3, || {
        window.generator_state().headers == ["#", "Title", "Length"]
    });
    wait_for("the artist under the title", 3, || {
        window
            .generator_state()
            .artist_under
            .is_some_and(|a| !a.is_empty())
    });
    shot(&window, "generator-narrow");
    window.demo_show_similar(SimilarSeed::Track(TrackKey(2)));
    wait_for("the similar tracks", 3, || {
        window.similar_state().tracks == 50
    });
    wait_for("the narrow similar columns", 3, || {
        let state = window.similar_state();
        state.headers == ["Title", "How alike ↓", "Length"]
            && state.row_shape.0.is_some_and(|a| !a.is_empty())
    });
    assert!(
        !window.similar_state().row_shape.1,
        "no bar in the narrow window"
    );
    shot(&window, "similar-narrow");
    window.close();
    // A wide window shows the full tables.
    let (wide, _) = window_with(app, 300);
    wide.demo_show_similar(SimilarSeed::Track(TrackKey(2)));
    wait_for("the wide similar columns", 3, || {
        let state = wide.similar_state();
        state.headers.len() == 6 && state.row_shape == (None, true)
    });
    wide.close();
}

type Test = fn(&gtk::Application);

const TESTS: &[(&str, Test)] = &[
    (
        "the_generator_page_shows_its_steps",
        the_generator_page_shows_its_steps,
    ),
    (
        "the_editor_counts_and_moves_steps",
        the_editor_counts_and_moves_steps,
    ),
    (
        "a_missing_playlist_keeps_the_last_result",
        a_missing_playlist_keeps_the_last_result,
    ),
    (
        "a_new_generator_opens_in_the_editor",
        a_new_generator_opens_in_the_editor,
    ),
    ("similar_tracks_list_and_grow", similar_tracks_list_and_grow),
    (
        "the_context_menu_shows_similar_tracks",
        the_context_menu_shows_similar_tracks,
    ),
    ("similar_tracks_not_yet", similar_tracks_not_yet),
    ("step_queries_complete", step_queries_complete),
    (
        "generators_take_their_place_in_folders",
        generators_take_their_place_in_folders,
    ),
    (
        "narrow_tables_keep_their_columns",
        narrow_tables_keep_their_columns,
    ),
];

fn main() -> ExitCode {
    if std::env::var_os("HELIKODU_GTK_TESTS").is_none() {
        println!("generators: skipped; run under crates/ui-gtk/tests/headless.sh");
        return ExitCode::SUCCESS;
    }
    if gtk::init().is_err() {
        println!("generators: skipped, no display");
        return ExitCode::SUCCESS;
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let scheme = if std::env::var_os("HELIKODU_GENERATOR_LIGHT").is_some() {
        SchemeChoice::Light
    } else {
        SchemeChoice::Dark
    };
    prepare_display(scheme, AccentChoice::Amber);
    let application = gtk::Application::builder()
        .application_id("io.github.sorrelfox.Helikodu.GeneratorTests")
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
    application.run_with_args(&["generators"]);
    let failures = failures.borrow();
    if failures.is_empty() {
        println!("generators: {} passed", ran.borrow());
        ExitCode::SUCCESS
    } else {
        println!("generators: failed: {}", failures.join(", "));
        ExitCode::FAILURE
    }
}
