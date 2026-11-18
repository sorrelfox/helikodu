// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The PulseAudio and JACK backends against a running PipeWire session, through its
//! PulseAudio and JACK services.
//!
//! These tests are ignored by default: they need PipeWire with pipewire-pulse
//! (`pactl`) and pipewire-jack. They never make a sound: each test loads its own
//! temporary null sink with `pactl`, plays only digital silence into it, and unloads
//! it again. Run them with
//!
//! ```sh
//! PIPEWIRE_PROPS='{ jack.self-connect-mode = ignore-all }' \
//!     cargo test -p helikodu-engine --test cpal_output -- --ignored --test-threads 1
//! ```
//!
//! `PIPEWIRE_PROPS` keeps cpal from linking the JACK stream to the default device
//! while it is built; without it the backend removes that link before the stream
//! starts, and the default device only ever receives silence.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]
#![cfg(any(feature = "pulseaudio", feature = "jack"))]

use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use helikodu_engine::output::{
    OpenedStream, OutputBackend, OutputEvent, OutputStream, SampleWriter, StreamConfig, StreamState,
};
use helikodu_model::audio::{
    AudioSpec, ChannelLayout, DeviceId, DeviceSelection, Endianness, SampleFormat, SampleRate,
};

/// A null sink loaded through `pactl`, unloaded when dropped.
struct NullSink {
    /// The sink name, the PulseAudio backend's device name.
    #[cfg_attr(not(feature = "pulseaudio"), allow(dead_code))]
    name: String,
    /// The description, which names the JACK client of the sink.
    description: String,
    module: String,
}

impl NullSink {
    fn load() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("helikodu_test_{}_{n}", std::process::id());
        let description = format!("Helikodu-test-{}-{n}", std::process::id());
        let output = Command::new("pactl")
            .args([
                "load-module",
                "module-null-sink",
                &format!("sink_name={name}"),
                &format!("sink_properties=node.description={description}"),
            ])
            .output()
            .expect("pactl is installed");
        assert!(output.status.success(), "pactl load-module failed");
        let module = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Self {
            name,
            description,
            module,
        }
    }

    fn unload(&mut self) {
        if !self.module.is_empty() {
            let _ = Command::new("pactl")
                .args(["unload-module", &self.module])
                .status();
            self.module.clear();
        }
    }
}

impl Drop for NullSink {
    fn drop(&mut self) {
        self.unload();
    }
}

