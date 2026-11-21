// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the MPRIS interfaces show, kept apart from D-Bus so that the mapping can
//! be tested on its own: the player's state as the events tell it, the values of
//! the `org.mpris.MediaPlayer2.Player` properties derived from it, the track list
//! and the playlist that plays, and which of them changed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use helikodu_model::audio::MediaTime;
use helikodu_model::library::TrackRef;
use helikodu_model::player::{
    ContextSource, EntryId, PlayModes, PlaybackState, QueueEntry, RepeatMode, ShuffleMode, Volume,
};
use helikodu_service::player::{PlayerEvent, PlayerStatus, QueueSnapshot};

use super::{TrackDetails, file_uri, track_id};

/// The `mpris:trackid` when nothing is current, as the specification reserves it.
pub(crate) const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

/// The most entries the track list shows. A longer queue shows a part of it
/// that holds the current entry and what follows.
pub const TRACK_LIST_LIMIT: usize = 1000;

/// The step by which the shown part of a long queue moves: it starts at a
/// multiple of this, so it moves seldom as playback goes on.
const TRACK_LIST_STEP: usize = 500;

/// A position that differs this much from where it should be is a seek.
const SEEK_TOLERANCE: Duration = Duration::from_millis(1000);

/// One value of the `Metadata` dictionary.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MetaValue {
    /// A D-Bus object path (`o`).
    Path(String),
    /// A string (`s`).
    Text(String),
    /// A list of strings (`as`).
    List(Vec<String>),
    /// A 32-bit integer (`i`).
    Int(i32),
    /// A 64-bit integer (`x`).
    Long(i64),
    /// A double (`d`).
    Real(f64),
}

/// The `Metadata` dictionary, sorted so that two of them compare as values.
pub(crate) type Metadata = BTreeMap<&'static str, MetaValue>;

/// The values of the `Player` properties that are announced with
/// `PropertiesChanged` (all but `Position` and the constant ones).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Properties {
    pub(crate) playback_status: &'static str,
    pub(crate) loop_status: &'static str,
    pub(crate) shuffle: bool,
    pub(crate) volume: f64,
    pub(crate) metadata: Metadata,
    pub(crate) can_go_next: bool,
    pub(crate) can_go_previous: bool,
    pub(crate) can_play: bool,
    pub(crate) can_pause: bool,
    pub(crate) can_seek: bool,
}

/// A property that changed, with its new value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Changed {
    Text(&'static str, &'static str),
    Flag(&'static str, bool),
    Real(&'static str, f64),
    Metadata(Metadata),
}

/// How the track list changed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TrackListChange {
    /// One entry came in after the track with this id (`NoTrack` for the start).
    Added {
        entry: EntryId,
        track: TrackRef,
        after: String,
    },
    /// The track with this id left.
    Removed(String),
    /// Anything else: the whole list and the current track's id.
    Replaced {
        tracks: Vec<String>,
        current: String,
    },
}

/// What an update asks to announce, in this order.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Announcement {
    /// `PropertiesChanged` on the `Player` interface.
    pub(crate) changed: Vec<Changed>,
    /// A `Seeked` signal with the new position in microseconds.
    pub(crate) seeked: Option<i64>,
    /// A change of the track list (`Tracks` is invalidated with it).
    pub(crate) tracks: Option<TrackListChange>,
    /// `TrackMetadataChanged`: the current track's metadata changed while it
    /// stayed current.
    pub(crate) track_metadata: Option<(String, Metadata)>,
    /// The playlist that plays changed (its id, if one plays).
    pub(crate) active_playlist: Option<Option<u64>>,
}

impl Announcement {
    pub(crate) fn is_empty(&self) -> bool {
        self.changed.is_empty()
            && self.seeked.is_none()
            && self.tracks.is_none()
            && self.track_metadata.is_none()
            && self.active_playlist.is_none()
    }
}

/// The entry that is current.
#[derive(Debug, Clone, PartialEq)]
struct Current {
    entry: EntryId,
    track: Option<TrackRef>,
    path: PathBuf,
    length: Option<Duration>,
}

