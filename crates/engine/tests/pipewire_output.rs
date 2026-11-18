// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The PipeWire backend against a running PipeWire daemon.
//!
//! These tests are ignored by default: they need a PipeWire session with its
//! PulseAudio service (for `pactl`). They never make a sound: each test loads its
//! own temporary null sink, plays only digital silence into it by name (with
//! fallback to other devices switched off), and unloads it again. Run them with
//!
//! ```sh
//! cargo test -p helikodu-engine --test pipewire_output -- --ignored --test-threads 1
//! ```

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]
#![cfg(feature = "pipewire")]

use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use helikodu_engine::output::{
    OpenedStream, OutputBackend, OutputError, OutputEvent, OutputStream, PipeWireBackend,
    SampleWriter, StreamConfig, StreamState,
};
use helikodu_model::audio::{
    AudioSpec, ChannelLayout, DeviceId, DeviceSelection, Endianness, OutputMode, SampleFormat,
    SampleRate,
};

/// A null sink loaded through `pactl`, unloaded when dropped.
struct NullSink {
    name: String,
    module: String,
}

impl NullSink {
    fn load() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "helikodu_test_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let output = Command::new("pactl")
            .args([
                "load-module",
                "module-null-sink",
                &format!("sink_name={name}"),
                &format!("sink_properties=node.description=Helikodu-test-{name}"),
            ])
            .output()
            .expect("pactl is installed");
        assert!(output.status.success(), "pactl load-module failed");
        let module = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Self { name, module }
    }

    fn id(&self) -> DeviceId {
        DeviceId::new(self.name.as_str()).unwrap()
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
                let zeros = vec![0.0f32; 1024 * writer.channels()];
                while !stop.load(Ordering::Relaxed) && !writer.is_closed() {
                    if writer.write(&zeros) == 0 {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        });
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
        SampleFormat::S16(Endianness::Little),
        SampleRate::new(rate).unwrap(),
        ChannelLayout::STEREO,
    )
}

fn config_for(sink: &NullSink, spec: AudioSpec, mode: OutputMode) -> StreamConfig {
    StreamConfig {
        device: DeviceSelection::Device(sink.id()),
        mode,
        ..StreamConfig::new(spec)
    }
}

/// Waits for an event that `want` accepts, skipping others.
fn wait_for(
    events: &mpsc::Receiver<OutputEvent>,
    timeout: Duration,
    want: impl Fn(&OutputEvent) -> bool,
) -> Option<OutputEvent> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        match events.recv_timeout(left) {
            Ok(event) if want(&event) => return Some(event),
            Ok(_) => {}
            Err(_) => return None,
        }
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

fn start_silent(opened: OpenedStream) -> (Box<dyn OutputStream>, Silence) {
    let OpenedStream { mut stream, writer } = opened;
    let silence = Silence::feed(writer);
    // Let the feeder fill the ring before the device starts pulling.
    std::thread::sleep(Duration::from_millis(50));
    stream.start().unwrap();
    (stream, silence)
}

#[test]
#[ignore = "needs a PipeWire session; loads a temporary null sink"]
fn lists_devices_and_reports_hot_plug() {
    let (events, received) = mpsc::channel();
    let backend = PipeWireBackend::connect(events).unwrap();
    let before = backend.devices().unwrap();
    assert!(
        before
            .iter()
            .all(|d| !d.id.as_str().starts_with("helikodu_test_"))
    );

    let mut sink = NullSink::load();
    let name = sink.name.clone();
    let added = wait_for(
        &received,
        Duration::from_secs(2),
        |e| matches!(e, OutputEvent::DeviceAdded(d) if d.id.as_str() == name),
    );
    let Some(OutputEvent::DeviceAdded(device)) = added else {
        panic!("no DeviceAdded event for {name}");
    };
    assert!(device.description.contains("Helikodu-test"), "{device:?}");
    assert!(backend.devices().unwrap().iter().any(|d| d.id == device.id));
    // The default device is marked, and loading a null sink does not change it.
    let defaults = backend
        .devices()
        .unwrap()
        .iter()
        .filter(|d| d.is_default)
        .count();
    assert!(defaults <= 1);

    sink.unload();
    let removed = wait_for(
        &received,
        Duration::from_secs(2),
        |e| matches!(e, OutputEvent::DeviceRemoved { id, .. } if id.as_str() == name),
    );
    assert!(removed.is_some(), "no DeviceRemoved event for {name}");
    assert!(backend.devices().unwrap().iter().all(|d| d.id != device.id));
}

/// The sink `pactl` reports as the default, if any.
fn pactl_default_sink() -> Option<String> {
    let output = Command::new("pactl")
        .arg("get-default-sink")
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !name.is_empty()).then_some(name)
}

#[test]
#[ignore = "needs a PipeWire session"]
fn the_default_sink_is_marked_as_soon_as_connected() {
    let (events, _received) = mpsc::channel();
    let backend = PipeWireBackend::connect(events).unwrap();
    let devices = backend.devices().unwrap();
    let defaults: Vec<&str> = devices
        .iter()
        .filter(|d| d.is_default)
        .map(|d| d.id.as_str())
        .collect();
    match pactl_default_sink() {
        Some(name) if devices.iter().any(|d| d.id.as_str() == name) => {
            assert_eq!(defaults, [name.as_str()], "{devices:#?}");
        }
        other => {
            eprintln!("SKIPPED: no default sink among the devices ({other:?})");
            assert!(defaults.len() <= 1, "{devices:#?}");
        }
    }
}

