// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Devices that go away, are unplugged, or have a profile of their own: the engine
//! plays in real time through a scripted backend, which stands for each sound
//! backend in turn and discards the audio.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use helikodu_engine::output::{
    BackendConnector, EventSender, NullBackend, OpenedStream, OutputBackend, OutputError,
    OutputEvent, OutputStream, SharedConnector, StreamConfig, StreamState, StreamTiming,
};
use helikodu_engine::transport::EngineConfig;
use helikodu_model::audio::{
    AudioSpec, BackendSelection, DeviceCapabilities, DeviceId, DeviceInfo, DeviceKey,
    DeviceSelection, Endianness, OutputBackendKind, OutputMode, OutputSelection, SampleFormat,
    SampleRate,
};
use helikodu_model::player::{EngineCommand, EngineError, EngineEvent, TransportState};
use helikodu_model::settings::{DeviceLostPolicy, DeviceProfile, OutputFormat, SampleRatePolicy};

use crate::media;
use crate::session::{Session, item};

const HEADPHONES: &str = "headphones";
const SPEAKERS: &str = "speakers";

/// What the scripted backend has done, and its switches.
#[derive(Debug, Default)]
struct Script {
    events: Mutex<Option<EventSender>>,
    /// Whether the open stream's device is gone.
    lost: AtomicBool,
    opened: Mutex<Vec<StreamConfig>>,
    stopped: AtomicUsize,
}

impl Script {
    fn send(&self, event: OutputEvent) {
        let events = self.events.lock().unwrap();
        events.as_ref().unwrap().send(event).unwrap();
    }

    /// The device of the open stream goes away.
    fn lose(&self, kind: OutputBackendKind) {
        self.lost.store(true, Ordering::Release);
        let device = self.opened.lock().unwrap().last().unwrap().device.clone();
        self.send(OutputEvent::DeviceLost {
            backend: kind,
            device,
        });
    }

    fn unplug(&self, kind: OutputBackendKind, name: &str) {
        self.send(OutputEvent::DeviceRemoved {
            backend: kind,
            id: DeviceId::new(name).unwrap(),
        });
    }

    fn opened(&self) -> Vec<StreamConfig> {
        self.opened.lock().unwrap().clone()
    }
}

#[derive(Debug)]
struct Connector {
    kind: OutputBackendKind,
    script: Arc<Script>,
}

impl BackendConnector for Connector {
    fn connect(
        &self,
        kind: OutputBackendKind,
        events: &EventSender,
    ) -> Option<Result<Box<dyn OutputBackend>, OutputError>> {
        if kind != self.kind {
            return None;
        }
        *self.script.events.lock().unwrap() = Some(events.clone());
        let backend: Box<dyn OutputBackend> = Box::new(ScriptedBackend {
            kind,
            script: self.script.clone(),
        });
        Some(Ok(backend))
    }
}

/// Two devices, headphones (the default) and speakers; streams discard the audio in
/// real time.
#[derive(Debug)]
struct ScriptedBackend {
    kind: OutputBackendKind,
    script: Arc<Script>,
}

impl OutputBackend for ScriptedBackend {
    fn kind(&self) -> OutputBackendKind {
        self.kind
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError> {
        Ok([HEADPHONES, SPEAKERS]
            .into_iter()
            .map(|name| DeviceInfo {
                backend: self.kind,
                id: DeviceId::new(name).unwrap(),
                description: name.into(),
                is_default: name == HEADPHONES,
                capabilities: DeviceCapabilities::default(),
            })
            .collect())
    }

    fn open(&self, config: &StreamConfig) -> Result<OpenedStream, OutputError> {
        if let DeviceSelection::Device(id) = &config.device
            && ![HEADPHONES, SPEAKERS].contains(&id.as_str())
        {
            return Err(OutputError::DeviceNotFound {
                backend: self.kind,
                device: config.device.clone(),
            });
        }
        self.script.opened.lock().unwrap().push(config.clone());
        self.script.lost.store(false, Ordering::Release);
        let inner = NullBackend::new().open(&StreamConfig {
            device: DeviceSelection::SystemDefault,
            ..config.clone()
        })?;
        Ok(OpenedStream {
            stream: Box::new(ScriptedStream {
                kind: self.kind,
                inner: inner.stream,
                script: self.script.clone(),
            }),
            writer: inner.writer,
        })
    }
}

#[derive(Debug)]
struct ScriptedStream {
    kind: OutputBackendKind,
    inner: Box<dyn OutputStream>,
    script: Arc<Script>,
}

impl OutputStream for ScriptedStream {
    fn backend(&self) -> OutputBackendKind {
        self.kind
    }