/// The player as the MPRIS interfaces see it.
#[derive(Debug)]
pub(crate) struct State {
    playback: PlaybackState,
    modes: PlayModes,
    volume: f64,
    current: Option<Current>,
    /// The current entry's place in the queue (play order) and the queue's length.
    index: Option<usize>,
    count: usize,
    /// The last known position and when it was known.
    position: Duration,
    position_at: Instant,
    /// What the front end looked up for an entry, and the `file://` URL of its
    /// exported cover.
    details: Option<(EntryId, TrackDetails, Option<String>)>,
    /// The entries the track list shows, in play order.
    tracks: Vec<QueueEntry>,
    /// The ids of the track list as last announced.
    published_tracks: Vec<EntryId>,
    /// The playlist the queue came from, and the one last announced.
    playlist: Option<u64>,
    published_playlist: Option<u64>,
    /// The values last announced.
    published: Properties,
}

impl State {
    pub(crate) fn new() -> Self {
        let mut state = Self {
            playback: PlaybackState::Stopped,
            modes: PlayModes::default(),
            volume: f64::from(Volume::FULL.position()),
            current: None,
            index: None,
            count: 0,
            position: Duration::ZERO,
            position_at: Instant::now(),
            details: None,
            tracks: Vec::new(),
            published_tracks: Vec::new(),
            playlist: None,
            published_playlist: None,
            published: Properties {
                playback_status: "Stopped",
                loop_status: "None",
                shuffle: false,
                volume: 1.0,
                metadata: Metadata::new(),
                can_go_next: false,
                can_go_previous: false,
                can_play: false,
                can_pause: false,
                can_seek: false,
            },
        };
        state.published = state.properties();
        state
    }

    /// The values of the properties now.
    pub(crate) fn properties(&self) -> Properties {
        let has_track = self.current.is_some();
        let can_go_next = match (has_track, self.index) {
            (true, Some(index)) => {
                index + 1 < self.count
                    || matches!(self.modes.repeat, RepeatMode::All | RepeatMode::Album)
                    || self.modes.repeat.continues_from_library()
            }
            _ => false,
        };
        Properties {
            playback_status: playback_status(self.playback),
            loop_status: loop_status(self.modes.repeat),
            shuffle: self.modes.shuffle != ShuffleMode::Off,
            volume: self.volume,
            metadata: self.metadata(),
            can_go_next,
            // "Previous" restarts the first entry, so it always does something.
            can_go_previous: has_track,
            can_play: has_track || self.count > 0,
            can_pause: has_track,
            can_seek: has_track && self.length().is_some(),
        }
    }

    /// The published values, as the properties' getters return them.
    pub(crate) fn published(&self) -> &Properties {
        &self.published
    }

    /// The position now, interpolated while playing, in microseconds.
    pub(crate) fn position_micros(&self) -> i64 {
        micros(self.position_now(Instant::now()))
    }

    /// The `mpris:trackid` of the current entry.
    pub(crate) fn current_track_id(&self) -> Option<String> {
        self.current.as_ref().map(|c| track_id(c.entry))
    }

    /// The length of the current entry.
    pub(crate) fn length(&self) -> Option<Duration> {
        let current = self.current.as_ref()?;
        self.details_for(current.entry)
            .and_then(|d| d.length)
            .or(current.length)
    }

    /// Takes everything from a status and a queue: at the start, and after events
    /// that change more than they say.
    pub(crate) fn sync(&mut self, status: &PlayerStatus, queue: &QueueSnapshot, position: bool) {
        self.playback = status.playback;
        self.modes = status.modes;
        self.volume = f64::from(status.volume.position());
        self.count = queue.entries.len();
        self.index = queue.current;
        self.tracks = shown_entries(&queue.entries, queue.current).to_vec();
        self.playlist = match queue.context.source {
            ContextSource::Playlist(id) => Some(id),
            _ => None,
        };
        let current = status.now_playing.as_ref().map(|now| Current {
            entry: now.entry,
            track: Some(now.track.clone()),
            path: now.path.clone(),
            length: now.length.map(MediaTime::as_duration),
        });
        // The queue says what is current; the status may still name an entry that
        // was removed.
        let current = current.filter(|c| {
            queue
                .current_entry()
                .is_some_and(|entry| entry.id == c.entry)
        });
        if current.as_ref().map(|c| c.entry) != self.current.as_ref().map(|c| c.entry) {
            self.set_position(Duration::ZERO, Instant::now());
        }
        self.current = current;
        if position {
            self.set_position(status.position.as_duration(), Instant::now());
        }
    }