#[test]
#[ignore = "needs a PipeWire session; loads a temporary null sink"]
fn plays_silence_into_a_null_sink() {
    let sink = NullSink::load();
    let (events, _received) = mpsc::channel();
    let backend = PipeWireBackend::connect(events).unwrap();
    let config = StreamConfig {
        volume: Some(0.125),
        ..config_for(&sink, stereo(48_000), OutputMode::Shared)
    };
    let opened = backend.open(&config).unwrap();
    // PipeWire takes float and converts for the device.
    assert_eq!(
        opened.stream.spec().format,
        SampleFormat::F32(Endianness::NATIVE)
    );
    assert_eq!(opened.stream.spec().rate, SampleRate::HZ_48000);
    assert_eq!(opened.stream.state(), StreamState::Ready);
    let (mut stream, silence) = start_silent(opened);
    assert_eq!(stream.state(), StreamState::Running);

    assert!(wait_until(Duration::from_secs(2), || stream
        .timing()
        .frames_delivered
        > 4_800));
    let timing = stream.timing();
    assert_eq!(timing.device_rate, Some(SampleRate::HZ_48000), "{timing:?}");
    // A null sink has no hardware latency, so the delay may be zero here.
    assert!(timing.frames_played() <= timing.frames_delivered);
    assert_eq!(timing.underruns, 0, "{timing:?}");

    assert!(stream.has_volume());
    stream.set_volume(0.5).unwrap();

    stream.pause().unwrap();
    assert_eq!(stream.state(), StreamState::Paused);
    std::thread::sleep(Duration::from_millis(100));
    let paused = stream.timing().frames_delivered;
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(stream.timing().frames_delivered, paused);

    stream.start().unwrap();
    assert!(wait_until(Duration::from_secs(2), || stream
        .timing()
        .frames_delivered
        > paused + 4_800));
    stream.stop().unwrap();
    assert_eq!(stream.state(), StreamState::Closed);
    drop(silence);
}

#[test]
#[ignore = "needs a PipeWire session; loads a temporary null sink"]
fn exclusive_mode_forces_the_rate_and_keeps_others_out() {
    let sink = NullSink::load();
    let (events, _received) = mpsc::channel();
    let backend = PipeWireBackend::connect(events).unwrap();
    let opened = backend
        .open(&config_for(&sink, stereo(44_100), OutputMode::Exclusive))
        .unwrap();
    let (mut stream, silence) = start_silent(opened);
    // node.force-rate switches the null sink's graph to the stream's rate.
    assert!(
        wait_until(Duration::from_secs(2), || stream.timing().device_rate
            == SampleRate::new(44_100)),
        "{:?}",
        stream.timing()
    );

    let second = backend.open(&config_for(&sink, stereo(44_100), OutputMode::Shared));
    assert!(
        matches!(second, Err(OutputError::DeviceBusy { .. })),
        "{second:?}"
    );
    stream.stop().unwrap();
    drop(silence);
}

#[test]
#[ignore = "needs a PipeWire session"]
fn a_missing_device_is_not_found_and_never_replaced() {
    let (events, _received) = mpsc::channel();
    let backend = PipeWireBackend::connect(events).unwrap();
    let config = StreamConfig {
        device: DeviceSelection::Device(DeviceId::new("helikodu_test_no_such_sink").unwrap()),
        ..StreamConfig::new(stereo(48_000))
    };
    let started = Instant::now();
    let result = backend.open(&config);
    assert!(
        matches!(result, Err(OutputError::DeviceNotFound { .. })),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
#[ignore = "needs a PipeWire session; loads a temporary null sink"]
fn a_removed_device_is_lost_not_moved() {
    let mut sink = NullSink::load();
    let (events, received) = mpsc::channel();
    let backend = PipeWireBackend::connect(events).unwrap();
    let opened = backend
        .open(&config_for(&sink, stereo(48_000), OutputMode::Shared))
        .unwrap();
    let (mut stream, silence) = start_silent(opened);
    assert!(wait_until(Duration::from_secs(2), || stream
        .timing()
        .frames_delivered
        > 0));

    sink.unload();
    let lost = wait_for(&received, Duration::from_secs(2), |e| {
        matches!(e, OutputEvent::DeviceLost { .. })
    });
    let Some(OutputEvent::DeviceLost { device, .. }) = lost else {
        panic!("no DeviceLost event");
    };
    assert_eq!(device, DeviceSelection::Device(sink.id()));
    assert_eq!(stream.state(), StreamState::Lost);
    assert!(matches!(stream.start(), Err(OutputError::Closed)));
    stream.stop().unwrap();
    drop(silence);
}

#[test]
#[ignore = "needs a PipeWire session; loads a temporary null sink"]
fn device_capabilities_come_from_the_node_formats() {
    let sink = NullSink::load();
    let (events, _received) = mpsc::channel();
    let backend = PipeWireBackend::connect(events).unwrap();
    let id = sink.id();
    let capabilities = || {
        backend
            .devices()
            .unwrap()
            .into_iter()
            .find(|d| d.id == id)
            .map(|d| d.capabilities)
            .unwrap_or_default()
    };
    assert!(
        wait_until(Duration::from_secs(3), || {
            let caps = capabilities();
            !caps.formats.is_empty() && !caps.sample_rates.is_empty()
        }),
        "no capabilities for the null sink: {:?}",
        capabilities()
    );
    let caps = capabilities();
    println!(
        "{}: formats {:?}, rates {:?}, channels {:?}",
        sink.name,
        caps.formats
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        caps.sample_rates.iter().map(|r| r.hz()).collect::<Vec<_>>(),
        caps.channel_counts
    );
    assert!(caps.sample_rates.windows(2).all(|w| w[0] < w[1]));
    assert!(!caps.channel_counts.is_empty());
}
