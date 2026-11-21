// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings store on disk: round trip, migration of an old file, bad and newer
//! files, change notifications, reset, and the window state.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};

use helikodu_model::settings::{Accent, LoudnessMode, Settings, SettingsGroup, UiState};
use helikodu_service::Event;
use helikodu_service::bus::EventBus;
use helikodu_service::settings::{
    LoadNotice, SettingsEvent, SettingsStore, SettingsStoreError, SettingsWatcher, UiStateStore,
};

fn folder(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("service-settings")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_language_is_saved_and_read_back() {
    let path = folder("language").join("settings.toml");
    let store = SettingsStore::open(&path, EventBus::new());
    assert_eq!(store.get().appearance.language, "");
    for language in ["pt_BR", "zh_CN", ""] {
        let groups = store
            .update(|s| s.appearance.language = language.to_owned())
            .unwrap();
        assert_eq!(groups, vec![SettingsGroup::Appearance]);
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains(&format!("language = \"{language}\"")),
            "{text}"
        );
        let again = SettingsStore::open(&path, EventBus::new());
        assert_eq!(again.load_notice(), None);
        assert_eq!(again.get().appearance.language, language);
    }
}

#[test]
fn changes_are_saved_announced_and_read_back() {
    let path = folder("round_trip").join("settings.toml");
    let bus = EventBus::<Event>::new();
    let events = bus.subscribe_with(|e: &Event| Some(e.clone()), None);
    let store = SettingsStore::open(&path, bus.clone());
    assert_eq!(store.load_notice(), None);
    assert_eq!(*store.get(), Settings::default());
    assert!(!path.exists(), "nothing is written until something changes");

    let groups = store
        .update(|s| {
            s.appearance.accent = Accent::Green;
            s.loudness.mode = LoudnessMode::Track;
            s.library.folders = vec!["/music".into()];
        })
        .unwrap();
    assert_eq!(
        groups,
        vec![
            SettingsGroup::Library,
            SettingsGroup::Loudness,
            SettingsGroup::Appearance
        ]
    );
    let Event::Settings(SettingsEvent::Changed {
        groups: announced,
        settings,
    }) = events.try_recv().unwrap()
    else {
        panic!("a settings event")
    };
    assert_eq!(announced, groups);
    assert_eq!(settings.appearance.accent, Accent::Green);

    // An unchanged update writes and announces nothing.
    assert_eq!(store.update(|_| {}).unwrap(), vec![]);
    assert!(events.try_recv().is_err());

    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("version = 2"), "{text}");
    assert!(text.contains("accent = \"green\""), "{text}");
    let again = SettingsStore::open(&path, EventBus::new());
    assert_eq!(again.get(), store.get());

    // An invalid value is refused and changes nothing.
    let error = store.update(|s| s.playback.seek_step_s = 0).unwrap_err();
    assert!(matches!(error, SettingsStoreError::Invalid(ref e) if e.key == "playback.seek_step_s"));
    assert_eq!(store.get().playback.seek_step_s, 5);

    // Reset keeps the library folders.
    store.reset().unwrap();
    assert_eq!(store.get().appearance.accent, Accent::Desktop);
    assert_eq!(store.get().library.folders, vec![PathBuf::from("/music")]);
}

#[test]
fn an_old_file_is_migrated() {
    let path = folder("migrate").join("settings.toml");
    // Written before the settings had a version: only the output table.
    fs::write(&path, "[output]\nbackend = \"alsa\"\nbuffer_ms = 300\n").unwrap();
    let store = SettingsStore::open(&path, EventBus::new());
    assert_eq!(store.load_notice(), None);
    let settings = store.get();
    assert_eq!(settings.version, Settings::VERSION);
    assert_eq!(settings.output.buffer_ms, 300);
    store.update(|s| s.playback.seek_step_s = 10).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("version = 2") && text.contains("buffer_ms = 300"),
        "{text}"
    );
}

#[test]
fn bad_files_give_defaults_and_are_kept() {
    let dir = folder("bad");
    let path = dir.join("settings.toml");
    fs::write(&path, "this is [not toml").unwrap();
    let store = SettingsStore::open(&path, EventBus::new());
    assert!(matches!(
        store.load_notice(),
        Some(LoadNotice::Unreadable { .. })
    ));
    assert_eq!(*store.get(), Settings::default());
    store.update(|s| s.appearance.stripe_rows = false).unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("settings.toml.bak")).unwrap(),
        "this is [not toml"
    );

    // A value out of range resets its table only.
    fs::write(
        &path,
        "version = 1\n[playback]\nseek_step_s = 999\n[appearance]\naccent = \"blue\"\n",
    )
    .unwrap();
    let store = SettingsStore::open(&path, EventBus::new());
    let Some(LoadNotice::Reset { problems }) = store.load_notice() else {
        panic!("{:?}", store.load_notice())
    };
    assert_eq!(problems[0].key, "playback.seek_step_s");
    assert_eq!(store.get().playback.seek_step_s, 5);
    assert_eq!(store.get().appearance.accent, Accent::Blue);
}

