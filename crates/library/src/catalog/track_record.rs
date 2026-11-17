// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the scanner knows about a track when it writes it to the library.

use std::path::PathBuf;
use std::time::Duration;

use super::ids::RootId;
use crate::stats::Rating;

/// A track as read from its file: where it is, what the file looked like, its audio
/// properties and its tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackRecord {
    /// The library folder the file is in.
    pub root: RootId,
    /// The file, as the bytes the kernel returned.
    pub path: PathBuf,
    /// The file's size, time stamps and quick hash.
    pub file: FileStamp,
    /// The CUE track or subsong number, or 0 for a whole file.
    pub sub_index: u32,
    /// The first frame, and the end frame if there is one, of a part of the file.
    pub frames: Option<(u64, Option<u64>)>,
    /// What kind of recording this is.
    pub kind: TrackKind,
    /// The audio properties.
    pub audio: AudioProperties,
    /// The tags.
    pub tags: TrackTags,
    /// The rating found in the tags. It is taken only when the track first enters the
    /// library with a new identity; from then on the database wins.
    pub rating_from_tags: Option<Rating>,
}

impl TrackRecord {
    /// A whole-file music track at `path` with no tags or audio properties yet.
    pub fn new(root: RootId, path: impl Into<PathBuf>, file: FileStamp) -> Self {
        Self {
            root,
            path: path.into(),
            file,
            sub_index: 0,
            frames: None,
            kind: TrackKind::Music,
            audio: AudioProperties::default(),
            tags: TrackTags::default(),
            rating_from_tags: None,
        }
    }
}

/// What the file looked like when it was read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileStamp {
    /// The size in bytes.
    pub size: u64,
    /// The modification time, in nanoseconds since the Unix epoch.
    pub mtime_ns: Option<i64>,
    /// The inode number.
    pub inode: Option<u64>,
    /// A hash of the size and the first and last 64 KiB, for move detection.
    pub quick_hash: Vec<u8>,
}

/// What kind of recording a track is. Statistics and resume positions treat them
/// differently.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TrackKind {
    /// Music, the default.
    #[default]
    Music,
    /// A chapter or part of an audiobook; it remembers where listening stopped.
    Audiobook,
    /// A podcast episode.
    Podcast,
    /// The audio of a video file.
    AudioFromVideo,
}

impl TrackKind {
    /// The value of `tracks.kind`.
    pub(crate) fn to_sql(self) -> i64 {
        match self {
            Self::Music => 0,
            Self::Audiobook => 1,
            Self::Podcast => 2,
            Self::AudioFromVideo => 3,
        }
    }

    /// The kind stored as `value`; music for a value this build does not know.
    pub(crate) fn from_sql(value: i64) -> Self {
        match value {
            1 => Self::Audiobook,
            2 => Self::Podcast,
            3 => Self::AudioFromVideo,
            _ => Self::Music,
        }
    }
}

/// Audio properties of a track, as the decoder reports them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioProperties {
    /// The length.
    pub duration: Option<Duration>,
    /// Frames per second.
    pub sample_rate: Option<u32>,
    /// The number of channels.
    pub channels: Option<u16>,
    /// Bits per sample of a lossless stream.
    pub bit_depth: Option<u16>,
    /// The average bit rate in bits per second.
    pub bitrate: Option<u32>,
    /// The codec, such as `flac` or `mp3`.
    pub codec: Option<String>,
    /// The container, such as `ogg` or `mp4`.
    pub container: Option<String>,
}

/// The tags the catalogue and search use. Lists keep the order of the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackTags {
    /// The title.
    pub title: Option<String>,
    /// The title to sort by, when the file has a sort tag.
    pub title_sort: Option<String>,
    /// The track artists.
    pub artists: Vec<String>,
    /// The album artists.
    pub album_artists: Vec<String>,
    /// The composers.
    pub composers: Vec<String>,
    /// The album title.
    pub album: Option<String>,
    /// The genres.
    pub genres: Vec<String>,
    /// The track number on its disc.
    pub track_no: Option<u32>,
    /// The disc number.
    pub disc_no: Option<u32>,
    /// The release date as written in the file.
    pub date: Option<String>,
    /// The release year.
    pub year: Option<i32>,
}
