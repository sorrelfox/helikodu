// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The fields a query can test and sort by.

use serde::{Deserialize, Serialize};

/// A field of a track that a query can compare, test for presence and sort by.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Field {
    /// The title.
    Title,
    /// A track artist.
    Artist,
    /// An album artist, as the library groups albums.
    AlbumArtist,
    /// The album title.
    Album,
    /// A composer.
    Composer,
    /// A performer.
    Performer,
    /// A conductor.
    Conductor,
    /// A lyricist.
    Lyricist,
    /// A genre.
    Genre,
    /// The classical work.
    Work,
    /// The movement name.
    Movement,
    /// The disc's title.
    DiscSubtitle,
    /// The release type, such as `album` or `live`.
    ReleaseType,
    /// The codec, such as `flac`.
    Codec,
    /// The container, such as `ogg`.
    Container,
    /// The whole path of the file.
    Path,
    /// The folder of the file.
    Folder,
    /// The name of the file.
    Filename,
    /// What the track is: music, an audiobook, a podcast or audio from a video.
    Kind,
    /// The release year.
    Year,
    /// The year of the original release.
    OriginalYear,
    /// The track number on its disc.
    Track,
    /// The disc number.
    Disc,
    /// The movement number.
    MovementNumber,
    /// Frames per second, in Hz.
    SampleRate,
    /// Bits per sample.
    BitDepth,
    /// The number of channels.
    Channels,
    /// The average bit rate, in kbit/s.
    Bitrate,
    /// The ReplayGain track gain, in dB.
    TrackGain,
    /// The ReplayGain album gain, in dB.
    AlbumGain,
    /// How often the track was played.
    Plays,
    /// How often it was skipped.
    Skips,
    /// The rating, in stars from 0 to 5 (half stars allowed).
    Rating,
    /// The length.
    Duration,
    /// Where listening stopped, for long tracks and audiobooks.
    Resume,
    /// The size of the file.
    Size,
    /// When the track came into the library.
    Added,
    /// When its tags last changed.
    Modified,
    /// When it was last played.
    LastPlayed,
    /// When it was first played.
    FirstPlayed,
    /// When it was last skipped.
    LastSkipped,
    /// A label the user gave the track, its album or one of its artists. Labels
    /// live in the library, never in the files.
    Label,
    /// Any other tag, by its name in upper case (`tag:MOOD`).
    Tag(String),
    /// A field the user defined by an expression, by its name in lower case (see
    /// [`user_fields`](crate::user_fields)).
    Computed(String),
}

/// The kind of values a field holds, which decides the operators and values it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FieldKind {
    /// Text: contains, equals, differs and matches a regular expression.
    Text,
    /// The media kind: equals or differs.
    Kind,
    /// A number: every comparison and ranges.
    Number,
    /// A rating in stars, in steps of half a star.
    Rating,
    /// A length of time.
    Duration,
    /// A number of bytes.
    Size,
    /// A point in time: calendar dates and ages relative to now.
    Date,
}

/// Each named field with its canonical name and other spellings.
const NAMES: &[(Field, &str, &[&str])] = &[
    (Field::Title, "title", &[]),
    (Field::Artist, "artist", &[]),
    (Field::AlbumArtist, "albumartist", &["album_artist"]),
    (Field::Album, "album", &[]),
    (Field::Composer, "composer", &[]),
    (Field::Performer, "performer", &[]),
    (Field::Conductor, "conductor", &[]),
    (Field::Lyricist, "lyricist", &[]),
    (Field::Genre, "genre", &[]),
    (Field::Work, "work", &[]),
    (Field::Movement, "movement", &[]),
    (Field::DiscSubtitle, "discsubtitle", &["subtitle"]),
    (Field::ReleaseType, "releasetype", &[]),
    (Field::Codec, "codec", &["format"]),
    (Field::Container, "container", &[]),
    (Field::Path, "path", &[]),
    (Field::Folder, "folder", &["dir"]),
    (Field::Filename, "filename", &["file"]),
    (Field::Kind, "kind", &[]),
    (Field::Year, "year", &[]),
    (Field::OriginalYear, "originalyear", &["origyear"]),
    (Field::Track, "track", &["tracknumber", "trackno"]),
    (Field::Disc, "disc", &["discnumber", "discno"]),
    (Field::MovementNumber, "movementno", &["movementnumber"]),
    (Field::SampleRate, "samplerate", &["rate"]),
    (Field::BitDepth, "bitdepth", &["bits"]),
    (Field::Channels, "channels", &[]),
    (Field::Bitrate, "bitrate", &[]),
    (Field::TrackGain, "trackgain", &[]),
    (Field::AlbumGain, "albumgain", &[]),
    (Field::Plays, "plays", &["playcount"]),
    (Field::Skips, "skips", &["skipcount"]),
    (Field::Rating, "rating", &["stars"]),
    (Field::Duration, "duration", &["length"]),
    (Field::Resume, "resume", &[]),
    (Field::Size, "size", &["filesize"]),
    (Field::Added, "added", &[]),
    (Field::Modified, "modified", &[]),
    (Field::LastPlayed, "lastplayed", &["played"]),
    (Field::FirstPlayed, "firstplayed", &[]),
    (Field::LastSkipped, "lastskipped", &["skipped"]),
    (Field::Label, "label", &["labels"]),
];

