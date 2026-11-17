// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Events of the player service.

use std::path::PathBuf;

use helikodu_model::audio::{AudioSpec, MediaTime, OutputSelection};
use helikodu_model::player::{EngineError, TransportState, Volume};

/// Something the player reports.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    /// The engine's transport moved to another state.
    State(TransportState),
    /// Another file of the queue began to play.
    TrackChanged {
        /// Its place in the queue, from 0.
        index: usize,
        /// The number of files in the queue.
        count: usize,
        /// The file.
        path: PathBuf,
        /// The format of its decoded audio.
        spec: AudioSpec,
        /// Its length, if the decoder knows it.
        length: Option<MediaTime>,
    },
    /// The playing position in the current file, a few times per second while
    /// playing and after every seek.
    Position {
        /// The file's place in the queue.
        index: usize,
        /// The position from its start.
        position: MediaTime,
        /// Its length, if known.
        length: Option<MediaTime>,
    },
    /// The volume changed.
    VolumeChanged(Volume),
    /// Playback was moved to another backend, device or mode.
    OutputChanged(OutputSelection),
    /// Something went wrong. Playback goes on where it can: a file that cannot be
    /// played is skipped.
    Problem {
        /// The file it concerns, with its place in the queue, if any.
        file: Option<(usize, PathBuf)>,
        /// What went wrong.
        error: EngineError,
    },
    /// Playback stopped and the player waits for a new command.
    Ended {
        /// Why.
        reason: EndReason,
        /// How many files began to play since the queue was set.
        played: usize,
        /// How many files could not be played, or failed while playing.
        failed: usize,
    },
}

/// Why playback ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndReason {
    /// The queue played to its end, or [`Next`](super::PlayerCommand::Next) went
    /// past it.
    QueueFinished,
    /// [`Stop`](super::PlayerCommand::Stop) was sent.
    Stopped,
    /// The output failed and the engine stopped; a [`PlayerEvent::Problem`] said why.
    OutputFailed,
}
