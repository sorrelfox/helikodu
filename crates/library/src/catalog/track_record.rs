// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the scanner knows about a track when it writes it to the library.

use std::path::PathBuf;
use std::time::Duration;

use helikodu_meta::worker::CoverReport;

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
    /// What part of the file the track is: the whole file, a track of a CUE sheet or
    /// a chapter.
    pub section: TrackSection,
    /// The tags.
    pub tags: TrackTags,
    /// The tags of the album and artist model beyond [`TrackTags`]: credits, sort
    /// names, MusicBrainz identifiers, the compilation flag and the disc.
    pub catalog: CatalogTags,
    /// The rating found in the tags. It is taken only when the track first enters the
    /// library with a new identity; from then on the database wins.
    pub rating_from_tags: Option<Rating>,
    /// The loudness values of the tags.
    pub replay_gain: ReplayGain,
    /// The covers found for the file, or `None` when they were not looked for; then
    /// the covers stored for the track and its folder stay as they are.
    pub covers: Option<TrackCovers>,
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
            section: TrackSection::Whole,
            audio: AudioProperties::default(),
            tags: TrackTags::default(),
            catalog: CatalogTags::default(),
            rating_from_tags: None,
            replay_gain: ReplayGain::default(),
            covers: None,
        }
    }
}

/// The ReplayGain values of a track, relative to the ReplayGain 2.0 reference
/// (−18 LUFS). An Opus `R128_*_GAIN` tag is converted to it.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReplayGain {
    /// The track gain in dB.
    pub track_gain_db: Option<f64>,
    /// The track peak, linear (1.0 is full scale).
    pub track_peak: Option<f64>,
    /// The album gain in dB.
    pub album_gain_db: Option<f64>,
    /// The album peak, linear.
    pub album_peak: Option<f64>,
}

impl ReplayGain {
    fn bits(&self) -> [Option<u64>; 4] {
        [
            self.track_gain_db,
            self.track_peak,
            self.album_gain_db,
            self.album_peak,
        ]
        .map(|v| v.map(f64::to_bits))
    }
}

/// Values are equal when their bits are.
impl PartialEq for ReplayGain {
    fn eq(&self, other: &Self) -> bool {
        self.bits() == other.bits()
    }
}

impl Eq for ReplayGain {}

/// The covers found for a file: its embedded picture and the image file in its
/// folder. Either may be missing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackCovers {
    /// The picture embedded in the file that best serves as its cover.
    pub embedded: Option<CoverReport>,
    /// The image file in the file's folder, or in the folder above a disc folder.
    pub folder: Option<CoverReport>,
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

/// What part of its file a track is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TrackSection {
    /// The whole file, the default.
    #[default]
    Whole,
    /// A track of a CUE sheet, external or embedded in the file.
    CueSheet,
    /// A chapter of the container (MP4/M4B, Matroska, Ogg, ID3v2 `CHAP`).
    Chapter,
    /// A subsong of a tracker module, game music file, SID tune or sound bank: the
    /// subsong `sub_index - 1`.
    Subsong,
}

impl TrackSection {
    /// The value of `tracks.section`.
    pub(crate) fn to_sql(self) -> i64 {
        match self {
            Self::Whole => 0,
            Self::CueSheet => 1,
            Self::Chapter => 2,
            Self::Subsong => 3,
        }
    }

