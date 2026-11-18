// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The whole service: settings, library, player and bus kept in step, and the library
//! service's queries off the calling thread.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use helikodu_model::audio::{BackendSelection, OutputBackendKind, OutputSelection};
use helikodu_model::library::TrackId;
use helikodu_service::bus::Subscription;
use helikodu_service::files::AppDirs;
use helikodu_service::library::{AlbumQuery, MusicLibrary};
use helikodu_service::player::{PlayerCommand, PlayerEvent};
use helikodu_service::settings::SettingsEvent;
use helikodu_service::{Event, LibraryChoice, Service, ServiceConfig, SubscribeOptions};

fn folder(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("service")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn null() -> OutputSelection {
    OutputSelection {
        backend: BackendSelection::Backend(OutputBackendKind::Null),
        ..OutputSelection::default()
    }
}

/// Starts a service in `dir` whose settings play to the null sink.
fn start(dir: &Path) -> Service {
    let dirs = AppDirs::under(dir);
    let settings = dirs.config.join("settings.toml");
    if !settings.exists() {
        fs::create_dir_all(&dirs.config).unwrap();
        fs::write(&settings, "version = 1\n[output]\nbackend = \"null\"\n").unwrap();
    }
    let mut config = ServiceConfig::new(dirs);
    config.library = LibraryChoice::Open(MusicLibrary::in_memory().unwrap());
    config.thumbnails = false;
    config.session_debounce = Duration::ZERO;
    Service::start(config).unwrap()
}

fn until(events: &Subscription<Event>, mut done: impl FnMut(&Event) -> bool) -> Event {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = events.recv_timeout(left).expect("timed out");
        if done(&event) {
            return event;
        }
    }
}

#[test]
fn settings_and_player_stay_in_step() {
    let dir = folder("in_step");
    let service = start(&dir);
    let events = service.subscribe(SubscribeOptions::all());
    assert_eq!(service.player().status().output, null());

    // A device chosen in the player is written to the settings.
    let wav = OutputSelection {
        backend: BackendSelection::Backend(OutputBackendKind::Wav),
        device: dir.join("out.wav").to_str().unwrap().parse().unwrap(),
        ..OutputSelection::default()
    };
    service
        .player()
        .send(PlayerCommand::SetOutput(wav.clone()))
        .unwrap();
    until(&events, |e| {
        matches!(e, Event::Settings(SettingsEvent::Changed { .. }))
    });
    assert_eq!(service.settings().get().output.selection(), wav);

    // A device chosen in the settings moves playback there.
    service
        .settings()
        .update(|s| s.output.backend = BackendSelection::Backend(OutputBackendKind::Null))
        .unwrap();
    let Event::Player(PlayerEvent::OutputChanged(selection)) = until(&events, |e| {
        matches!(e, Event::Player(PlayerEvent::OutputChanged(_)))
    }) else {
        unreachable!()
    };
    assert_eq!(
        selection.backend,
        BackendSelection::Backend(OutputBackendKind::Null)
    );
    assert!(
        fs::read_to_string(dir.join("config/helikodu/settings.toml"))
            .unwrap()
            .contains("backend = \"null\"")
    );
    drop(service);

    // The output survives a restart.
    let service = start(&dir);
    assert_eq!(
        service.player().status().output.backend,
        BackendSelection::Backend(OutputBackendKind::Null)
    );
}

#[test]
fn library_queries_answer_on_another_thread() {
    let dir = folder("library");
    let service = start(&dir);
    let library = service.library().unwrap();
    let caller = std::thread::current().id();
    let (reply, answer) = mpsc::channel();
    library
        .albums(AlbumQuery::default(), move |result| {
            reply
                .send((std::thread::current().id(), result.map(|page| page.total)))
                .unwrap();
        })
        .unwrap();
    let (thread, total) = answer.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_ne!(thread, caller);
    assert_eq!(total.unwrap(), 0);

    let events = service.subscribe(SubscribeOptions {
        player: false,
        settings: false,
        ..SubscribeOptions::all()
    });
    let (reply, answer) = mpsc::channel();
    library
        .rate(TrackId::new(42), Some(4), move |result| {
            reply.send(result.is_err()).unwrap()
        })
        .unwrap();
    assert!(
        answer.recv_timeout(Duration::from_secs(10)).unwrap(),
        "no such track"
    );
    assert!(events.try_recv().is_err(), "nothing was rated");
    let (reply, answer) = mpsc::channel();
    library
        .rate(TrackId::new(1), Some(9), move |result| {
            reply.send(result.is_err()).unwrap()
        })
        .unwrap();
    assert!(
        answer.recv_timeout(Duration::from_secs(10)).unwrap(),
        "9 stars"
    );
}
