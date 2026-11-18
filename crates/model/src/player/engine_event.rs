// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Events from the engine.

use serde::{Deserialize, Serialize};

use super::{EngineError, ItemId, LimiterActivity, PlaybackStats, SignalPath, TransportState};
use crate::audio::{AudioSpec, DeviceInfo, MediaTime, OutputBackendKind};

/// One audio stream of a file with several, as the stream selector shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioStreamInfo {
    /// The stream's index in the file, as
    /// [`PlaybackItem::audio_stream`](super::PlaybackItem::audio_stream) names it.
    pub index: u32,
    /// The codec, such as `ac3` or `aac`.
    pub codec: String,
    /// The language tag, if the file gives one.
    pub language: Option<String>,
    /// The stream's title, if the file gives one.
    pub title: Option<String>,
    /// The channel count.
    pub channels: Option<u16>,
    /// The sample rate in Hz.
    pub sample_rate: Option<u32>,
    /// Whether the file marks the stream as the default one.
    pub default: bool,
}

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
    /// The signal path of the item being heard changed: another item, another plan
    /// (gain, DSP, volume), another stream or device format. `None` when nothing is
    /// being played.
    SignalPath(Option<Box<SignalPath>>),
    /// The open stream's counters, a few times per second while playing.
    PlaybackStats(PlaybackStats),
    /// What the limiter does: sent when it changes, at most a few times per
    /// second, so nothing is sent while the limiter is idle. The counts start
    /// anew with each track. `None` when the limiter is off.
    Limiter(Option<LimiterActivity>),
    /// The audio streams of an item's file, sent when the item starts and the file
    /// has more than one, for the stream selector.
    AudioStreams {
        /// The item.
        item: ItemId,
        /// The file's audio streams, in file order.
        streams: Vec<AudioStreamInfo>,
        /// The index of the stream that plays.
        playing: u32,
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
            EngineEvent::SignalPath(None),
            EngineEvent::PlaybackStats(PlaybackStats {
                buffer_ms: 200,
                buffered_ms: 184,
                underruns: 1,
                frames_delivered: 48_000,
                silent_frames: 256,
                total_underruns: 2,
            }),
            EngineEvent::Limiter(Some(LimiterActivity {
                reduction_db: 1.5,
                largest_db: 2.25,
                times: 3,
            })),
            EngineEvent::Limiter(None),
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
