// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The play queue as front ends see it, and the play modes.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::library::TrackRef;

/// Identifies one entry of the play queue. The same track queued twice has two
/// entries with two identifiers. Identifiers are not reused while the service runs.
///
/// Serialized as the plain number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryId(u64);

impl EntryId {
    /// The identifier with the number `id`.
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// One item of the play queue.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QueueEntry {
    /// The entry.
    pub id: EntryId,
    /// What it plays.
    pub track: TrackRef,
    /// Whether it came with the queue's context, rather than being queued from
    /// elsewhere with "Play next" or "Add to queue". The queue panel shows the
    /// track number for the first and the cover and artist for the second.
    pub from_context: bool,
}

/// Where the tracks of a queue came from, for display ("Continuing from …").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QueueContext {
    /// The source.
    pub source: ContextSource,
    /// Its name as shown, such as the album title; the service fills it in for the
    /// sources it can look up when it is not given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl QueueContext {
    /// A context from `source`, without a title.
    pub const fn new(source: ContextSource) -> Self {
        Self {
            source,
            title: None,
        }
    }

    /// Loose files, such as those named on the command line or dropped on the
    /// window.
    pub const fn files() -> Self {
        Self::new(ContextSource::Files)
    }

    /// The same context with the title `title`.
    #[must_use]
    pub fn with_title(self, title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            ..self
        }
    }
}

impl Default for QueueContext {
    fn default() -> Self {
        Self::files()
    }
}

/// The source of a queue. Identifiers are the library's numbers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSource {
    /// An album, in album order.
    Album(u64),
    /// The tracks of an artist.
    Artist(u64),
    /// The tracks of a genre.
    Genre(u64),
    /// A playlist.
    Playlist(u64),
    /// The results of a search.
    Query(String),
    /// A folder.
    Folder(
        #[serde(
            serialize_with = "crate::path_serde::serialize_bytes",
            deserialize_with = "crate::path_serde::deserialize"
        )]
        PathBuf,
    ),
    /// Loose files.
    Files,
}

/// Where "Play next" and "Add to queue" put tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnqueueAt {
    /// Right after the current entry: "Play next".
    Next,
    /// At the end: "Add to queue".
    End,
}

/// Whether the queue plays in a shuffled order.
///
/// Every shuffled order is drawn once and stored, so that "Previous" walks back the
/// order actually played, and every entry plays once per cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ShuffleMode {
    /// In queue order.
    #[default]
    Off,
    /// Every track once per cycle, in a random order.
    Tracks,
    /// Whole albums in a random order; the tracks of each album in album order.
    Albums,
    /// Whole folders in a random order; the tracks of each folder in queue order.
    Folders,
    /// Groupings (the grouping or work tag) in a random order, each in queue order;
    /// a track without one is a group of its own.
    Groupings,
    /// The albums in queue order, the tracks of each album in a random order.
    WithinAlbum,
}

impl ShuffleMode {
    /// Every mode, in the order the menus list them.
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Tracks,
        Self::Albums,
        Self::Folders,
        Self::Groupings,
        Self::WithinAlbum,
    ];
}

/// What happens at the end of a track or of the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RepeatMode {
    /// Playback ends after the last entry.
    #[default]
    Off,
    /// The current entry plays again and again. "Next" still moves on.
    One,
    /// After the last entry the queue starts again (in a new order when shuffled).
    All,
    /// The album of the current entry plays again and again: after its last track
    /// in play order, its first one follows.
    Album,
}

impl RepeatMode {
    /// Every mode, in the order the menus list them.
    pub const ALL: [Self; 4] = [Self::Off, Self::One, Self::Album, Self::All];
}

/// Whether playback stops at a point instead of going on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StopAfter {
    /// Playback goes on.
    #[default]
    Off,
    /// Playback stops when the current track ends; the mark then goes away.
    Track,
    /// Playback stops when the last track of the current album ends (the next entry
    /// belongs to another album, or there is none).
    Album,
    /// Playback stops at the end of the queue, also when repeating.
    Queue,
}

impl StopAfter {
    /// Every choice, in the order the menus list them.
    pub const ALL: [Self; 4] = [Self::Off, Self::Track, Self::Album, Self::Queue];
}

