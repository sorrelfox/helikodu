// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Playback: the commands the engine accepts, the events it sends, and the values they
//! carry.
//!
//! These are the engine's own messages. The engine plays what it is told and decides
//! nothing about what comes next; the queue, play modes and the commands front ends
//! send to the service are not defined here yet.

mod engine_command;
mod engine_error;
mod engine_event;
mod playback_item;
mod transport_state;
mod volume;

pub use engine_command::EngineCommand;
pub use engine_error::EngineError;
pub use engine_event::{EngineEvent, TrackEndReason};
pub use playback_item::{ItemId, Loudness, PlaybackItem};
pub use transport_state::TransportState;
pub use volume::Volume;