    /// The section stored as `value`; the whole file for a value this build does not
    /// know.
    pub(crate) fn from_sql(value: i64) -> Self {
        match value {
            1 => Self::CueSheet,
            2 => Self::Chapter,
            3 => Self::Subsong,
            _ => Self::Whole,
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

/// The tags of the album and artist model beyond [`TrackTags`]. Every field is
/// optional; without them a track is grouped by its album title, album artists or
/// artists, and year alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogTags {
    /// The track artist as credited, such as `A feat. B`. Without it, the artists
    /// joined with `, ` are shown.
    pub artist_credit: Option<String>,
    /// The track artist for sorting. It names the person when there is one artist.
    pub artist_sort: Option<String>,
    /// The album artist as credited, such as `A & B`. Without it, the album artists
    /// joined with `, `.
    pub album_artist_credit: Option<String>,
    /// The album artist for sorting.
    pub album_artist_sort: Option<String>,
    /// The album title for sorting.
    pub album_sort: Option<String>,
    /// The composer for sorting. It names the person when there is one composer.
    pub composer_sort: Option<String>,
    /// The people credited besides the artists, album artists and composers
    /// (conductors, performers, lyricists, …), in tag order within each role.
    pub credits: Vec<Credit>,
    /// The compilation flag (`COMPILATION`, `TCMP`, `cpil`), when the file has one.
    pub compilation: Option<bool>,
    /// The release types, such as `album` and `live`.
    pub release_types: Vec<String>,
    /// The MusicBrainz release ID. When present it identifies the album.
    pub release_mbid: Option<String>,
    /// The MusicBrainz release group ID.
    pub release_group_mbid: Option<String>,
    /// The MusicBrainz IDs of the track artists, in the order of
    /// [`TrackTags::artists`].
    pub artist_mbids: Vec<String>,
    /// The MusicBrainz IDs of the album artists, in the order of
    /// [`TrackTags::album_artists`].
    pub album_artist_mbids: Vec<String>,
    /// The number of tracks on the disc.
    pub track_total: Option<u32>,
    /// The number of discs.
    pub disc_total: Option<u32>,
    /// The disc's title.
    pub disc_subtitle: Option<String>,
    /// The year of the original release.
    pub original_year: Option<i32>,
    /// The classical work.
    pub work: Option<String>,
    /// The movement name.
    pub movement: Option<String>,
    /// The movement number.
    pub movement_no: Option<u32>,
    /// Whether the file carries lyrics.
    pub has_lyrics: bool,
}

/// One person credited on a track besides its artists, album artists and composers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
    /// What they did.
    pub role: ArtistRole,
    /// The name as tagged.
    pub name: String,
    /// The instrument or vocal part of a performer, such as `violin`.
    pub instrument: Option<String>,
}

/// What a person did on a track or an album.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ArtistRole {
    /// A track artist.
    Artist,
    /// An album artist. For albums, this is the album's artists as the library
    /// groups them, which may come from the track artists.
    AlbumArtist,
    /// A composer.
    Composer,
    /// A performer, possibly with an instrument.
    Performer,
    /// A conductor.
    Conductor,
    /// A lyricist.
    Lyricist,
    /// A writer of music and lyrics.
    Writer,
    /// An arranger.
    Arranger,
    /// A remixer.
    Remixer,
    /// A producer.
    Producer,
    /// An engineer.
    Engineer,
    /// A mixing engineer.
    Mixer,
    /// A DJ mixer.
    DjMixer,
}

impl ArtistRole {
    /// Every role, in the order of their stored values.
    pub const ALL: [Self; 13] = [
        Self::Artist,
        Self::AlbumArtist,
        Self::Composer,
        Self::Performer,
        Self::Conductor,
        Self::Lyricist,
        Self::Writer,
        Self::Arranger,
        Self::Remixer,
        Self::Producer,
        Self::Engineer,
        Self::Mixer,
        Self::DjMixer,
    ];

    /// The value of `track_people.role`.
    pub(crate) fn to_sql(self) -> i64 {
        match self {
            Self::Artist => 0,
            Self::AlbumArtist => 1,
            Self::Composer => 2,
            Self::Performer => 3,
            Self::Conductor => 4,
            Self::Lyricist => 5,
            Self::Writer => 6,
            Self::Arranger => 7,
            Self::Remixer => 8,
            Self::Producer => 9,
            Self::Engineer => 10,
            Self::Mixer => 11,
            Self::DjMixer => 12,
        }
    }
}