/// Which entries of the queue play ("Play only"). Entries outside the choice are
/// passed over, not removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PlayScope {
    /// Every entry.
    #[default]
    Everything,
    /// The entries of the current track's artist.
    Artist,
    /// The entries of the current track's album.
    Album,
}

impl PlayScope {
    /// Every choice, in the order the menus list them.
    pub const ALL: [Self; 3] = [Self::Everything, Self::Artist, Self::Album];
}

/// The play modes together. Fields added after the first version have defaults, so
/// older sessions still read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct PlayModes {
    /// Shuffle.
    pub shuffle: ShuffleMode,
    /// Repeat.
    pub repeat: RepeatMode,
    /// Consume: an entry leaves the queue once playback moves on from it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub consume: bool,
    /// Play only the entries of this scope.
    #[serde(default, skip_serializing_if = "PlayScope::is_everything")]
    pub scope: PlayScope,
    /// "Continue from the selected track": the entry selected in the queue (the
    /// player service's `SetCursor` command) plays after the current one, in place
    /// of the next in play order.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub follow_cursor: bool,
}

impl PlayModes {
    /// The modes with `shuffle` and `repeat`, the others off.
    pub const fn new(shuffle: ShuffleMode, repeat: RepeatMode) -> Self {
        Self {
            shuffle,
            repeat,
            consume: false,
            scope: PlayScope::Everything,
            follow_cursor: false,
        }
    }
}

impl PlayScope {
    /// Whether this is [`Everything`](Self::Everything).
    #[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
    pub fn is_everything(&self) -> bool {
        *self == Self::Everything
    }
}

/// What the player is doing, as the transport controls show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    /// Nothing plays.
    #[default]
    Stopped,
    /// Audio reaches the device.
    Playing,
    /// Held at a position: paused, or a restored session waiting for "Play".
    Paused,
    /// Opening, seeking or reopening the device; audio follows.
    Buffering,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{MediaSource, TrackId};
    use crate::path_serde::from_bytes;

    #[test]
    fn entries_round_trip_in_json() {
        let entries = vec![
            QueueEntry {
                id: EntryId::new(1),
                track: TrackRef::Library(TrackId::new(7)),
                from_context: true,
            },
            QueueEntry {
                id: EntryId::new(2),
                track: TrackRef::File(MediaSource::file(from_bytes(b"/m/Caf\xe9.flac".to_vec()))),
                from_context: false,
            },
        ];
        let json = serde_json::to_string(&entries).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<QueueEntry>>(&json).unwrap(),
            entries
        );
    }

    #[test]
    fn contexts_round_trip_in_json() {
        for context in [
            QueueContext::new(ContextSource::Album(4)).with_title("Water"),
            QueueContext::new(ContextSource::Query("love".into())),
            QueueContext::new(ContextSource::Folder(from_bytes(b"/m/\xff".to_vec()))),
            QueueContext::files(),
        ] {
            let json = serde_json::to_string(&context).unwrap();
            assert_eq!(
                serde_json::from_str::<QueueContext>(&json).unwrap(),
                context,
                "{json}"
            );
        }
    }

    #[test]
    fn modes_have_snake_case_names() {
        assert_eq!(
            serde_json::to_string(&PlayModes::new(ShuffleMode::Tracks, RepeatMode::All)).unwrap(),
            r#"{"shuffle":"tracks","repeat":"all"}"#
        );
        let modes = PlayModes {
            consume: true,
            scope: PlayScope::Artist,
            follow_cursor: true,
            ..PlayModes::new(ShuffleMode::WithinAlbum, RepeatMode::Album)
        };
        let json = serde_json::to_string(&modes).unwrap();
        assert_eq!(
            json,
            r#"{"shuffle":"within_album","repeat":"album","consume":true,"scope":"artist","follow_cursor":true}"#
        );
        assert_eq!(serde_json::from_str::<PlayModes>(&json).unwrap(), modes);
        assert_eq!(
            serde_json::to_string(&StopAfter::Queue).unwrap(),
            r#""queue""#
        );
        assert_eq!(
            serde_json::to_string(&StopAfter::Track).unwrap(),
            r#""track""#
        );
    }
}