/// The prefix of a tag field's name.
const TAG_PREFIX: &str = "tag:";

impl Field {
    /// Every field except [`Field::Tag`], in the order a completion list shows them.
    pub fn named() -> impl Iterator<Item = Field> {
        NAMES.iter().map(|(field, _, _)| field.clone())
    }

    /// A tag field for `name`, which is stored in upper case; `None` if the name is
    /// empty or has characters other than ASCII letters, digits, `_` and `-`.
    pub fn tag(name: &str) -> Option<Self> {
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        valid.then(|| Self::Tag(name.to_ascii_uppercase()))
    }

    /// The field named `name` (any case, canonical name or another spelling), a tag
    /// field for `tag:NAME`, or a field the user defined (see
    /// [`user_fields`](crate::user_fields)): a computed field, or the tag a custom
    /// field stands for.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::built_in(name).or_else(|| crate::user_fields::current().field(name))
    }

    /// The field named `name` among the fields every library has: a named field or
    /// `tag:NAME`, without the ones the user defined.
    pub fn built_in(name: &str) -> Option<Self> {
        if let Some(tag) = name
            .get(..TAG_PREFIX.len())
            .filter(|prefix| prefix.eq_ignore_ascii_case(TAG_PREFIX))
        {
            return Self::tag(&name[tag.len()..]);
        }
        NAMES
            .iter()
            .find(|(_, canonical, other)| {
                canonical.eq_ignore_ascii_case(name)
                    || other.iter().any(|alias| alias.eq_ignore_ascii_case(name))
            })
            .map(|(field, _, _)| field.clone())
    }

    /// The canonical name, as the text form writes it: `albumartist`, `tag:MOOD`.
    pub fn name(&self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Tag(tag) => format!("{TAG_PREFIX}{tag}").into(),
            Self::Computed(name) => name.clone().into(),
            field => NAMES
                .iter()
                .find(|(named, _, _)| named == field)
                .map_or("", |(_, name, _)| name)
                .into(),
        }
    }

    /// The kind of values the field holds.
    pub fn kind(&self) -> FieldKind {
        match self {
            Self::Title
            | Self::Artist
            | Self::AlbumArtist
            | Self::Album
            | Self::Composer
            | Self::Performer
            | Self::Conductor
            | Self::Lyricist
            | Self::Genre
            | Self::Work
            | Self::Movement
            | Self::DiscSubtitle
            | Self::ReleaseType
            | Self::Codec
            | Self::Container
            | Self::Path
            | Self::Folder
            | Self::Filename
            | Self::Label
            | Self::Tag(_)
            | Self::Computed(_) => FieldKind::Text,
            Self::Kind => FieldKind::Kind,
            Self::Year
            | Self::OriginalYear
            | Self::Track
            | Self::Disc
            | Self::MovementNumber
            | Self::SampleRate
            | Self::BitDepth
            | Self::Channels
            | Self::Bitrate
            | Self::TrackGain
            | Self::AlbumGain
            | Self::Plays
            | Self::Skips => FieldKind::Number,
            Self::Rating => FieldKind::Rating,
            Self::Duration | Self::Resume => FieldKind::Duration,
            Self::Size => FieldKind::Size,
            Self::Added
            | Self::Modified
            | Self::LastPlayed
            | Self::FirstPlayed
            | Self::LastSkipped => FieldKind::Date,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_aliases_resolve() {
        for field in Field::named() {
            assert_eq!(Field::from_name(&field.name()), Some(field.clone()));
            assert_eq!(
                Field::from_name(&field.name().to_uppercase()),
                Some(field.clone())
            );
        }
        assert_eq!(Field::from_name("length"), Some(Field::Duration));
        assert_eq!(
            Field::from_name("Tag:mood"),
            Some(Field::Tag("MOOD".into()))
        );
        assert_eq!(Field::from_name("tag:"), None);
        assert_eq!(Field::from_name("tag:a b"), None);
        assert_eq!(Field::from_name("nonsense"), None);
        assert_eq!(Field::Tag("MOOD".into()).name(), "tag:MOOD");
    }

    #[test]
    fn every_named_field_has_a_distinct_name() {
        let mut names: Vec<String> = Field::named().map(|f| f.name().into_owned()).collect();
        let count = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), count);
        assert!(names.iter().all(|name| !name.is_empty()));
    }
}
