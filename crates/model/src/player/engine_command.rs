// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Commands to the engine.

use serde::{Deserialize, Serialize};

use super::{PlaybackItem, Volume};
use crate::audio::{MediaTime, OutputSelection};
use crate::settings::{LoudnessSettings, OutputSettings};

/// A command to the engine. The engine answers with [`EngineEvent`](super::EngineEvent)s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineCommand {
    /// Open `item`, replacing whatever was loaded, and play it from `start`.
    Load {
        /// What to play.
        item: PlaybackItem,
        /// Where to start, from the beginning of the item's source or section.
        start: MediaTime,
        /// Stay paused at `start` instead of playing, for example when a session is
        /// restored.
        paused: bool,
    },
    /// Play or resume the loaded item.
    Play,
    /// Pause, keeping the position.
    Pause,
    /// Stop and release the device.
    Stop,
    /// Move to a position in the loaded item.
    Seek(MediaTime),
    /// The item to join without a gap when the current one ends, or `None` to end
    /// playback there. Replaces any earlier next item.
    SetNext(Option<PlaybackItem>),
    /// Change the volume.
    SetVolume(Volume),
    /// Move playback to another backend, device or mode, keeping the position.
    SetOutput(OutputSelection),
    /// Replace the output and loudness settings while running.
    ///
    /// The backend, device and mode in `output` are ignored: [`SetOutput`](Self::SetOutput)
    /// chooses them. A change of the sample rate policy, sample format, buffer length or
    /// volume control reopens the stream at the heard position, as `SetOutput` does; the
    /// other output settings apply from then on. A loudness change applies to the audio
    /// rendered from then on, with a short gain ramp, without reopening the stream.
    UpdateSettings {
        /// The `[output]` settings.
        output: OutputSettings,
        /// The `[loudness]` settings.
        loudness: LoudnessSettings,
    },
}