    /// Follows one event of the player.
    pub(crate) fn apply(&mut self, event: &PlayerEvent, now: Instant) -> Option<i64> {
        match event {
            PlayerEvent::State { playback, .. } => {
                // Anchor the interpolation where it is before the state changes.
                let position = self.position_now(now);
                self.playback = *playback;
                self.set_position(position, now);
                None
            }
            PlayerEvent::TrackChanged {
                index,
                count,
                entry,
                track,
                path,
                length,
                ..
            } => {
                self.index = Some(*index);
                self.count = *count;
                self.current = Some(Current {
                    entry: *entry,
                    track: Some(track.clone()),
                    path: path.clone(),
                    length: length.map(MediaTime::as_duration),
                });
                self.set_position(Duration::ZERO, now);
                // A jump to another track: clients reset their position.
                Some(0)
            }
            PlayerEvent::Position {
                index,
                position,
                length,
            } => {
                let position = position.as_duration();
                let expected = self.position_now(now);
                if let Some(current) = &mut self.current
                    && current.length.is_none()
                {
                    current.length = length.map(MediaTime::as_duration);
                }
                let same_entry = self.index == Some(*index);
                self.set_position(position, now);
                (same_entry && position.abs_diff(expected) > SEEK_TOLERANCE)
                    .then(|| micros(position))
            }
            PlayerEvent::ModesChanged(modes) => {
                self.modes = *modes;
                None
            }
            PlayerEvent::VolumeChanged(volume) => {
                self.volume = f64::from(volume.position());
                None
            }
            _ => None,
        }
    }

    /// Notes what the front end looked up for `entry`, with the URL of its
    /// exported cover.
    pub(crate) fn set_details(
        &mut self,
        entry: EntryId,
        details: TrackDetails,
        art_url: Option<String>,
    ) {
        self.details = Some((entry, details, art_url));
    }

    /// Changes the cover of `entry`'s details, if they are still the ones held.
    pub(crate) fn set_art_url(&mut self, entry: EntryId, art_url: Option<String>) -> bool {
        match &mut self.details {
            Some((held, _, url)) if *held == entry => {
                *url = art_url;
                true
            }
            _ => false,
        }
    }

    /// The entry the held details belong to.
    pub(crate) fn details_entry(&self) -> Option<EntryId> {
        self.details.as_ref().map(|(entry, ..)| *entry)
    }

    /// What a client set itself, shown at once (`Volume`, `LoopStatus`,
    /// `Shuffle`): the setter announces the value, so it counts as published.
    pub(crate) fn assume(&mut self, change: impl FnOnce(&mut Self)) {
        change(self);
        self.published = self.properties();
    }

    /// The entries the track list shows, as last announced.
    pub(crate) fn published_tracks(&self) -> &[EntryId] {
        &self.published_tracks
    }

    /// The shown entry with the `mpris:trackid` `id`.
    pub(crate) fn shown_entry(&self, id: &str) -> Option<&QueueEntry> {
        self.tracks.iter().find(|entry| track_id(entry.id) == id)
    }

    /// The playlist the queue came from, as last announced.
    pub(crate) fn active_playlist(&self) -> Option<u64> {
        self.published_playlist
    }

    /// The metadata of `entry` playing `track`: the published metadata for the
    /// current entry, otherwise what `details` say.
    pub(crate) fn entry_metadata(
        &self,
        entry: EntryId,
        track: &TrackRef,
        details: Option<&TrackDetails>,
    ) -> Metadata {
        if self.current.as_ref().is_some_and(|c| c.entry == entry) {
            return self.published.metadata.clone();
        }
        entry_metadata(entry, track, details)
    }

