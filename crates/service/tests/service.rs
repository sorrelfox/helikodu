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

/// Writes two seconds of 16-bit stereo digital silence as a WAV file.
fn silent_wav(path: &Path) {
    let rate: u32 = 44_100;
    let data_len: u32 = rate * 2 * 4;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    b.resize(b.len() + data_len as usize, 0);
    fs::write(path, b).unwrap();
}

#[test]
fn output_and_loudness_settings_reach_the_playing_engine() {
    use helikodu_model::player::TransportState;
    use helikodu_model::settings::LoudnessMode;

    let dir = folder("live_audio_settings");
    let file = dir.join("silence.wav");
    silent_wav(&file);
    let service = start(&dir);
    let events = service.subscribe(SubscribeOptions::all());
    let transport = |wanted: TransportState| move |e: &Event| matches!(e, Event::Player(PlayerEvent::State { transport, .. }) if *transport == wanted);
    service
        .player()
        .send(PlayerCommand::PlayFiles {
            files: vec![file],
            start: 0,
        })
        .unwrap();
    until(&events, transport(TransportState::Playing));

    // Loudness keeps the stream; a new buffer length reopens it and plays on.
    service
        .settings()
        .update(|s| s.loudness.mode = LoudnessMode::Track)
        .unwrap();
    service
        .settings()
        .update(|s| s.output.buffer_ms = 400)
        .unwrap();
    until(&events, transport(TransportState::Reconfiguring));
    until(&events, transport(TransportState::Playing));
}

/// A rating is announced on the bus once it is stored, with the track and the
/// stars, so every list showing the track can update it in place.
#[test]
fn a_stored_rating_is_announced() {
    use std::sync::Arc;

    use helikodu_library::{DatabaseConfig, FileStamp, Library, TrackRecord, TrackTags};
    use helikodu_service::library::LibraryEvent;

    let dir = folder("rating_event");
    let inner = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let root = inner.add_root("/nonexistent-music").unwrap();
    let mut record = TrackRecord::new(
        root,
        "a.flac",
        FileStamp {
            size: 1000,
            mtime_ns: Some(1),
            inode: None,
            quick_hash: b"a.flac".to_vec(),
        },
    );
    record.tags = TrackTags {
        title: Some("A".into()),
        ..TrackTags::default()
    };
    let track = inner.upsert_track(record).unwrap().track;
    let dirs = AppDirs::under(&dir);
    fs::create_dir_all(&dirs.config).unwrap();
    fs::write(
        dirs.config.join("settings.toml"),
        "version = 1\n[output]\nbackend = \"null\"\n",
    )
    .unwrap();
    let mut config = ServiceConfig::new(dirs);
    config.library = LibraryChoice::Open(MusicLibrary::from_library(Arc::new(inner), ":memory:"));
    config.thumbnails = false;
    let service = Service::start(config).unwrap();
    let events = service.subscribe(SubscribeOptions {
        player: false,
        settings: false,
        ..SubscribeOptions::all()
    });
    let library = service.library().unwrap();
    for stars in [Some(4), None] {
        let (reply, answer) = mpsc::channel();
        library
            .rate(track, stars, move |result| {
                reply.send(result.is_ok()).unwrap()
            })
            .unwrap();
        assert!(answer.recv_timeout(Duration::from_secs(10)).unwrap());
        let event = until(&events, |e| {
            matches!(e, Event::Library(LibraryEvent::RatingChanged { .. }))
        });
        assert_eq!(
            event,
            Event::Library(LibraryEvent::RatingChanged { track, stars })
        );
    }
}