    fn spec(&self) -> AudioSpec {
        self.inner.spec()
    }

    fn state(&self) -> StreamState {
        if self.script.lost.load(Ordering::Acquire) {
            StreamState::Lost
        } else {
            self.inner.state()
        }
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
        self.script.stopped.fetch_add(1, Ordering::AcqRel);
        self.inner.stop()
    }
}

/// Ten seconds of a sine, written once for every test here.
fn music() -> PathBuf {
    static MUSIC: OnceLock<PathBuf> = OnceLock::new();
    MUSIC
        .get_or_init(|| media::write_wav("devices-music.wav", &media::sine(48_000, 480_000)))
        .clone()
}

/// An engine on the scripted backend standing for `kind`, playing to `device`.
fn start(
    kind: OutputBackendKind,
    device: DeviceSelection,
    setup: impl FnOnce(&mut EngineConfig),
) -> (Session, Arc<Script>) {
    let script = Arc::new(Script::default());
    let mut config = EngineConfig {
        backend_connector: SharedConnector::new(Connector {
            kind,
            script: script.clone(),
        }),
        ..EngineConfig::default()
    };
    config.output.backend = BackendSelection::Backend(kind);
    config.output.device = device;
    setup(&mut config);
    let mut session = Session::new(config);
    session.load(&item(1, &music()), helikodu_model::audio::MediaTime::ZERO);
    session.wait_state(TransportState::Playing);
    session.wait_for(|e| matches!(e, EngineEvent::Position { .. }));
    (session, script)
}

fn device(name: &str) -> DeviceSelection {
    DeviceSelection::Device(DeviceId::new(name).unwrap())
}

/// Collects events for `time` and returns whether the state changed.
fn state_changes(session: &mut Session, time: Duration) -> Vec<TransportState> {
    let until = Instant::now() + time;
    let mut changes = Vec::new();
    while Instant::now() < until {
        // Positions keep coming while the engine plays.
        if let EngineEvent::StateChanged(state) = session.wait_for(|e| {
            matches!(
                e,
                EngineEvent::Position { .. } | EngineEvent::StateChanged(_)
            )
        }) {
            changes.push(state);
            if state != TransportState::Playing {
                break;
            }
        }
    }
    changes
}

#[test]
fn unplugging_the_default_device_pauses() {
    for kind in [
        OutputBackendKind::PipeWire,
        OutputBackendKind::PulseAudio,
        OutputBackendKind::Jack,
    ] {
        let (mut session, script) = start(kind, DeviceSelection::SystemDefault, |_| {});
        script.unplug(kind, HEADPHONES);
        session.wait_state(TransportState::Paused);
        assert!(
            !session
                .log
                .iter()
                .any(|e| matches!(e, EngineEvent::Error { .. })),
            "{kind}: {:#?}",
            session.log
        );
        // Resuming plays on (the server moved the stream to the next default).
        session.send(EngineCommand::Play);
        session.wait_state(TransportState::Playing);
    }
}

#[test]
fn unplugging_goes_on_when_the_setting_is_off_or_another_device_goes() {
    let kind = OutputBackendKind::PulseAudio;
    let (mut session, script) = start(kind, DeviceSelection::SystemDefault, |config| {
        config.output.pause_on_unplug = false;
    });
    script.unplug(kind, HEADPHONES);
    assert_eq!(state_changes(&mut session, Duration::from_millis(500)), []);

    let (mut session, script) = start(kind, DeviceSelection::SystemDefault, |_| {});
    script.unplug(kind, SPEAKERS);
    assert_eq!(state_changes(&mut session, Duration::from_millis(500)), []);

    // A chosen device that is unplugged is not followed: its stream reports itself
    // lost, and the device-lost policy applies (below).
    let (mut session, script) = start(kind, device(HEADPHONES), |_| {});
    script.unplug(kind, HEADPHONES);
    assert_eq!(state_changes(&mut session, Duration::from_millis(300)), []);
}

#[test]
fn a_lost_device_follows_the_policy_on_every_backend() {
    for kind in [
        OutputBackendKind::PipeWire,
        OutputBackendKind::Alsa,
        OutputBackendKind::PulseAudio,
        OutputBackendKind::Jack,
    ] {
        for policy in [
            DeviceLostPolicy::Pause,
            DeviceLostPolicy::Stop,
            DeviceLostPolicy::FallbackDefault,
        ] {
            let (mut session, script) = start(kind, device(SPEAKERS), |config| {
                config.output.on_device_lost = policy;
            });
            script.lose(kind);
            let lost = session.wait_for(|e| matches!(e, EngineEvent::Error { .. }));
            assert!(
                matches!(
                    &lost,
                    EngineEvent::Error {
                        error: EngineError::DeviceLost { backend, device: d },
                        ..
                    } if *backend == kind && *d == device(SPEAKERS)
                ),
                "{kind} {policy}: {lost:?}"
            );
            match policy {
                DeviceLostPolicy::Pause => session.wait_state(TransportState::Paused),
                DeviceLostPolicy::Stop => session.wait_state(TransportState::Stopped),
                DeviceLostPolicy::FallbackDefault => {
                    session.wait_state(TransportState::Playing);
                    let opened = script.opened();
                    assert_eq!(opened.len(), 2, "{kind}");
                    assert_eq!(opened[1].device, DeviceSelection::SystemDefault);
                }
            }
        }
    }
}

#[test]
fn a_profile_applies_while_its_device_is_open() {
    let kind = OutputBackendKind::PulseAudio;
    let native = Endianness::NATIVE;
    let speakers = DeviceKey::new(kind, DeviceId::new(SPEAKERS).unwrap());
    let (mut session, script) = start(kind, device(SPEAKERS), |config| {
        config.output.profiles.insert(
            speakers.clone(),
            DeviceProfile {
                mode: Some(OutputMode::Exclusive),
                sample_rate: Some(SampleRatePolicy::Fixed(SampleRate::new(44_100).unwrap())),
                format: Some(OutputFormat::S32),
                buffer_ms: Some(500),
                exclusive_release_s: Some(1),
                ..DeviceProfile::default()
            },
        );
    });
    let opened = script.opened();
    let profile = &opened[0];
    assert_eq!(profile.mode, OutputMode::Exclusive);
    assert_eq!(profile.spec.rate.hz(), 44_100);
    assert!(profile.request_rate);
    assert_eq!(profile.spec.format, SampleFormat::S32(native));
    assert_eq!(profile.buffer, Duration::from_millis(500));

    // The profile's release time (1 s), not the general one (30 s), closes the
    // paused exclusive stream.
    session.send(EngineCommand::Pause);
    session.wait_state(TransportState::Paused);
    let deadline = Instant::now() + Duration::from_secs(5);
    while script.stopped.load(Ordering::Acquire) == 0 {
        assert!(Instant::now() < deadline, "the device was not released");
        std::thread::sleep(Duration::from_millis(20));
    }

    // Another device has no profile: the general settings apply.
    session.send(EngineCommand::SetOutput(OutputSelection {
        backend: BackendSelection::Backend(kind),
        device: device(HEADPHONES),
        mode: OutputMode::Shared,
    }));
    session.send(EngineCommand::Play);
    session.wait_state(TransportState::Playing);
    let opened = script.opened();
    let general = opened.last().unwrap();
    assert_eq!(general.device, device(HEADPHONES));
    assert_eq!(general.mode, OutputMode::Shared);
    assert_eq!(general.spec.rate.hz(), 48_000);
    assert_eq!(general.spec.format, SampleFormat::S16(native));
    assert_eq!(general.buffer, Duration::from_millis(200));

    // A changed profile reopens the stream of its device.
    let mut output = helikodu_model::settings::OutputSettings::default();
    output.profiles.insert(
        DeviceKey::new(kind, DeviceId::new(HEADPHONES).unwrap()),
        DeviceProfile {
            buffer_ms: Some(300),
            ..DeviceProfile::default()
        },
    );
    session.send(EngineCommand::UpdateSettings {
        output,
        loudness: Default::default(),
        dsp: Default::default(),
    });
    session.wait_state(TransportState::Reconfiguring);
    session.wait_state(TransportState::Playing);
    let opened = script.opened();
    assert_eq!(opened.last().unwrap().buffer, Duration::from_millis(300));
}