    /// How the track list changed since it was last announced.
    fn track_list_change(&self) -> Option<TrackListChange> {
        let old = &self.published_tracks;
        let new: Vec<EntryId> = self.tracks.iter().map(|e| e.id).collect();
        if *old == new {
            return None;
        }
        let ids = |list: &[EntryId]| list.iter().map(|id| track_id(*id)).collect::<Vec<_>>();
        let first_difference = old
            .iter()
            .zip(&new)
            .position(|(a, b)| a != b)
            .unwrap_or(old.len().min(new.len()));
        if new.len() == old.len() + 1 && new[first_difference + 1..] == old[first_difference..] {
            let after = first_difference
                .checked_sub(1)
                .map_or_else(|| NO_TRACK.to_owned(), |i| track_id(new[i]));
            let added = &self.tracks[first_difference];
            return Some(TrackListChange::Added {
                entry: added.id,
                track: added.track.clone(),
                after,
            });
        }
        if old.len() == new.len() + 1 && old[first_difference + 1..] == new[first_difference..] {
            return Some(TrackListChange::Removed(track_id(old[first_difference])));
        }
        Some(TrackListChange::Replaced {
            tracks: ids(&new),
            current: self
                .current
                .as_ref()
                .map_or_else(|| NO_TRACK.to_owned(), |c| track_id(c.entry)),
        })
    }

    /// What changed since the last announcement, now counted as announced.
    pub(crate) fn announce(&mut self, seeked: Option<i64>) -> Announcement {
        let tracks = self.track_list_change();
        self.published_tracks = self.tracks.iter().map(|e| e.id).collect();
        let active_playlist = (self.playlist != self.published_playlist).then_some(self.playlist);
        self.published_playlist = self.playlist;
        let now = self.properties();
        let old = &self.published;
        let same_track = now.metadata.get("mpris:trackid") == old.metadata.get("mpris:trackid");
        let track_metadata = match now.metadata.get("mpris:trackid") {
            Some(MetaValue::Path(id))
                if same_track && id != NO_TRACK && now.metadata != old.metadata =>
            {
                Some((id.clone(), now.metadata.clone()))
            }
            _ => None,
        };
        let mut changed = Vec::new();
        if now.playback_status != old.playback_status {
            changed.push(Changed::Text("PlaybackStatus", now.playback_status));
        }
        if now.loop_status != old.loop_status {
            changed.push(Changed::Text("LoopStatus", now.loop_status));
        }
        if now.shuffle != old.shuffle {
            changed.push(Changed::Flag("Shuffle", now.shuffle));
        }
        if now.volume.to_bits() != old.volume.to_bits() {
            changed.push(Changed::Real("Volume", now.volume));
        }
        if now.metadata != old.metadata {
            changed.push(Changed::Metadata(now.metadata.clone()));
        }
        let flags = [
            ("CanGoNext", now.can_go_next, old.can_go_next),
            ("CanGoPrevious", now.can_go_previous, old.can_go_previous),
            ("CanPlay", now.can_play, old.can_play),
            ("CanPause", now.can_pause, old.can_pause),
            ("CanSeek", now.can_seek, old.can_seek),
        ];
        for (name, new, old) in flags {
            if new != old {
                changed.push(Changed::Flag(name, new));
            }
        }
        self.published = now;
        Announcement {
            changed,
            seeked,
            tracks,
            track_metadata,
            active_playlist,
        }
    }

    fn details_for(&self, entry: EntryId) -> Option<&TrackDetails> {
        match &self.details {
            Some((held, details, _)) if *held == entry => Some(details),
            _ => None,
        }
    }

    fn position_now(&self, now: Instant) -> Duration {
        let position = if self.playback == PlaybackState::Playing {
            self.position + now.saturating_duration_since(self.position_at)
        } else {
            self.position
        };
        match self.length() {
            Some(length) => position.min(length),
            None => position,
        }
    }

    fn set_position(&mut self, position: Duration, now: Instant) {
        self.position = position;
        self.position_at = now;
    }

    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::new();
        let Some(current) = &self.current else {
            metadata.insert("mpris:trackid", MetaValue::Path(NO_TRACK.to_owned()));
            return metadata;
        };
        metadata.insert("mpris:trackid", MetaValue::Path(track_id(current.entry)));
        if let Some(length) = self.length() {
            metadata.insert("mpris:length", MetaValue::Long(micros(length)));
        }
        if let Some(url) = file_uri(&current.path) {
            metadata.insert("xesam:url", MetaValue::Text(url));
        }
        let (details, art_url) = match &self.details {
            Some((entry, details, art_url)) if *entry == current.entry => {
                (Some(details), art_url.as_ref())
            }
            _ => (None, None),
        };
        let title = details
            .and_then(|d| d.title.clone())
            .unwrap_or_else(|| file_title(&current.path));
        metadata.insert("xesam:title", MetaValue::Text(title));
        if let Some(url) = art_url {
            metadata.insert("mpris:artUrl", MetaValue::Text(url.clone()));
        }
        if let Some(details) = details {
            map_details(details, &mut metadata);
        }
        metadata
    }
}

