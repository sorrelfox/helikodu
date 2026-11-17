// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Generated test files, reading the WAV sink's output, and waiting for events.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use helikodu_model::audio::{
    BackendSelection, DeviceSelection, OutputBackendKind, OutputSelection,
};
use helikodu_service::player::{Player, PlayerConfig, PlayerEvent, SinkPacing};

pub(crate) const RATE: u32 = 44_100;

/// The folder for one test's files, emptied first.
pub(crate) fn folder(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("player")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Interleaved 16-bit stereo samples that differ from frame to frame: `seed` makes
/// the files of one test different from each other.
pub(crate) fn samples(frames: usize, seed: u32) -> Vec<i16> {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..frames * 2)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            // A quiet signal: the top bits of the generator, scaled down.
            ((state >> 16) as i16) / 8
        })
        .collect()
}

/// Writes 16-bit stereo `samples` as a WAV file at [`RATE`].
pub(crate) fn write_wav(path: &Path, samples: &[i16]) {
    let data_len = u32::try_from(samples.len() * 2).unwrap();
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

/// The 16-bit samples of a WAV file written by the WAV sink.
pub(crate) fn read_wav(path: &Path) -> Vec<i16> {
    let bytes = fs::read(path).unwrap();
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + len).min(bytes.len())];
        if id == b"fmt " {
            let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            assert_eq!(bits, 16, "the sink wrote {bits}-bit samples");
        }
        if id == b"data" {
            return body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair))
                .collect();
        }
        at += 8 + len + (len & 1);
    }
    panic!("no data chunk in {}", path.display());
}

/// A selection of the WAV sink writing to `path`.
pub(crate) fn wav_output(path: &Path) -> OutputSelection {
    OutputSelection {
        backend: BackendSelection::Backend(OutputBackendKind::Wav),
        device: DeviceSelection::Device(path.to_str().unwrap().parse().unwrap()),
        ..OutputSelection::default()
    }
}

/// A selection of the null sink.
pub(crate) fn null_output() -> OutputSelection {
    OutputSelection {
        backend: BackendSelection::Backend(OutputBackendKind::Null),
        ..OutputSelection::default()
    }
}

/// Starts a player on `selection`.
pub(crate) fn player(
    selection: OutputSelection,
    pacing: SinkPacing,
) -> (Player, Receiver<PlayerEvent>) {
    let mut config = PlayerConfig::new(selection);
    config.pacing = pacing;
    Player::spawn(config).unwrap()
}

/// Collects events until one matches `done`, which it returns with those before it.
pub(crate) fn until(
    events: &Receiver<PlayerEvent>,
    mut done: impl FnMut(&PlayerEvent) -> bool,
) -> (PlayerEvent, Vec<PlayerEvent>) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = events
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("timed out; events so far: {seen:#?}"));
        if done(&event) {
            return (event, seen);
        }
        seen.push(event);
    }
}

/// Waits for `TrackChanged` and returns its index.
pub(crate) fn next_track(events: &Receiver<PlayerEvent>) -> usize {
    match until(events, |e| matches!(e, PlayerEvent::TrackChanged { .. })).0 {
        PlayerEvent::TrackChanged { index, .. } => index,
        _ => unreachable!(),
    }
}
