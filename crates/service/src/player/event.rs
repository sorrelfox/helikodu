// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Events of the player service.

use std::path::PathBuf;

use helikodu_library::ListenOutcome;
use helikodu_model::audio::{AudioSpec, DeviceInfo, MediaTime, OutputBackendKind, OutputSelection};
use helikodu_model::library::{TrackId, TrackRef};
use helikodu_model::player::{
    AudioStreamInfo, EngineError, EntryId, PlayModes, PlaybackState, PlaybackStats, SignalPath,
    StopAfter, TrackFlags, TransportState, Volume,
};

use helikodu_model::settings::SpeedSettings;

use super::timers::{AbLoop, SleepTimerStatus};

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
    /// The signal path of what is heard changed: another entry, another plan (gain,
    /// DSP, volume) or another device format. `None` when nothing plays. Also in
    /// [`PlayerStatus::signal_path`](super::PlayerStatus::signal_path).
    SignalPath(Option<Box<SignalPath>>),
    /// The output's counters (buffer, underruns), a few times per second while
    /// playing.
    PlaybackStats(PlaybackStats),
    /// The audio streams of the current entry's file, when it has several: for the
    /// stream selector in the signal path's Source row. Sent when the entry
    /// starts and after [`SelectAudioStream`](super::PlayerCommand::SelectAudioStream).
    AudioStreams {
        /// The entry.
        entry: EntryId,
        /// The file's audio streams, in file order.
        streams: Vec<AudioStreamInfo>,
        /// The index of the stream that plays.
        playing: u32,
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
    /// The A-B loop of the playing track changed: a point was set, or the loop
    /// ended (`None`), also because another track started.
    LoopChanged(Option<AbLoop>),
    /// The sleep timer started, changed or ended (`None`).
    SleepTimerChanged(Option<SleepTimerStatus>),
    /// The playback speed changed (the settings', or one set with `SetSpeed`).
    SpeedChanged(SpeedSettings),
    /// The entry selected for "Continue from the selected track" changed.
    CursorChanged(Option<EntryId>),
    /// The playback flags of a library track were changed through the player.
    TrackFlagsChanged {
        /// The track.
        track: TrackId,
        /// Its flags now.
        flags: TrackFlags,
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
    /// Stop-after stopped playback: after the track, the album or the queue.
    StopAfter,
    /// The sleep timer's time was up.
    SleepTimer,
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
    /// The playback flags of a track could not be stored.
    FlagsNotSaved {
        /// The track.
        track: TrackId,
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