/// The part of the queue the track list shows: all of it, or for a long queue
/// [`TRACK_LIST_LIMIT`] entries from a multiple of [`TRACK_LIST_STEP`] before
/// the current entry.
fn shown_entries(entries: &[QueueEntry], current: Option<usize>) -> &[QueueEntry] {
    if entries.len() <= TRACK_LIST_LIMIT {
        return entries;
    }
    let start = current.map_or(0, |c| c / TRACK_LIST_STEP * TRACK_LIST_STEP);
    let start = start.min(entries.len() - TRACK_LIST_LIMIT);
    &entries[start..start + TRACK_LIST_LIMIT]
}

/// The metadata of an entry that is not current, from what was looked up about
/// it (`details`) or else from its file.
pub(crate) fn entry_metadata(
    entry: EntryId,
    track: &TrackRef,
    details: Option<&TrackDetails>,
) -> Metadata {
    let mut metadata = Metadata::new();
    metadata.insert("mpris:trackid", MetaValue::Path(track_id(entry)));
    let path = match track {
        TrackRef::File(source) => Some(source.path().to_path_buf()),
        _ => details.and_then(|d| d.path.clone()),
    };
    if let Some(url) = path.as_deref().and_then(file_uri) {
        metadata.insert("xesam:url", MetaValue::Text(url));
    }
    let title = details
        .and_then(|d| d.title.clone())
        .or_else(|| path.as_deref().map(file_title));
    if let Some(title) = title {
        metadata.insert("xesam:title", MetaValue::Text(title));
    }
    if let Some(details) = details {
        if let Some(length) = details.length {
            metadata.insert("mpris:length", MetaValue::Long(micros(length)));
        }
        map_details(details, &mut metadata);
    }
    metadata
}

/// Adds the looked-up fields to `metadata`.
pub(crate) fn map_details(details: &TrackDetails, metadata: &mut Metadata) {
    let list = |values: &[String]| {
        let values: Vec<String> = values.iter().filter(|v| !v.is_empty()).cloned().collect();
        (!values.is_empty()).then_some(MetaValue::List(values))
    };
    let number = |n: Option<u32>| n.and_then(|n| i32::try_from(n).ok()).map(MetaValue::Int);
    let fields = [
        ("xesam:artist", list(&details.artists)),
        ("xesam:albumArtist", list(&details.album_artists)),
        ("xesam:composer", list(&details.composers)),
        ("xesam:genre", list(&details.genres)),
        (
            "xesam:album",
            details
                .album
                .clone()
                .filter(|a| !a.is_empty())
                .map(MetaValue::Text),
        ),
        ("xesam:trackNumber", number(details.track_number)),
        ("xesam:discNumber", number(details.disc_number)),
        (
            "xesam:contentCreated",
            details.year.map(|y| MetaValue::Text(format!("{y:04}"))),
        ),
        (
            "xesam:userRating",
            details
                .rating
                .map(|stars| MetaValue::Real(f64::from(stars.min(5)) / 5.0)),
        ),
    ];
    for (key, value) in fields {
        if let Some(value) = value {
            metadata.insert(key, value);
        }
    }
}

/// The title shown for a file without one: its name without the extension.
fn file_title(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn playback_status(state: PlaybackState) -> &'static str {
    match state {
        // Opening or seeking: audio follows at once, so the controls show "playing".
        PlaybackState::Playing | PlaybackState::Buffering => "Playing",
        PlaybackState::Paused => "Paused",
        PlaybackState::Stopped => "Stopped",
    }
}

/// `LoopStatus`: repeating an album is `Playlist`, because the queue holds it.
pub(crate) fn loop_status(repeat: RepeatMode) -> &'static str {
    match repeat {
        RepeatMode::One => "Track",
        RepeatMode::All | RepeatMode::Album => "Playlist",
        _ => "None",
    }
}

