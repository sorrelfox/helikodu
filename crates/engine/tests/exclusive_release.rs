// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! An exclusive device is given back to other programs when playback stops, and
//! after it has been paused for `exclusive_release_s` (never with 0).
//!
//! A scripted backend stands for a sound card that has to be reserved, as the ALSA
//! backend does: opening an exclusive stream takes the card from a fake reservation,
//! and the stream holds the guard until it is stopped or dropped. Its streams
//! discard the audio in real time.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use helikodu_engine::output::{
    BackendConnector, DeviceReservation, EventSender, NullBackend, OpenedStream, OutputBackend,
    OutputError, OutputStream, ReservationError, ReservationGuard, ReservedCard, SharedConnector,
    StreamConfig, StreamState, StreamTiming,
};
use helikodu_engine::transport::{Engine, EngineConfig};
use helikodu_model::audio::{
    AudioSpec, BackendSelection, DeviceCapabilities, DeviceId, DeviceInfo, DeviceSelection,
    MediaTime, OutputBackendKind, OutputMode,
};
use helikodu_model::library::MediaSource;
use helikodu_model::player::{EngineCommand, EngineEvent, ItemId, PlaybackItem, TransportState};

const KIND: OutputBackendKind = OutputBackendKind::Alsa;
const CARD: &str = "hw:CARD=DAC,DEV=0";
const PATIENCE: Duration = Duration::from_secs(10);

/// A reservation that records who takes and gives back the card.
#[derive(Debug, Clone, Default)]
struct FakeReservation {
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl FakeReservation {
    fn log(&self) -> Vec<&'static str> {
        self.log.lock().unwrap().clone()
    }

    fn held(&self) -> bool {
        self.log().last() == Some(&"acquire")
    }
}

/// Gives the card back when dropped with the guard.
#[derive(Debug)]
struct Holder(Arc<Mutex<Vec<&'static str>>>);

impl Drop for Holder {
    fn drop(&mut self) {
        self.0.lock().unwrap().push("release");
    }
}

impl DeviceReservation for FakeReservation {
    fn acquire(&self, _card: &ReservedCard) -> Result<ReservationGuard, ReservationError> {
        self.log.lock().unwrap().push("acquire");
        Ok(ReservationGuard::new(
            Holder(self.log.clone()),
            Arc::new(AtomicBool::new(false)),
        ))
    }
}

#[derive(Debug)]
struct Connector(FakeReservation);

impl BackendConnector for Connector {
    fn connect(
        &self,
        kind: OutputBackendKind,
        _events: &EventSender,
    ) -> Option<Result<Box<dyn OutputBackend>, OutputError>> {
        (kind == KIND).then(|| {
            let backend: Box<dyn OutputBackend> = Box::new(CardBackend(self.0.clone()));
            Ok(backend)
        })
    }
}

/// One card, reserved for exclusive streams.
#[derive(Debug)]
struct CardBackend(FakeReservation);

impl OutputBackend for CardBackend {
    fn kind(&self) -> OutputBackendKind {
        KIND
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError> {
        Ok(vec![DeviceInfo {
            backend: KIND,
            id: DeviceId::new(CARD).unwrap(),
            description: "DAC".into(),
            is_default: true,
            capabilities: DeviceCapabilities::default(),
        }])
    }

    fn open(&self, config: &StreamConfig) -> Result<OpenedStream, OutputError> {
        let guard = (config.mode == OutputMode::Exclusive)
            .then(|| {
                let card = ReservedCard {
                    index: 0,
                    id: "DAC".into(),
                    device: CARD.into(),
                };
                self.0.acquire(&card)
            })
            .transpose()
            .map_err(|error| OutputError::Backend {
                backend: KIND,
                reason: error.to_string(),
            })?;
        let inner = NullBackend::new().open(&StreamConfig {
            device: DeviceSelection::SystemDefault,
            ..config.clone()
        })?;
        Ok(OpenedStream {
            stream: Box::new(CardStream {
                inner: inner.stream,
                guard,
            }),
            writer: inner.writer,
        })
    }
}

#[derive(Debug)]
struct CardStream {
    inner: Box<dyn OutputStream>,
    guard: Option<ReservationGuard>,
}

impl OutputStream for CardStream {
    fn backend(&self) -> OutputBackendKind {
        KIND
    }

    fn spec(&self) -> AudioSpec {
        self.inner.spec()
    }

    fn state(&self) -> StreamState {
        self.inner.state()
    }

    fn timing(&self) -> StreamTiming {
        self.inner.timing()
    }

    fn start(&mut self) -> Result<(), OutputError> {
        self.inner.start()
    }

    fn pause(&mut self) -> Result<(), OutputError> {
        self.inner.pause()
    }

    fn drain(&mut self, timeout: Duration) -> Result<(), OutputError> {
        self.inner.drain(timeout)
    }

    fn stop(&mut self) -> Result<(), OutputError> {
        let result = self.inner.stop();
        // Closed first, then the card goes back.
        self.guard = None;
        result
    }
}

/// Twenty seconds of a quiet 16-bit sine, written once.
fn music() -> PathBuf {
    static MUSIC: OnceLock<PathBuf> = OnceLock::new();
    MUSIC.get_or_init(write_music).clone()
}

fn write_music() -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("exclusive-release.wav");
    let rate = 48_000u32;
    let frames = rate as usize * 20;
    let mut data = Vec::with_capacity(frames * 4);
    for i in 0..frames {
        let v = ((i as f64 * 0.05).sin() * 1000.0) as i16;
        data.extend_from_slice(&v.to_le_bytes());
        data.extend_from_slice(&v.to_le_bytes());
    }
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    for word in [16u32, 1 | 2 << 16, rate, rate * 4, 4 | 16 << 16] {
        b.extend_from_slice(&word.to_le_bytes());
    }
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    fs::write(&path, b).unwrap();
    path
}

/// An engine playing to the card in `mode`, with `release_s` as the release time.
fn start(mode: OutputMode, release_s: u32) -> (Engine, Receiver<EngineEvent>, FakeReservation) {
    let reservation = FakeReservation::default();
    let mut config = EngineConfig {
        backend_connector: SharedConnector::new(Connector(reservation.clone())),
        ..EngineConfig::default()
    };
    config.output.backend = BackendSelection::Backend(KIND);
    config.output.device = DeviceSelection::Device(DeviceId::new(CARD).unwrap());
    config.output.mode = mode;
    config.output.exclusive_release_s = release_s;
    let (engine, events) = Engine::spawn(config).unwrap();
    engine
        .send(EngineCommand::Load {
            item: PlaybackItem::new(ItemId::new(1), MediaSource::file(music())),
            start: MediaTime::ZERO,
            paused: false,
        })
        .unwrap();
    wait_state(&events, TransportState::Playing);
    (engine, events, reservation)
}

fn wait_state(events: &Receiver<EngineEvent>, state: TransportState) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(EngineEvent::StateChanged(s)) if s == state => return,
            Ok(EngineEvent::Error { error, .. }) => panic!("{error}"),
            Ok(_) => {}
            Err(_) => panic!("no {state:?}"),
        }
    }
}

