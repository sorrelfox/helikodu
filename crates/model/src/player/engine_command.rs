// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Commands to the engine.

use serde::{Deserialize, Serialize};

use super::{ItemId, PlaybackItem, Volume};
use crate::audio::{MediaTime, OutputSelection};
use crate::settings::{
    DecoderSettings, DspSettings, LoudnessSettings, OutputSettings, TransitionSettings,
};

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
    /// The item to join without a gap when the current one ends (or to crossfade
    /// into, when crossfades are on), or `None` to end playback there. Replaces any
    /// earlier next item.
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
    /// rendered from then on, with a short gain ramp, without reopening the stream. A
    /// change of the DSP chain crossfades to the new chain within a few tens of
    /// milliseconds of the audio rendered from then on; the resampler quality applies
    /// to the next track, or after a seek.
    UpdateSettings {
        /// The `[output]` settings.
        output: OutputSettings,
        /// The `[loudness]` settings.
        loudness: LoudnessSettings,
        /// The `[dsp]` settings (boxed: they are large).
        #[serde(default)]
        dsp: Box<DspSettings>,
    },
    /// Replace the settings of the long-tail decoders (loops, fades, SoundFont, SID
    /// song lengths and ROMs, tracker rendering, whole-file buffering). They apply to
    /// the items opened from then on.
    SetDecoderSettings(DecoderSettings),
    /// Play another audio stream of the loaded item's file, by its index in the file
    /// (`None`: the default one), continuing at the heard position. Ignored when
    /// nothing is loaded.
    SelectAudioStream(Option<u32>),
    /// Replace the crossfade and fade settings while running. Fades apply from the
    /// next transition; a crossfade already under way finishes as it began.
    SetTransitions(TransitionSettings),
    /// Loop a section of `item`, the loaded item, or end the loop with `None`.
    ///
    /// When playback reaches the end of the range it continues at its start without
    /// a gap: the frame after the last one of the range is the range's first frame,
    /// exactly. Audio already rendered past the end is dropped from the buffer, so
    /// a loop whose end is behind the heard position jumps to its start at once. The
    /// loop ends with the item; a seek past its end leaves it. Ignored when `item`
    /// is not the loaded item or the range is empty.
    SetLoop {
        /// The item the range belongs to.
        item: ItemId,
        /// The section, or `None` to end the loop.
        range: Option<LoopRange>,
    },
}

/// A section of an item, from `start` to just before `end`, for an A-B loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LoopRange {
    /// The first position of the section (A).
    pub start: MediaTime,
    /// The position just after its last frame (B).
    pub end: MediaTime,
}

impl LoopRange {
    /// The range from `start` to `end`, or `None` when it is empty.
    pub fn new(start: MediaTime, end: MediaTime) -> Option<Self> {
        (start < end).then_some(Self { start, end })
    }
}