/// The repeat mode a client asks for with `LoopStatus`.
pub(crate) fn repeat_mode(loop_status: &str) -> Option<RepeatMode> {
    match loop_status {
        "None" => Some(RepeatMode::Off),
        "Track" => Some(RepeatMode::One),
        "Playlist" => Some(RepeatMode::All),
        _ => None,
    }
}

/// Microseconds, as MPRIS counts time.
pub(crate) fn micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

/// The play modes with only `repeat` or `shuffle` changed (for the setters).
pub(crate) fn with_repeat(modes: PlayModes, repeat: RepeatMode) -> PlayModes {
    PlayModes { repeat, ..modes }
}

/// See [`with_repeat`].
pub(crate) fn with_shuffle(modes: PlayModes, shuffle: bool) -> PlayModes {
    PlayModes {
        shuffle: if shuffle && modes.shuffle != ShuffleMode::Off {
            modes.shuffle
        } else if shuffle {
            ShuffleMode::Tracks
        } else {
            ShuffleMode::Off
        },
        ..modes
    }
}

impl State {
    /// The play modes, for the setters.
    pub(crate) fn modes(&self) -> PlayModes {
        self.modes
    }

    /// Sets the play modes as a client asked.
    pub(crate) fn set_modes(&mut self, modes: PlayModes) {
        self.modes = modes;
    }