/// Waits up to `limit` for `done`.
fn eventually(limit: Duration, done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

#[test]
fn stop_gives_the_card_back() {
    let (engine, events, reservation) = start(OutputMode::Exclusive, 30);
    assert!(reservation.held());
    engine.send(EngineCommand::Stop).unwrap();
    wait_state(&events, TransportState::Stopped);
    assert_eq!(reservation.log(), ["acquire", "release"]);
}

#[test]
fn a_pause_gives_the_card_back_after_the_release_time() {
    let (engine, events, reservation) = start(OutputMode::Exclusive, 1);
    let paused = Instant::now();
    engine.send(EngineCommand::Pause).unwrap();
    wait_state(&events, TransportState::Paused);
    assert!(reservation.held(), "released at once");
    assert!(
        eventually(PATIENCE, || !reservation.held()),
        "not released while paused"
    );
    assert!(paused.elapsed() >= Duration::from_millis(900));
    // Playing again takes the card again.
    engine.send(EngineCommand::Play).unwrap();
    wait_state(&events, TransportState::Playing);
    assert_eq!(reservation.log(), ["acquire", "release", "acquire"]);
    drop(engine);
    assert!(!reservation.held());
}

#[test]
fn a_release_time_of_zero_keeps_the_card_while_paused() {
    let (engine, events, reservation) = start(OutputMode::Exclusive, 0);
    engine.send(EngineCommand::Pause).unwrap();
    wait_state(&events, TransportState::Paused);
    assert!(!eventually(Duration::from_millis(1500), || !reservation.held()));
    engine.send(EngineCommand::Play).unwrap();
    wait_state(&events, TransportState::Playing);
    assert_eq!(reservation.log(), ["acquire"]);
    engine.send(EngineCommand::Stop).unwrap();
    wait_state(&events, TransportState::Stopped);
    assert_eq!(reservation.log(), ["acquire", "release"]);
}

#[test]
fn shared_streams_take_no_card() {
    let (engine, events, reservation) = start(OutputMode::Shared, 1);
    engine.send(EngineCommand::Pause).unwrap();
    wait_state(&events, TransportState::Paused);
    std::thread::sleep(Duration::from_millis(300));
    engine.send(EngineCommand::Stop).unwrap();
    wait_state(&events, TransportState::Stopped);
    assert!(reservation.log().is_empty());
}
