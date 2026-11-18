// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Events of the player service.

use std::path::PathBuf;

use helikodu_library::ListenOutcome;
use helikodu_model::audio::{AudioSpec, DeviceInfo, MediaTime, OutputBackendKind, OutputSelection};
use helikodu_model::library::{TrackId, TrackRef};
use helikodu_model::player::{
    EngineError, EntryId, PlayModes, PlaybackState, StopAfter, TransportState, Volume,
};

/// Something the player reports. Every event comes from the player's own thread, in
/// the order things happened.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PlayerEvent {
    /// The state of playback changed: what the transport controls show, and the
    /// engine's own state underneath it.
    State {
        /// As the controls show it.
        playback: PlaybackState,
        /// The engine's transport.
        transport: TransportState,
    },
    /// An entry of the queue began to play: the now-playing information.
    TrackChanged {
        /// Its place in the queue (play order), from 0.
        index: usize,
        /// The number of entries in the queue.
        count: usize,
        /// The entry.
        entry: EntryId,
        /// What it plays.
        track: TrackRef,
        /// The file.
        path: PathBuf,
        /// The format of its decoded audio.
        spec: AudioSpec,
        /// Its length, if the decoder knows it.
        length: Option<MediaTime>,
    },
    /// The playing position in the current entry: a few times per second while
    /// playing and after every seek. Front ends interpolate between them.
    Position {
        /// The entry's place in the queue.
        index: usize,
        /// The position from its start.
        position: MediaTime,
        /// Its length, if known.
        length: Option<MediaTime>,
    },
    /// The queue changed: entries, order, the current entry or the context. Front
    /// ends fetch it again with [`Player::queue`](super::Player::queue).
    QueueChanged {
        /// The queue's version now.
        version: u64,
    },
    /// Shuffle or repeat changed.
    ModesChanged(PlayModes),
    /// Stop-after was set or cleared.
    StopAfterChanged(StopAfter),
    /// The volume changed.
    VolumeChanged(Volume),
    /// Playback was moved to another backend, device or mode.
    OutputChanged(OutputSelection),
    /// A backend's device list changed, for the output quick switcher.
    DevicesChanged {
        /// The backend.
        backend: OutputBackendKind,
        /// Its devices now.
        devices: Vec<DeviceInfo>,
    },
    /// Something went wrong. Playback goes on where it can: an entry that cannot be
    /// played is skipped.
    Problem {
        /// The file it concerns, with its place in the queue, if any.
        file: Option<(usize, PathBuf)>,
        /// The queue entry it concerns, if any.
        entry: Option<EntryId>,
        /// What went wrong.
        error: EngineError,
    },
    /// Something the user should be told that is not an error of playback.
    Notice(Notice),
    /// A listen to a library track was recorded in the statistics.
    ListenRecorded {
        /// The track.
        track: TrackId,
        /// What it counted as.
        outcome: ListenOutcome,
    },
    /// Playback stopped and the player waits for a new command.
    Ended {
        /// Why.
        reason: EndReason,
        /// How many entries began to play since the queue was set.
        played: usize,
        /// How many could not be played, or failed while playing.
        failed: usize,
    },
}

/// Why playback ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EndReason {
    /// The queue played to its end, or [`Next`](super::PlayerCommand::Next) went
    /// past it.
    QueueFinished,
    /// [`Stop`](super::PlayerCommand::Stop) was sent.
    Stopped,
    /// The track marked with stop-after ended.
    StopAfter,
    /// The output failed and the engine stopped; a [`PlayerEvent::Problem`] said why.
    OutputFailed,
}

/// A message for the user about the session or the statistics.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Notice {
    /// The program did not end normally last time; the session was restored paused.
    RecoveredAfterCrash {
        /// What was playing or loading then.
        tracks: Vec<TrackRef>,
        /// Those of them that were named at two abnormal ends in a row: automatic
        /// playback skips them from now on, until one plays to its end.
        suspects: Vec<TrackRef>,
    },
    /// Automatic playback passed over a track that has ended the program before.
    SuspectSkipped {
        /// The entry.
        entry: EntryId,
        /// Its track.
        track: TrackRef,
    },
    /// A context could not be turned into tracks.
    ContextUnavailable {
        /// Why.
        reason: String,
    },
    /// The session could not be saved.
    SessionNotSaved {
        /// Why.
        reason: String,
    },
    /// A listen could not be recorded.
    ListenNotRecorded {
        /// The track.
        track: TrackId,
        /// Why.
        reason: String,
    },
}
