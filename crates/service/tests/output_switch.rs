// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Moving playback between PipeWire and ALSA devices while it plays.
//!
//! Ignored by default: it needs a running PipeWire session with pipewire-pulse
//! (`pactl`) and the ALSA `null` PCM. It never makes a sound: it loads two
//! temporary PipeWire null sinks, plays to them by name and to the ALSA `null` PCM,
//! and unloads the sinks again.
//!
//! ```sh
//! cargo test -p helikodu-service --test output_switch -- --ignored
//! ```

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use helikodu_model::audio::{
    BackendSelection, DeviceSelection, MediaTime, OutputBackendKind, OutputMode, OutputSelection,
};
use helikodu_model::player::TransportState;
use helikodu_service::player::{Player, PlayerCommand, PlayerConfig, PlayerEvent};

/// A PipeWire null sink, unloaded when dropped.
struct NullSink {
    module: String,
}

impl NullSink {
    fn load(name: &str) -> Self {
        let output = Command::new("pactl")
            .args(["load-module", "module-null-sink"])
            .arg(format!("sink_name={name}"))
            .output()
            .expect("pactl is needed");
        assert!(output.status.success(), "cannot load a null sink");
        let module = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        Self { module }
    }
}

impl Drop for NullSink {
    fn drop(&mut self) {
        let _ = Command::new("pactl")
            .args(["unload-module", &self.module])
            .status();
    }
}

fn output(backend: OutputBackendKind, device: &str) -> OutputSelection {
    OutputSelection {
        backend: BackendSelection::Backend(backend),
        device: DeviceSelection::Device(device.parse().unwrap()),
        mode: OutputMode::Shared,
    }
}

/// Ten seconds of digital silence, 16-bit stereo at 48 kHz.
fn write_silence(path: &Path) {
    let frames: u32 = 480_000;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + frames * 4).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&[1, 0, 2, 0]);
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&192_000u32.to_le_bytes());
    bytes.extend_from_slice(&[4, 0, 16, 0]);
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(frames * 4).to_le_bytes());
    bytes.resize(bytes.len() + frames as usize * 4, 0);
    fs::write(path, bytes).unwrap();
}

/// Waits until playback is past `after`, failing on any problem.
fn playing_past(events: &Receiver<PlayerEvent>, after: MediaTime) -> MediaTime {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left).expect("timed out") {
            PlayerEvent::Problem { error, .. } => panic!("{error}"),
            PlayerEvent::Position { position, .. } if position > after => return position,
            _ => {}
        }
    }
}

#[test]
#[ignore = "needs PipeWire with pipewire-pulse and the ALSA null PCM"]
fn switches_between_pipewire_and_alsa_while_playing() {
    let _a = NullSink::load("helikodu_switch_a");
    let _b = NullSink::load("helikodu_switch_b");
    // Give the session manager a moment to publish the new nodes.
    std::thread::sleep(Duration::from_millis(500));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("output_switch");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("silence.wav");
    write_silence(&file);

    let first = output(OutputBackendKind::PipeWire, "helikodu_switch_a");
    // Full volume: the session manager remembers the stream volume of the
    // application, and the file is silent anyway.
    let (player, events) = Player::spawn(PlayerConfig::new(first)).unwrap();
    player
        .send(PlayerCommand::PlayFiles {
            files: vec![file],
            start: 0,
        })
        .unwrap();
    let mut at = playing_past(&events, MediaTime::from_millis(500));

    for next in [
        output(OutputBackendKind::Alsa, "null"),
        output(OutputBackendKind::PipeWire, "helikodu_switch_b"),
    ] {
        player.send(PlayerCommand::SetOutput(next.clone())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut reconfigured = false;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match events.recv_timeout(left).expect("timed out") {
                PlayerEvent::Problem { error, .. } => panic!("{next:?}: {error}"),
                PlayerEvent::State(TransportState::Reconfiguring) => reconfigured = true,
                PlayerEvent::State(TransportState::Playing) if reconfigured => break,
                _ => {}
            }
        }
        // The position goes on from where it was, on the new device.
        let moved = playing_past(&events, at);
        assert!(moved >= at);
        at = playing_past(
            &events,
            MediaTime::from_duration(moved.as_duration() + Duration::from_millis(500)),
        );
    }
    player.send(PlayerCommand::Stop).unwrap();
}