#[test]
fn a_newer_file_is_not_overwritten() {
    let path = folder("newer").join("settings.toml");
    let text = "version = 7\n[appearance]\naccent = \"violet\"\n[future]\nkey = 1\n";
    fs::write(&path, text).unwrap();
    let store = SettingsStore::open(&path, EventBus::new());
    assert_eq!(store.load_notice(), Some(&LoadNotice::Newer { version: 7 }));
    assert_eq!(store.get().appearance.accent, Accent::Violet);
    assert!(matches!(
        store.update(|s| s.appearance.accent = Accent::Amber),
        Err(SettingsStoreError::NewerFile(7))
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn external_edits_are_reloaded() {
    let path = folder("reload").join("settings.toml");
    let bus = EventBus::<Event>::new();
    let events = bus.subscribe_with(|e: &Event| Some(e.clone()), None);
    let store = SettingsStore::open(&path, bus.clone());
    fs::write(&path, "version = 1\n[appearance]\nmenu_bar = false\n").unwrap();
    assert_eq!(store.reload(), None);
    assert!(!store.get().appearance.menu_bar);
    assert!(matches!(
        events.try_recv().unwrap(),
        Event::Settings(SettingsEvent::Changed { ref groups, .. }) if *groups == vec![SettingsGroup::Appearance]
    ));
}

#[test]
fn the_window_state_is_kept_in_range() {
    let path = folder("ui_state").join("ui-state.toml");
    let store = UiStateStore::new(&path);
    assert_eq!(store.load(), UiState::default());
    let mut state = UiState::default();
    state.panes.sidebar_width = 300;
    state.panes.queue_visible = false;
    state.last_view = "albums".into();
    store.save(&state).unwrap();
    assert_eq!(store.load(), state);
    fs::write(&path, "[panes]\nqueue_width = 5\n").unwrap();
    assert_eq!(store.load().panes.queue_width, 260);
    fs::write(&path, "not toml at all [").unwrap();
    assert_eq!(store.load(), UiState::default());
}

#[test]
fn the_dsp_table_is_saved_and_a_bad_one_reset() {
    use helikodu_model::settings::{EqMode, ParametricFilter};

    let path = folder("dsp").join("settings.toml");
    let store = SettingsStore::open(&path, EventBus::new());
    let groups = store
        .update(|s| {
            s.dsp.equalizer.enabled = true;
            s.dsp.equalizer.mode = EqMode::Graphic10;
            s.dsp.equalizer.graphic_10[4] = -3.5;
            s.dsp.equalizer.parametric = vec![ParametricFilter::default(); 3];
            s.dsp.limiter.enabled = true;
        })
        .unwrap();
    assert_eq!(groups, vec![SettingsGroup::Dsp]);
    let again = SettingsStore::open(&path, EventBus::new());
    assert_eq!(again.get(), store.get());
    let error = store
        .update(|s| s.dsp.limiter.lookahead_ms = 9.0)
        .unwrap_err();
    assert!(
        matches!(error, SettingsStoreError::Invalid(ref e) if e.key == "dsp.limiter.lookahead_ms")
    );

    // A value out of range in the file resets the `[dsp]` table only.
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("lookahead_ms = 2.0", "lookahead_ms = 9.0");
    fs::write(&path, text).unwrap();
    let reread = SettingsStore::open(&path, EventBus::new());
    assert!(matches!(
        reread.load_notice(),
        Some(LoadNotice::Reset { .. })
    ));
    assert_eq!(reread.get().dsp, Settings::default().dsp);
}

/// The next event about the settings within `wait`, if any.
fn next_settings_event(
    events: &helikodu_service::bus::Subscription<Event>,
    wait: std::time::Duration,
) -> Option<SettingsEvent> {
    let end = std::time::Instant::now() + wait;
    loop {
        let left = end.saturating_duration_since(std::time::Instant::now());
        match events.recv_timeout(left) {
            Ok(Event::Settings(event)) => return Some(event),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

#[test]
fn the_watcher_reads_edits_from_outside() {
    let path = folder("watch_outside").join("settings.toml");
    let bus = EventBus::<Event>::new();
    let events = bus.subscribe_with(|e: &Event| Some(e.clone()), None);
    let store = std::sync::Arc::new(SettingsStore::open(&path, bus.clone()));
    let _watcher =
        SettingsWatcher::start(store.clone(), std::time::Duration::from_millis(50)).unwrap();
    // Another program replaces the file, as the command line does.
    let other = SettingsStore::open(&path, EventBus::new());
    other.update(|s| s.dsp.limiter.enabled = true).unwrap();
    let event = next_settings_event(&events, std::time::Duration::from_secs(5));
    assert!(
        matches!(
            event,
            Some(SettingsEvent::Changed { ref groups, ref settings })
                if *groups == vec![SettingsGroup::Dsp] && settings.dsp.limiter.enabled
        ),
        "{event:?}"
    );
    assert!(store.get().dsp.limiter.enabled);
    // A text editor writes in place.
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("menu_bar = true", "menu_bar = false");
    fs::write(&path, text).unwrap();
    let event = next_settings_event(&events, std::time::Duration::from_secs(5));
    assert!(
        matches!(event, Some(SettingsEvent::Changed { ref groups, .. }) if *groups == vec![SettingsGroup::Appearance]),
        "{event:?}"
    );
    assert!(!store.get().appearance.menu_bar);
}

#[test]
fn the_stores_own_writes_are_not_read_again() {
    let path = folder("watch_own").join("settings.toml");
    let bus = EventBus::<Event>::new();
    let events = bus.subscribe_with(|e: &Event| Some(e.clone()), None);
    let store = std::sync::Arc::new(SettingsStore::open(&path, bus.clone()));
    let _watcher =
        SettingsWatcher::start(store.clone(), std::time::Duration::from_millis(50)).unwrap();
    store.update(|s| s.dsp.limiter.enabled = true).unwrap();
    assert!(matches!(
        next_settings_event(&events, std::time::Duration::from_secs(1)),
        Some(SettingsEvent::Changed { .. })
    ));
    // Only the change itself is announced; the watcher's read finds the file as
    // it was written.
    assert_eq!(
        next_settings_event(&events, std::time::Duration::from_millis(600)),
        None
    );
    assert_eq!(store.reload(), None);
    assert_eq!(
        next_settings_event(&events, std::time::Duration::from_millis(50)),
        None
    );
}

#[test]
fn a_broken_edit_leaves_the_settings_as_they_were() {
    let path = folder("watch_broken").join("settings.toml");
    let bus = EventBus::<Event>::new();
    let events = bus.subscribe_with(|e: &Event| Some(e.clone()), None);
    let store = std::sync::Arc::new(SettingsStore::open(&path, bus.clone()));
    store.update(|s| s.dsp.limiter.enabled = true).unwrap();
    let _ = next_settings_event(&events, std::time::Duration::from_secs(1));
    let _watcher =
        SettingsWatcher::start(store.clone(), std::time::Duration::from_millis(50)).unwrap();
    fs::write(&path, "[dsp\nlimiter = ").unwrap();
    let event = next_settings_event(&events, std::time::Duration::from_secs(5));
    assert!(
        matches!(
            event,
            Some(SettingsEvent::Rejected {
                notice: LoadNotice::Unreadable { .. }
            })
        ),
        "{event:?}"
    );
    assert!(store.get().dsp.limiter.enabled);
    // A value out of range would reset its table: also kept out.
    fs::write(&path, "version = 1\n[playback]\nseek_step_s = 100000\n").unwrap();
    let event = next_settings_event(&events, std::time::Duration::from_secs(5));
    assert!(
        matches!(
            event,
            Some(SettingsEvent::Rejected {
                notice: LoadNotice::Reset { .. }
            })
        ),
        "{event:?}"
    );
    assert_eq!(store.get().playback, Settings::default().playback);
    assert!(store.get().dsp.limiter.enabled);
    // The next change keeps the broken file beside the new one.
    store
        .update(|s| s.appearance.accent = Accent::Amber)
        .unwrap();
    assert!(
        fs::read_to_string(path.with_extension("toml.bak"))
            .unwrap()
            .contains("seek_step_s = 100000")
    );
    // A good edit is read again.
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("menu_bar = true", "menu_bar = false");
    fs::write(&path, text).unwrap();
    let mut seen = None;
    while let Some(event) = next_settings_event(&events, std::time::Duration::from_secs(5)) {
        if let SettingsEvent::Changed { groups, .. } = &event
            && *groups == vec![SettingsGroup::Appearance]
            && !store.get().appearance.menu_bar
        {
            seen = Some(event);
            break;
        }
    }
    assert!(seen.is_some());
}
