// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Commands to the player service.

use std::path::PathBuf;
use std::time::Duration;

use helikodu_model::audio::{MediaTime, OutputSelection};
use helikodu_model::library::TrackRef;
use helikodu_model::player::{
    EnqueueAt, EntryId, QueueContext, RepeatMode, ShuffleMode, StopAfter, Volume,
};
use helikodu_model::settings::{LoudnessSettings, OutputSettings};

use super::config::PlayerPrefs;
use crate::library::{AlbumId, GenreId, PersonId};

/// A context the service turns into tracks by asking the library.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ContextRequest {
    /// An album, in album order.
    Album(AlbumId),
    /// Every track of a person, in any role.
    Artist(PersonId),
    /// Every track of a genre.
    Genre(GenreId),
    /// The results of a search, best first (at most 5000).
    Search(String),
}

/// A command to the player. Every front end (window, MPRIS, command line) sends the
/// same commands.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PlayerCommand {
    /// Play `files` from the one at `start`: [`PlayTracks`](Self::PlayTracks) with
    /// loose files as the context.
    PlayFiles {
        /// The files, in the order they play.
        files: Vec<PathBuf>,
        /// The index of the first file to play.
        start: usize,
    },
    /// Play the tracks of `context` from the one at `start`. The service looks the
    /// tracks up in the library: albums, artists, genres and searches. Folders,
    /// playlists and loose files need [`PlayTracks`](Self::PlayTracks).
    ///
    /// The queue is replaced, or the tracks are added to its end, as the setting
    /// [`PlayerPrefs::append_contexts`] says.
    PlayContext {
        /// What to play.
        context: ContextRequest,
        /// Its name as shown, when the front end knows it; an album's title is looked
        /// up otherwise.
        title: Option<String>,
        /// The index of the first track to play.
        start: usize,
    },
    /// Play `tracks`, which came from `context`, from the one at `start`.
    PlayTracks {
        /// Where they came from, for display.
        context: QueueContext,
        /// The tracks, in context order.
        tracks: Vec<TrackRef>,
        /// The index of the first track to play.
        start: usize,
    },
    /// "Play next" or "Add to queue".
    Enqueue {
        /// The tracks.
        tracks: Vec<TrackRef>,
        /// Where they go.
        at: EnqueueAt,
    },
    /// Play this entry of the queue now, also if automatic playback would pass over
    /// it.
    PlayEntry(EntryId),
    /// Remove entries from the queue. Removing the playing entry plays the next one.
    Remove(Vec<EntryId>),
    /// Move an entry to `to`, an index in play order (counted without the entry).
    Move {
        /// The entry.
        entry: EntryId,
        /// Where to.
        to: usize,
    },
    /// Empty the queue, except the entry that is playing or paused.
    Clear,
    /// Pause when playing; play otherwise.
    PlayPause,
    /// Resume, or after a stop play the current entry again.
    Play,
    /// Pause, keeping the position.
    Pause,
    /// Stop and release the device.
    Stop,
    /// Go to the next entry; past the last one, playback ends (unless repeating).
    Next,
    /// Restart the current entry, or go to the one before (see
    /// [`PlayerPrefs::previous_restart`]).
    Previous,
    /// Move to a position in the current entry.
    Seek(MediaTime),
    /// Move forward in the current entry.
    SeekForward(Duration),
    /// Move back in the current entry.
    SeekBackward(Duration),
    /// Set the volume.
    SetVolume(Volume),
    /// Change the volume by this much of the whole range (from -1.0 to 1.0).
    AdjustVolume(f32),
    /// Shuffle on or off.
    SetShuffle(ShuffleMode),
    /// Repeat off, one or all.
    SetRepeat(RepeatMode),
    /// Stop after the current track, or not.
    SetStopAfter(StopAfter),
    /// Move playback to another backend, device or mode, keeping the position.
    SetOutput(OutputSelection),
    /// Move playback to the next output device: through the devices of every
    /// sound backend, or of the null backend while it is used.
    NextOutput,
    /// Change how the player behaves (from the playback settings).
    Configure(PlayerPrefs),
    /// Apply the `[output]` and `[loudness]` settings while playing: a new sample
    /// rate policy, sample format, buffer length or volume control reopens the stream
    /// at the same position; loudness changes ramp the gain without a gap. The backend,
    /// device and mode in `output` are left to [`SetOutput`](Self::SetOutput).
    ConfigureAudio {
        /// The `[output]` settings.
        output: OutputSettings,
        /// The `[loudness]` settings.
        loudness: LoudnessSettings,
    },
}