/// Keeps a ring buffer topped up with silence until stopped.
struct Silence {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Silence {
    fn feed(mut writer: SampleWriter) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let stop = stop.clone();
            move || {
                // Digital silence only.
                let zeros = vec![0.0f32; 512 * writer.channels()];
                while !stop.load(Ordering::Relaxed) && !writer.is_closed() {
                    if writer.write(&zeros) == 0 {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            }
        });
        // Let the ring fill before the stream starts.
        std::thread::sleep(Duration::from_millis(50));
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Silence {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn stereo(rate: u32) -> AudioSpec {
    AudioSpec::new(
        SampleFormat::S16(Endianness::NATIVE),
        SampleRate::new(rate).unwrap(),
        ChannelLayout::STEREO,
    )
}

fn config_for(device: &str, spec: AudioSpec) -> StreamConfig {
    StreamConfig {
        device: DeviceSelection::Device(DeviceId::new(device).unwrap()),
        ..StreamConfig::new(spec)
    }
}

fn wait_until(timeout: Duration, condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    condition()
}

/// Waits for an event that `want` accepts, skipping others.
fn wait_for(
    events: &mpsc::Receiver<OutputEvent>,
    timeout: Duration,
    want: impl Fn(&OutputEvent) -> bool,
) -> Option<OutputEvent> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(event) if want(&event) => return Some(event),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

/// Opens `config` on `backend`, starts it on a ring of silence and checks that it
/// plays: frames are delivered, a delay is reported and nothing runs short.
fn play_silence(
    backend: &dyn OutputBackend,
    config: &StreamConfig,
) -> (Box<dyn OutputStream>, Silence) {
    let OpenedStream { mut stream, writer } = backend.open(config).unwrap();
    let silence = Silence::feed(writer);
    stream.start().unwrap();
    assert_eq!(stream.state(), StreamState::Running);
    let rate = u64::from(stream.spec().rate.hz());
    assert!(
        wait_until(Duration::from_secs(5), || stream.timing().frames_delivered
            > rate / 2),
        "{:?}",
        stream.timing()
    );
    let timing = stream.timing();
    assert!(timing.delay_frames > 0, "{timing:?}");
    assert!(
        timing.heard_position() <= timing.frames_delivered,
        "{timing:?}"
    );
    assert_eq!(timing.underruns, 0, "{timing:?}");
    (stream, silence)
}

#[cfg(feature = "pulseaudio")]
mod pulseaudio {
    use helikodu_engine::output::PulseBackend;
    use helikodu_model::audio::OutputBackendKind;

    use super::*;

    /// The sink index `pactl` shows for `name`, and the sink each of its sink inputs
    /// plays to.
    fn sink_index(name: &str) -> Option<String> {
        let output = Command::new("pactl")
            .args(["list", "short", "sinks"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find(|line| line.split('\t').nth(1) == Some(name))
            .and_then(|line| line.split('\t').next().map(str::to_owned))
    }

    fn sinks_of_inputs() -> Vec<String> {
        let output = Command::new("pactl")
            .args(["list", "short", "sink-inputs"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split('\t').nth(1).map(str::to_owned))
            .collect()
    }

    #[test]
    #[ignore = "needs a PipeWire or PulseAudio session; loads a temporary null sink"]
    fn lists_sinks_and_plays_silence_into_a_null_sink() {
        let sink = NullSink::load();
        let (events, _received) = mpsc::channel();
        let backend = PulseBackend::connect(events).unwrap();
        let devices = backend.devices().unwrap();
        let device = devices
            .iter()
            .find(|d| d.id.as_str() == sink.name)
            .expect("the null sink is listed by its sink name");
        assert_eq!(device.backend, OutputBackendKind::PulseAudio);
        assert_eq!(device.description, sink.description);
        assert!(!device.capabilities.formats.is_empty());
        assert_eq!(devices.iter().filter(|d| d.is_default).count(), 1);

        for rate in [44_100, 48_000] {
            let config = config_for(&sink.name, stereo(rate));
            let (mut stream, _silence) = play_silence(&backend, &config);
            assert_eq!(stream.spec(), config.spec);
            // The stream plays to the null sink and nowhere else.
            let index = sink_index(&sink.name).unwrap();
            let inputs = sinks_of_inputs();
            assert!(inputs.contains(&index), "{inputs:?} {index}");
            stream.pause().unwrap();
            assert_eq!(stream.state(), StreamState::Paused);
            stream.start().unwrap();
            stream.stop().unwrap();
            assert_eq!(stream.state(), StreamState::Closed);
        }
    }

    #[test]
    #[ignore = "needs a PipeWire or PulseAudio session; loads a temporary null sink"]
    fn reports_hot_plug_and_a_lost_sink() {
        let (events, received) = mpsc::channel();
        let backend = PulseBackend::connect(events).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let mut sink = NullSink::load();
        let added = wait_for(
            &received,
            Duration::from_secs(5),
            |e| matches!(e, OutputEvent::DeviceAdded(info) if info.id.as_str() == sink.name),
        );
        assert!(added.is_some(), "no DeviceAdded");

        let config = config_for(&sink.name, stereo(48_000));
        let (stream, _silence) = play_silence(&backend, &config);
        sink.unload();
        let lost = wait_for(&received, Duration::from_secs(5), |e| {
            matches!(e, OutputEvent::DeviceLost { .. })
        });
        assert_eq!(
            lost,
            Some(OutputEvent::DeviceLost {
                backend: OutputBackendKind::PulseAudio,
                device: config.device.clone(),
            })
        );
        assert_eq!(stream.state(), StreamState::Lost);
        drop(stream);
    }

    #[test]
    #[ignore = "needs a PipeWire or PulseAudio session"]
    fn a_missing_sink_is_not_found() {
        let (events, _received) = mpsc::channel();
        let backend = PulseBackend::connect(events).unwrap();
        let config = config_for("helikodu_no_such_sink", stereo(48_000));
        assert!(matches!(
            backend.open(&config),
            Err(helikodu_engine::output::OutputError::DeviceNotFound { .. })
        ));
    }
}

#[cfg(feature = "jack")]
mod jack {
    use helikodu_engine::output::JackBackend;
    use helikodu_model::audio::OutputBackendKind;

    use super::*;

    /// Every connection of this process's cpal output ports.
    fn our_connections() -> Vec<(String, Vec<String>)> {
        let (client, _) = ::jack::Client::new(
            "helikodu-test-probe",
            ::jack::ClientOptions::NO_START_SERVER,
        )
        .unwrap();
        let ours = client.ports(
            Some(&format!("^cpal_client_{}_", std::process::id())),
            None,
            ::jack::PortFlags::IS_OUTPUT,
        );
        ours.into_iter()
            .map(|port| {
                let connections = client
                    .port_by_name(&port)
                    .map(|p| p.get_connections())
                    .unwrap_or_default();
                (port, connections)
            })
            .collect()
    }

    #[test]
    #[ignore = "needs PipeWire's JACK service; loads a temporary null sink"]
    fn connects_only_to_the_chosen_client_and_plays_silence() {
        let sink = NullSink::load();
        let (events, _received) = mpsc::channel();
        let backend = JackBackend::connect(events).unwrap();
        // PipeWire names a sink's JACK client after its description.
        assert!(wait_until(Duration::from_secs(3), || backend
            .devices()
            .unwrap()
            .iter()
            .any(|d| d.id.as_str() == sink.description)));
        let devices = backend.devices().unwrap();
        let device = devices
            .iter()
            .find(|d| d.id.as_str() == sink.description)
            .unwrap();
        assert_eq!(device.backend, OutputBackendKind::Jack);
        assert_eq!(device.capabilities.channel_counts, [2]);
        let server_rate = device.capabilities.sample_rates[0];

        // JACK runs at the server's rate: a 44.1 kHz request opens at that rate.
        let config = config_for(&sink.description, stereo(44_100));
        let opened = backend.open(&config).unwrap();
        assert_eq!(opened.stream.spec().rate, server_rate);
        assert_eq!(
            opened.stream.spec().format,
            SampleFormat::F32(Endianness::NATIVE)
        );
        // Before it starts, its ports lead to the null sink and nowhere else.
        let connections = our_connections();
        assert_eq!(connections.len(), 2, "{connections:?}");
        for (port, to) in &connections {
            assert_eq!(to.len(), 1, "{port}: {to:?}");
            assert!(
                to[0].starts_with(&format!("{}:", sink.description)),
                "{port}: {to:?}"
            );
        }
        drop(opened);

        let config = config_for(&sink.description, stereo(server_rate.hz()));
        let (mut stream, _silence) = play_silence(&backend, &config);
        stream.pause().unwrap();
        stream.start().unwrap();
        stream.stop().unwrap();
        assert!(wait_until(Duration::from_secs(2), || our_connections().is_empty()));
    }

    #[test]
    #[ignore = "needs PipeWire's JACK service; loads a temporary null sink"]
    fn a_removed_client_loses_the_stream() {
        let mut sink = NullSink::load();
        let (events, received) = mpsc::channel();
        let backend = JackBackend::connect(events).unwrap();
        assert!(wait_until(Duration::from_secs(3), || backend
            .devices()
            .unwrap()
            .iter()
            .any(|d| d.id.as_str() == sink.description)));
        let rate = backend.devices().unwrap()[0].capabilities.sample_rates[0];
        let config = config_for(&sink.description, stereo(rate.hz()));
        let (stream, _silence) = play_silence(&backend, &config);
        sink.unload();
        let lost = wait_for(&received, Duration::from_secs(5), |e| {
            matches!(e, OutputEvent::DeviceLost { .. })
        });
        assert_eq!(
            lost,
            Some(OutputEvent::DeviceLost {
                backend: OutputBackendKind::Jack,
                device: config.device.clone(),
            })
        );
        assert_eq!(stream.state(), StreamState::Lost);
        // JACK removed the links with the client: the stream is connected to nothing.
        assert!(our_connections().iter().all(|(_, to)| to.is_empty()));
    }
}
