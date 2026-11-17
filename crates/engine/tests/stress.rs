// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The underrun stress test: the engine plays an endless queue in real time while
//! every processor is kept busy, and the output must never run dry.
//!
//! Ignored by default; run it with
//!
//! ```sh
//! cargo test -p helikodu-engine --release --test stress -- --ignored --nocapture
//! ```
//!
//! | Variable | Meaning | Default |
//! |---|---|---|
//! | `HELIKODU_STRESS_SECS` | How long to play | 60 |
//! | `HELIKODU_STRESS_BACKEND` | `null` (the null sink, paced like a device) or `pipewire` (a temporary PipeWire null sink, loaded with `pactl` and unloaded afterwards) | `null` |
//! | `HELIKODU_STRESS_LOAD` | Busy threads in the background | the number of processors |
//!
//! It never makes a sound: the material is near-silent (noise of ±1 LSB at
//! 16 bits, about -90 dBFS), and with `pipewire` it plays only into its own null
//! sink by name, with fallback to other devices switched off.
//!
//! The queue alternates a WAV and a FLAC file (when the `ffmpeg` tool is there) of
//! 20 seconds each, so the run also covers gapless joins, opening the next item
//! and the `NearEnd` exchange. The test fails on any underrun, any error event,
//! any reopening of the device, and when less audio played than the time passed.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml).
#![allow(clippy::unwrap_used)]

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helikodu_engine::output::Pacing;
use helikodu_engine::transport::{Engine, EngineConfig};
use helikodu_model::audio::{
    BackendSelection, DeviceId, DeviceSelection, MediaTime, OutputBackendKind,
};
use helikodu_model::library::MediaSource;
use helikodu_model::player::{
    EngineCommand, EngineEvent, ItemId, PlaybackItem, TrackEndReason, TransportState,
};
use helikodu_model::settings::DeviceLostPolicy;

const RATE: u32 = 48_000;
const ITEM_SECONDS: u32 = 20;

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// Writes `ITEM_SECONDS` of 16-bit stereo noise of ±1 LSB.
fn write_quiet_wav(path: &Path) {
    let frames = (RATE * ITEM_SECONDS) as usize;
    let data_len = frames * 4;
    let mut b = Vec::with_capacity(44 + data_len);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data_len as u32).to_le_bytes());
    let mut lcg: u32 = 0x2468_ace1;
    for _ in 0..frames * 2 {
        lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let value: i16 = match lcg >> 30 {
            0 => -1,
            1 => 1,
            _ => 0,
        };
        b.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

/// The files of the queue: the WAV, and a FLAC copy if `ffmpeg` can make one.
fn material() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine-stress");
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("quiet.wav");
    write_quiet_wav(&wav);
    let flac = dir.join("quiet.flac");
    let made = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&wav)
        .args(["-c:a", "flac"])
        .arg(&flac)
        .status()
        .is_ok_and(|s| s.success());
    if made {
        vec![wav, flac]
    } else {
        eprintln!("no ffmpeg tool: the queue repeats the WAV file only");
        vec![wav]
    }
}

/// A PipeWire null sink loaded through `pactl`, unloaded when dropped.
struct NullSink {
    name: String,
    module: String,
}

impl NullSink {
    fn load() -> Self {
        let name = format!("helikodu_stress_{}", std::process::id());
        let output = Command::new("pactl")
            .args([
                "load-module",
                "module-null-sink",
                &format!("sink_name={name}"),
                &format!("sink_properties=node.description=Helikodu-stress-{name}"),
            ])
            .output()
            .expect("pactl is installed");
        assert!(output.status.success(), "pactl load-module failed");
        let module = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Self { name, module }
    }
}

impl Drop for NullSink {
    fn drop(&mut self) {
        let _ = Command::new("pactl")
            .args(["unload-module", &self.module])
            .status();
    }
}

/// Threads that keep the processors busy until dropped.
struct Load {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Load {
    fn start(count: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let threads = (0..count)
            .map(|i| {
                let stop = stop.clone();
                thread::spawn(move || {
                    // Arithmetic and memory traffic, so caches are shared too.
                    let mut buffer = vec![0u64; 1 << 18];
                    let mut x = i as u64 + 1;
                    while !stop.load(Ordering::Relaxed) {
                        for slot in buffer.iter_mut().step_by(7) {
                            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                            *slot ^= x;
                        }
                        black_box(&buffer);
                    }
                })
            })
            .collect();
        Self { stop, threads }
    }
}

impl Drop for Load {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[test]
#[ignore = "runs for minutes in real time; see the module documentation"]
fn plays_without_underruns_under_load() {
    let seconds = env_number("HELIKODU_STRESS_SECS", 60);
    let backend = std::env::var("HELIKODU_STRESS_BACKEND").unwrap_or_else(|_| "null".into());
    let cpus = thread::available_parallelism().map_or(4, usize::from);
    let load_threads = usize::try_from(env_number("HELIKODU_STRESS_LOAD", cpus as u64)).unwrap();
    helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);