    /// Sets the volume as a client asked.
    pub(crate) fn set_volume(&mut self, volume: f64) {
        self.volume = volume;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use helikodu_model::audio::{
        AudioSpec, ChannelLayout, Endianness, MediaTime, SampleFormat, SampleRate,
    };
    use helikodu_model::library::{MediaSource, TrackId};
    use helikodu_model::player::{QueueEntry, TransportState};

    use super::*;

    fn spec() -> AudioSpec {
        AudioSpec::new(
            SampleFormat::S16(Endianness::Little),
            SampleRate::HZ_44100,
            ChannelLayout::STEREO,
        )
    }

    fn track_changed(index: usize, count: usize, entry: u64) -> PlayerEvent {
        PlayerEvent::TrackChanged {
            index,
            count,
            entry: EntryId::new(entry),
            track: TrackRef::File(MediaSource::file(format!("/music/{entry}.flac"))),
            path: PathBuf::from(format!("/music/Song {entry}.flac")),
            spec: spec(),
            length: Some(MediaTime::from_millis(180_000)),
        }
    }

    fn state_event(playback: PlaybackState) -> PlayerEvent {
        PlayerEvent::State {
            playback,
            transport: TransportState::Playing,
        }
    }

    fn position(index: usize, millis: u64) -> PlayerEvent {
        PlayerEvent::Position {
            index,
            position: MediaTime::from_millis(millis),
            length: Some(MediaTime::from_millis(180_000)),
        }
    }

    #[test]
    fn a_long_queue_shows_the_part_around_the_current_entry() {
        let entries: Vec<QueueEntry> = (1..=2500)
            .map(|id| QueueEntry {
                id: EntryId::new(id),
                track: TrackRef::Library(TrackId::new(id)),
                from_context: true,
            })
            .collect();
        assert_eq!(shown_entries(&entries[..10], Some(3)).len(), 10);
        let shown = shown_entries(&entries, Some(1203));
        assert_eq!(shown.len(), TRACK_LIST_LIMIT);
        assert_eq!(shown[0].id, EntryId::new(1001));
        // Near the end the part ends with the queue.
        let shown = shown_entries(&entries, Some(2400));
        assert_eq!(shown[0].id, EntryId::new(1501));
        assert_eq!(shown_entries(&entries, None)[0].id, EntryId::new(1));
    }

    #[test]
    fn nothing_current_is_the_no_track_id() {
        let state = State::new();
        let props = state.properties();
        assert_eq!(
            props.metadata.get("mpris:trackid"),
            Some(&MetaValue::Path(NO_TRACK.into()))
        );
        assert_eq!(props.metadata.len(), 1);
        assert_eq!(props.playback_status, "Stopped");
        assert!(!props.can_play && !props.can_pause && !props.can_seek);
        assert!(!props.can_go_next && !props.can_go_previous);
    }

    #[test]
    fn a_track_maps_to_metadata_and_capabilities() {
        let mut state = State::new();
        let now = Instant::now();
        assert_eq!(state.apply(&track_changed(0, 2, 7), now), Some(0));
        state.apply(&state_event(PlaybackState::Playing), now);
        let props = state.properties();
        let metadata = &props.metadata;
        assert_eq!(
            metadata.get("mpris:trackid"),
            Some(&MetaValue::Path(
                "/io/github/sorrelfox/Helikodu/Track/7".into()
            ))
        );
        assert_eq!(
            metadata.get("mpris:length"),
            Some(&MetaValue::Long(180_000_000))
        );
        assert_eq!(
            metadata.get("xesam:title"),
            Some(&MetaValue::Text("Song 7".into()))
        );
        assert_eq!(
            metadata.get("xesam:url"),
            Some(&MetaValue::Text("file:///music/Song%207.flac".into()))
        );
        assert_eq!(props.playback_status, "Playing");
        assert!(props.can_go_next && props.can_go_previous);
        assert!(props.can_play && props.can_pause && props.can_seek);

        // The last entry of a queue that does not repeat has no next one.
        state.apply(&track_changed(1, 2, 8), now);
        assert!(!state.properties().can_go_next);
        state.apply(
            &PlayerEvent::ModesChanged(PlayModes::new(ShuffleMode::Tracks, RepeatMode::All)),
            now,
        );
        let props = state.properties();
        assert!(props.can_go_next);
        assert_eq!(props.loop_status, "Playlist");
        assert!(props.shuffle);
    }

    #[test]
    fn looked_up_details_fill_the_xesam_fields() {
        let mut state = State::new();
        state.apply(&track_changed(0, 1, 3), Instant::now());
        let details = TrackDetails {
            title: Some("Teardrop".into()),
            artists: vec!["Massive Attack".into(), String::new()],
            album: Some("Mezzanine".into()),
            album_artists: vec!["Massive Attack".into()],
            track_number: Some(3),
            disc_number: Some(1),
            genres: vec!["Trip hop".into()],
            composers: vec![],
            year: Some(1998),
            rating: Some(4),
            length: Some(Duration::from_millis(330_500)),
            ..TrackDetails::default()
        };
        state.set_details(
            EntryId::new(3),
            details,
            Some("file:///run/user/1000/helikodu/cover-ab.jpg".into()),
        );
        let metadata = state.properties().metadata;
        assert_eq!(
            metadata.get("xesam:title"),
            Some(&MetaValue::Text("Teardrop".into()))
        );
        assert_eq!(
            metadata.get("xesam:artist"),
            Some(&MetaValue::List(vec!["Massive Attack".into()]))
        );
        assert_eq!(
            metadata.get("xesam:album"),
            Some(&MetaValue::Text("Mezzanine".into()))
        );
        assert_eq!(metadata.get("xesam:trackNumber"), Some(&MetaValue::Int(3)));
        assert_eq!(metadata.get("xesam:discNumber"), Some(&MetaValue::Int(1)));
        assert_eq!(
            metadata.get("xesam:contentCreated"),
            Some(&MetaValue::Text("1998".into()))
        );
        assert_eq!(
            metadata.get("xesam:userRating"),
            Some(&MetaValue::Real(0.8))
        );
        assert_eq!(
            metadata.get("mpris:length"),
            Some(&MetaValue::Long(330_500_000))
        );
        assert_eq!(
            metadata.get("mpris:artUrl"),
            Some(&MetaValue::Text(
                "file:///run/user/1000/helikodu/cover-ab.jpg".into()
            ))
        );
        assert!(!metadata.contains_key("xesam:composer"));

        // Details of another entry are not shown.
        state.apply(&track_changed(0, 1, 4), Instant::now());
        let metadata = state.properties().metadata;
        assert_eq!(
            metadata.get("xesam:title"),
            Some(&MetaValue::Text("Song 4".into()))
        );
        assert!(!metadata.contains_key("xesam:album"));
    }

    #[test]
    fn only_changes_are_announced() {
        let mut state = State::new();
        let now = Instant::now();
        assert!(state.announce(None).is_empty());
        let seeked = state.apply(&track_changed(0, 3, 1), now);
        let first = state.announce(seeked);
        assert_eq!(first.seeked, Some(0));
        let names: Vec<_> = first.changed.iter().map(changed_name).collect();
        assert_eq!(
            names,
            [
                "Metadata",
                "CanGoNext",
                "CanGoPrevious",
                "CanPlay",
                "CanPause",
                "CanSeek"
            ]
        );
        assert!(state.announce(None).is_empty());

        state.apply(&state_event(PlaybackState::Playing), now);
        assert_eq!(
            state.announce(None).changed,
            [Changed::Text("PlaybackStatus", "Playing")]
        );
        // Buffering shows as playing: nothing to announce.
        state.apply(&state_event(PlaybackState::Buffering), now);
        assert!(state.announce(None).is_empty());

        state.apply(&PlayerEvent::VolumeChanged(Volume::clamped(0.5)), now);
        assert_eq!(state.announce(None).changed, [Changed::Real("Volume", 0.5)]);
        state.apply(&PlayerEvent::VolumeChanged(Volume::clamped(0.5)), now);
        assert!(state.announce(None).is_empty());
    }

    fn changed_name(changed: &Changed) -> &'static str {
        match changed {
            Changed::Text(name, _) | Changed::Flag(name, _) | Changed::Real(name, _) => name,
            Changed::Metadata(_) => "Metadata",
        }
    }

