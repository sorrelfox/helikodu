// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Commands to the player service.

use std::path::PathBuf;
use std::time::Duration;

use helikodu_model::audio::{MediaTime, OutputSelection};
use helikodu_model::library::{TrackId, TrackRef};
use helikodu_model::player::{
    EnqueueAt, EntryId, LoopRange, PlayScope, QueueContext, RepeatMode, ShuffleMode, StopAfter,
    TrackFlags, Volume,
};
use helikodu_model::settings::{
    DecoderSettings, DspSettings, LoudnessSettings, OutputSettings, SmartPlaySettings,
    SpeedSettings, TransitionSettings,
};

use super::config::PlayerPrefs;
use super::timers::SleepWhen;
use crate::library::{AlbumId, GenreId, PersonId, PlaySeed, YearSpan};

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
    /// Every track on which the person is a composer.
    Composer(PersonId),
    /// Every track of a span of release years.
    Years(YearSpan),
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
    /// tracks up in the library: albums, artists, genres, composers, years and
    /// searches. Audiobooks are left out of every context but their album. Folders,
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
    /// Plays the restored session's current entry from its saved position if it
    /// was playing when the program ended, fading in as after a pause. Does
    /// nothing if it was paused or stopped, if its track may have ended the
    /// program abnormally (see [`session`](super::session)), if its file is
    /// missing, or once anything else has been played. A front end sends it once
    /// at the start, unless it was started to play something else.
    ResumeSession,
    /// The desktop session is about to end, which may end the program without a
    /// normal quit: the session is saved now, and the tracks playing are not
    /// counted as having ended the program abnormally, until playback moves on.
    SessionEnding,
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
    /// Shuffle off, or by tracks, albums, folders, groupings or within albums.
    SetShuffle(ShuffleMode),
    /// Repeat off, one, the album or all.
    SetRepeat(RepeatMode),
    /// Stop after the current track, the album or the queue, or not.
    SetStopAfter(StopAfter),
    /// Consume ("Remove tracks after they play") on or off.
    SetConsume(bool),
    /// "Play only": every entry, those of the current artist, or of the current
    /// album.
    SetPlayScope(PlayScope),
    /// "Continue from the selected track" on or off.
    SetFollowCursor(bool),
    /// Intro scan on or off: only the first seconds of each track play
    /// (`playback.intro_scan_s`).
    SetIntroScan(bool),
    /// "Revert": back to the entry "Next" last moved on from, where it was.
    /// Nothing happens when there is none still in the queue.
    Revert,
    /// "Radio Random": a random entry of the queue, from a random point in it.
    RadioRandom,
    /// Starts endless play from `seed`: a track plays first, an album plays
    /// through, an artist starts with related tracks; tracks related to the seed
    /// follow whenever the queue runs out (the repeat mode becomes
    /// [`RepeatMode::Endless`]).
    StartRadio(PlaySeed),
    /// Puts back exactly what the last [`StartRadio`](Self::StartRadio)
    /// replaced: the entries (queued ones stay queued), their play order with a
    /// shuffled one, the current entry and its position, playing, paused or
    /// stopped as it was, and the play modes. Nothing happens once the queue has
    /// been changed otherwise since (a new queue, entries added, removed or
    /// moved, the queue cleared, or shuffle or repeat changed).
    UndoRadio,
    /// The entry selected in the queue, for "Continue from the selected track".
    SetCursor(Option<EntryId>),
    /// Go to the first track of the next album in the queue.
    NextAlbum,
    /// Go to the first track of the album before, or of the current one when it is
    /// the first.
    PreviousAlbum,
    /// Go to the first track of an album of the queue chosen at random.
    RandomAlbum,
    /// Set the start of the A-B loop at the playing position.
    LoopStartHere,
    /// Set the end of the A-B loop at the playing position; the loop then plays.
    LoopEndHere,
    /// Set both points of the A-B loop (dragging its flags), or clear it with
    /// `None`.
    SetLoop(Option<LoopRange>),
    /// Start the sleep timer, replacing a running one.
    StartSleepTimer {
        /// How long until the time is up.
        length: Duration,
        /// What happens then.
        when: SleepWhen,
    },
    /// Give the running sleep timer more time.
    ExtendSleepTimer(Duration),
    /// Turn the sleep timer off.
    CancelSleepTimer,
    /// Play at another speed, keeping the pitch or not, until the program ends
    /// (it wins over the `[dsp.speed]` settings until then). Positions stay those
    /// of the track; playback goes on where it is at the new speed.
    SetSpeed(SpeedSettings),
    /// Change the playback flags of a library track; they are stored in the library
    /// and apply to the queue at once (the volume from the next time it plays).
    SetTrackFlags {
        /// The track.
        track: TrackId,
        /// Its flags.
        flags: TrackFlags,
    },
    /// Move playback to another backend, device or mode, keeping the position.
    SetOutput(OutputSelection),
    /// Move playback to the next output device: through the devices of every
    /// sound backend, or of the null backend while it is used.
    NextOutput,
    /// Change how the player behaves (from the playback settings).
    Configure(PlayerPrefs),
    /// Change weighted and filtered shuffle, endless play, the Auto-DJ and intro
    /// scan (from the playback settings).
    ConfigureSmartPlay(SmartPlaySettings),
    /// Apply the `[output]` and `[loudness]` settings while playing: a new sample
    /// rate policy, sample format, buffer length or volume control reopens the stream
    /// at the same position; loudness changes ramp the gain without a gap. The backend,
    /// device and mode in `output` are left to [`SetOutput`](Self::SetOutput).
    ConfigureAudio {
        /// The `[output]` settings.
        output: OutputSettings,
        /// The `[loudness]` settings.
        loudness: LoudnessSettings,
        /// The `[dsp]` settings: chain changes crossfade in, the resampler quality
        /// applies from the next track. Boxed: they are large.
        dsp: Box<DspSettings>,
    },
    /// Apply the crossfade and fade settings (`[playback.transitions]`) while
    /// playing: fades apply from the next transition, crossfades from the next track
    /// change.
    ConfigureTransitions(TransitionSettings),
    /// Apply the `[playback.decoders]` settings (loops, fades, SoundFont, SID song
    /// lengths and ROMs, tracker rendering, whole-file buffering) to the tracks
    /// opened from now on.
    ConfigureDecoders(DecoderSettings),
    /// Play another audio stream of the current track's file, by its index in the
    /// file (`None`: the default one), from the same position. The engine
    /// answers with [`PlayerEvent::AudioStreams`](super::PlayerEvent::AudioStreams).
    SelectAudioStream(Option<u32>),
}