    let files = material();
    let mut config = EngineConfig {
        sink_pacing: Pacing::RealTime,
        ..EngineConfig::default()
    };
    // Never fall back to another (audible) device.
    config.output.on_device_lost = DeviceLostPolicy::Stop;
    let _sink = match backend.as_str() {
        "null" => {
            config.output.backend = BackendSelection::Backend(OutputBackendKind::Null);
            None
        }
        "pipewire" => {
            let sink = NullSink::load();
            config.output.backend = BackendSelection::Backend(OutputBackendKind::PipeWire);
            config.output.device =
                DeviceSelection::Device(DeviceId::new(sink.name.as_str()).unwrap());
            Some(sink)
        }
        other => panic!("HELIKODU_STRESS_BACKEND must be null or pipewire, not {other}"),
    };
    let buffer_ms = config.output.buffer_ms;

    let (engine, events) = Engine::spawn(config).unwrap();
    let stats = engine.stats();
    let mut next_id = 1;
    let mut next_item = || {
        let path = &files[(next_id - 1) as usize % files.len()];
        let item = PlaybackItem::new(ItemId::new(next_id), MediaSource::file(path));
        next_id += 1;
        item
    };
    let _load = Load::start(load_threads);
    println!(
        "stress: {seconds} s on {backend}, {load_threads} busy threads, buffer {buffer_ms} ms, \
         {} files in the queue",
        files.len()
    );
    engine
        .send(EngineCommand::Load {
            item: next_item(),
            start: MediaTime::ZERO,
            paused: false,
        })
        .unwrap();

    let start = Instant::now();
    let deadline = start + Duration::from_secs(seconds);
    let mut errors = Vec::new();
    let mut reopened = 0;
    let mut finished = 0;
    let mut last_report = start;
    let mut playing_since = None;
    while Instant::now() < deadline {
        match events.recv_timeout(Duration::from_millis(200)) {
            Ok(EngineEvent::NearEnd { .. }) => {
                engine
                    .send(EngineCommand::SetNext(Some(next_item())))
                    .unwrap();
            }
            Ok(EngineEvent::StateChanged(TransportState::Playing)) => {
                playing_since.get_or_insert_with(Instant::now);
            }
            Ok(EngineEvent::StateChanged(TransportState::Reconfiguring)) => reopened += 1,
            Ok(EngineEvent::StateChanged(TransportState::Stopped)) => {
                errors.push("playback stopped".to_owned());
                break;
            }
            Ok(EngineEvent::TrackEnded { reason, .. }) => {
                if reason == TrackEndReason::Finished {
                    finished += 1;
                } else {
                    errors.push(format!("an item ended with {reason:?}"));
                }
            }
            Ok(EngineEvent::Error { error, .. }) => errors.push(error.to_string()),
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                errors.push("the engine thread ended".into());
                break;
            }
        }
        if last_report.elapsed() >= Duration::from_secs(60) {
            last_report = Instant::now();
            let s = stats.snapshot();
            println!(
                "stress: {:>5} s, {} frames, {} underruns, {} items finished",
                start.elapsed().as_secs(),
                s.frames_delivered,
                s.underruns,
                finished
            );
        }
    }
    let played_for = playing_since.map_or(Duration::ZERO, |since| since.elapsed());
    let during = stats.snapshot();
    engine.send(EngineCommand::Stop).unwrap();
    drop(engine);
    let after = stats.snapshot();
    let expected = played_for.as_secs_f64() * f64::from(RATE);
    println!(
        "stress: played {:.1} s: {} frames delivered ({:.4} of real time), {} underruns, \
         {} silent frames, {} streams opened, {} items finished, {} errors",
        played_for.as_secs_f64(),
        during.frames_delivered,
        during.frames_delivered as f64 / expected.max(1.0),
        after.underruns,
        after.silent_frames,
        after.streams_opened,
        finished,
        errors.len()
    );
    assert!(errors.is_empty(), "errors: {errors:#?}");
    assert_eq!(reopened, 0, "the device was reopened between items");
    assert_eq!(after.underruns, 0, "underruns: {after:?}");
    // The device took at least the audio of the time that passed, less one buffer
    // and the device's own delay.
    let slack = f64::from(RATE) * (f64::from(buffer_ms) / 1000.0 + 0.5);
    assert!(
        during.frames_delivered as f64 >= expected - slack,
        "only {} of {expected:.0} frames were delivered",
        during.frames_delivered
    );
}
