// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Playback: the commands the engine accepts, the events it sends, and the values they
//! carry.
//!
//! The engine's own messages: the engine plays what it is told and decides nothing
//! about what comes next. The queue, its entries and context, and the play modes,
//! which the service owns and front ends see.

mod engine_command;
mod engine_error;
mod engine_event;
mod playback_item;
mod queue;
mod signal_path;
mod transport_state;
mod volume;

pub use engine_command::EngineCommand;
pub use engine_error::EngineError;
pub use engine_event::{AudioStreamInfo, EngineEvent, TrackEndReason};
pub use playback_item::{ItemId, Loudness, PlaybackItem};
pub use queue::{
    ContextSource, EnqueueAt, EntryId, PlayModes, PlaybackState, QueueContext, QueueEntry,
    RepeatMode, ShuffleMode, StopAfter,
};
pub use signal_path::{
    PlaybackStats, SignalDevice, SignalLamp, SignalPath, SignalSource, SignalStage, StageKind,
    StageState,
};
pub use transport_state::TransportState;
pub use volume::Volume;
