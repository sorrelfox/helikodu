// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Events from the engine.

use serde::{Deserialize, Serialize};

use super::{EngineError, ItemId, TransportState};
use crate::audio::{AudioSpec, DeviceInfo, MediaTime, OutputBackendKind};

/// Something the engine reports.
///
/// At a gapless join the engine sends [`TrackEnded`](Self::TrackEnded) for the item
/// that ended and then [`TrackStarted`](Self::TrackStarted) for the next one, with no
/// state change in between.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineEvent {
    /// The transport moved to another state.
    StateChanged(TransportState),
    /// An item's audio began to reach the device.
    TrackStarted {
        /// The item.
        item: ItemId,
        /// The format of the decoded audio, before any conversion.
        spec: AudioSpec,
        /// The item's length, if the decoder knows it.
        length: Option<MediaTime>,
    },
    /// The playing position, sent a few times per second while playing and after
    /// every seek. It already allows for the device's delay.
    Position {
        /// The item.
        item: ItemId,
        /// The position from the start of the item.
        position: MediaTime,
    },
    /// The current item ends soon: the time to send the next one with
    /// [`SetNext`](super::EngineCommand::SetNext).
    NearEnd {
        /// The item that is about to end.
        item: ItemId,
    },
    /// An item stopped playing.
    TrackEnded {
        /// The item.
        item: ItemId,
        /// Why it stopped.
        reason: TrackEndReason,
    },
    /// A backend's device list changed. Carries the whole new list.
    DevicesChanged {
        /// The backend.
        backend: OutputBackendKind,
        /// Its devices.
        devices: Vec<DeviceInfo>,
    },
    /// Something went wrong.
    Error {
        /// The item it concerns, if any.
        item: Option<ItemId>,
        /// What went wrong.
        error: EngineError,
    },
}

/// Why an item stopped playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackEndReason {
    /// It played to its end.
    Finished,
    /// A command stopped it or replaced it.
    Interrupted,
    /// An error ended it; an [`EngineEvent::Error`] says which.
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{ChannelLayout, Endianness, SampleFormat, SampleRate};

    #[test]
    fn json_round_trip() {
        let events = [
            EngineEvent::StateChanged(TransportState::Playing),
            EngineEvent::TrackStarted {
                item: ItemId::new(7),
                spec: AudioSpec::new(
                    SampleFormat::S16(Endianness::Little),
                    SampleRate::HZ_44100,
                    ChannelLayout::STEREO,
                ),
                length: Some(MediaTime::from_frames(10_000_000, SampleRate::HZ_44100)),
            },
            EngineEvent::TrackEnded {
                item: ItemId::new(7),
                reason: TrackEndReason::Finished,
            },
            EngineEvent::Error {
                item: None,
                error: EngineError::NothingLoaded,
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            assert_eq!(
                serde_json::from_str::<EngineEvent>(&json).unwrap(),
                event,
                "{json}"
            );
        }
    }
}