    #[test]
    fn seeks_are_told_from_positions_that_jump() {
        let mut state = State::new();
        let start = Instant::now();
        state.apply(&track_changed(0, 1, 1), start);
        state.apply(&state_event(PlaybackState::Playing), start);
        // Positions where interpolation puts them are no seeks.
        assert_eq!(
            state.apply(&position(0, 250), start + Duration::from_millis(250)),
            None
        );
        assert_eq!(
            state.apply(&position(0, 500), start + Duration::from_millis(500)),
            None
        );
        // A jump ahead is.
        assert_eq!(
            state.apply(&position(0, 60_000), start + Duration::from_millis(750)),
            Some(60_000_000)
        );
        // So is a jump back while paused.
        state.apply(
            &state_event(PlaybackState::Paused),
            start + Duration::from_millis(800),
        );
        assert_eq!(
            state.apply(&position(0, 10_000), start + Duration::from_secs(5)),
            Some(10_000_000)
        );
        // While paused the position stands still.
        assert_eq!(
            state
                .position_now(start + Duration::from_secs(9))
                .as_millis(),
            10_000
        );
    }

    #[test]
    fn sync_takes_the_status_and_queue() {
        use helikodu_service::player::NowPlaying;

        let entries: Vec<QueueEntry> = (1..=3)
            .map(|id| QueueEntry {
                id: EntryId::new(id),
                track: TrackRef::Library(TrackId::new(id)),
                from_context: true,
            })
            .collect();
        let queue = QueueSnapshot {
            version: 1,
            entries,
            current: Some(2),
            ..QueueSnapshot::default()
        };
        let status = PlayerStatus {
            playback: PlaybackState::Paused,
            now_playing: Some(NowPlaying {
                entry: EntryId::new(3),
                track: TrackRef::Library(TrackId::new(3)),
                path: "/music/c.ogg".into(),
                spec: spec(),
                length: None,
            }),
            position: MediaTime::from_millis(4_000),
            volume: Volume::clamped(0.25),
            ..PlayerStatus::default()
        };
        let mut state = State::new();
        state.sync(&status, &queue, true);
        let props = state.properties();
        assert_eq!(props.playback_status, "Paused");
        assert_eq!(props.volume, 0.25);
        assert!(!props.can_go_next);
        assert!(!props.can_seek, "the length is unknown");
        assert_eq!(state.position_micros(), 4_000_000);
        assert_eq!(
            state.current_track_id().as_deref(),
            Some("/io/github/sorrelfox/Helikodu/Track/3")
        );
    }

    #[test]
    fn loop_status_maps_both_ways() {
        for repeat in [RepeatMode::Off, RepeatMode::One, RepeatMode::All] {
            assert_eq!(repeat_mode(loop_status(repeat)), Some(repeat));
        }
        assert_eq!(repeat_mode("Album"), None);
    }
}
