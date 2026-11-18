// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! An engine driven the way the service drives it: it answers `NearEnd` with the
//! next item of a queue and skips items that fail.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use helikodu_engine::output::Pacing;
use helikodu_engine::transport::{Engine, EngineConfig};
use helikodu_model::audio::{
    BackendSelection, DeviceId, DeviceSelection, MediaTime, OutputBackendKind, OutputSelection,
};
use helikodu_model::library::MediaSource;
use helikodu_model::player::{
    EngineCommand, EngineEvent, ItemId, PlaybackItem, TrackEndReason, TransportState,
};

/// How long any test waits for an event.
const PATIENCE: Duration = Duration::from_secs(30);

/// The WAV backend writing to `path`.
pub(crate) fn wav_output(path: &Path) -> OutputSelection {
    OutputSelection {
        backend: BackendSelection::Backend(OutputBackendKind::Wav),
        device: DeviceSelection::Device(DeviceId::new(path.to_string_lossy()).unwrap()),
        ..OutputSelection::default()
    }
}

/// A configuration that writes to the WAV file `path` as fast as it can.
pub(crate) fn wav_config(path: &Path) -> EngineConfig {
    let mut config = EngineConfig {
        sink_pacing: Pacing::Freewheel,
        ..EngineConfig::default()
    };
    let selection = wav_output(path);
    config.output.backend = selection.backend;
    config.output.device = selection.device;
    config
}

/// A configuration for the null backend, as fast as it can.
pub(crate) fn null_config() -> EngineConfig {
    let mut config = EngineConfig {
        sink_pacing: Pacing::Freewheel,
        ..EngineConfig::default()
    };
    config.output.backend = BackendSelection::Backend(OutputBackendKind::Null);
    config
}

pub(crate) fn item(id: u64, path: &Path) -> PlaybackItem {
    PlaybackItem::new(ItemId::new(id), MediaSource::file(path))
}

pub(crate) struct Session {
    pub(crate) engine: Engine,
    events: Receiver<EngineEvent>,
    /// Every event so far.
    pub(crate) log: Vec<EngineEvent>,
    /// What follows each item.
    successors: HashMap<ItemId, Option<PlaybackItem>>,
}

impl Session {
    pub(crate) fn new(config: EngineConfig) -> Self {
        let (engine, events) = Engine::spawn(config).unwrap();
        Self {
            engine,
            events,
            log: Vec::new(),
            successors: HashMap::new(),
        }
    }

    pub(crate) fn send(&self, command: EngineCommand) {
        self.engine.send(command).unwrap();
    }

    /// Plays `items` in order: loads the first, and answers `NearEnd` with the rest.
    pub(crate) fn play_queue(&mut self, items: &[PlaybackItem]) {
        for pair in items.windows(2) {
            self.successors.insert(pair[0].id, Some(pair[1].clone()));
        }
        self.send(EngineCommand::Load {
            item: items[0].clone(),
            start: MediaTime::ZERO,
            paused: false,
        });
    }

    pub(crate) fn load(&self, item: &PlaybackItem, start: MediaTime) {
        self.send(EngineCommand::Load {
            item: item.clone(),
            start,
            paused: false,
        });
    }

    /// Waits for the first event that `wanted` accepts, acting as the service on
    /// the way, and returns it.
    pub(crate) fn wait_for(&mut self, wanted: impl Fn(&EngineEvent) -> bool) -> EngineEvent {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = self
                .events
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no such event; got {:#?}", self.log));
            self.serve(&event);
            self.log.push(event.clone());
            if wanted(&event) {
                return event;
            }
        }
    }

    pub(crate) fn wait_state(&mut self, state: TransportState) {
        self.wait_for(|e| *e == EngineEvent::StateChanged(state));
    }

    /// Waits for the next `Position` and returns it in frames at `rate`.
    pub(crate) fn wait_position(&mut self, rate: u32) -> u64 {
        match self.wait_for(|e| matches!(e, EngineEvent::Position { .. })) {
            EngineEvent::Position { position, .. } => {
                position.frames_at(helikodu_model::audio::SampleRate::new(rate).unwrap())
            }
            _ => unreachable!(),
        }
    }

    /// The service's part: the next item on `NearEnd`, and skipping failed items.
    fn serve(&mut self, event: &EngineEvent) {
        match event {
            EngineEvent::NearEnd { item } => {
                let next = self.successors.get(item).cloned().flatten();
                self.send(EngineCommand::SetNext(next));
            }
            EngineEvent::TrackEnded {
                item,
                reason: TrackEndReason::Failed,
            } => {
                let after = self.successors.get(item).cloned().flatten();
                for successor in self.successors.values_mut() {
                    if successor.as_ref().is_some_and(|s| s.id == *item) {
                        *successor = after.clone();
                    }
                }
            }
            _ => {}
        }
    }

    /// The logged events without the frequent and the incidental ones.
    pub(crate) fn milestones(&self) -> Vec<EngineEvent> {
        self.log
            .iter()
            .filter(|e| {
                !matches!(
                    e,
                    EngineEvent::Position { .. }
                        | EngineEvent::DevicesChanged { .. }
                        | EngineEvent::NearEnd { .. }
                        | EngineEvent::SignalPath(_)
                        | EngineEvent::PlaybackStats(_)
                        | EngineEvent::Limiter(_)
                )
            })
            .cloned()
            .collect()
    }
}
