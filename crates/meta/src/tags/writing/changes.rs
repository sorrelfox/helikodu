// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a tag edit changes: fields set or cleared, the front cover, the lyrics and
//! the rating.

use std::collections::BTreeMap;

use crate::covers::{CoverLimits, PictureFormat};
use crate::tags::builder::parse_number;
use crate::tags::field::Field;
use crate::tags::gain::parse_r128;
use crate::tags::mapping;
use crate::tags::rating::Rating;

use super::WriteError;

/// A field the tag editor can write, named after MusicBrainz Picard's tag names.
///
/// Each format writes it under the key Picard uses (see the README's table); keys
/// of other taggers that mean the same field are removed when it is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(missing_docs)] // The names say what they are; `picard_name` gives the tag name.
pub enum TagField {
    Title,
    TitleSort,
    Subtitle,
    /// The track artist as credited, such as `A feat. B`.
    Artist,
    /// The individual track artists.
    Artists,
    ArtistSort,
    Album,
    AlbumSort,
    /// The album artist as credited.
    AlbumArtist,
    /// The individual album artists.
    AlbumArtists,
    AlbumArtistSort,
    TrackNumber,
    TrackTotal,
    DiscNumber,
    DiscTotal,
    DiscSubtitle,
    /// The release date: `YYYY`, `YYYY-MM` or `YYYY-MM-DD`.
    Date,
    OriginalDate,
    ReleaseDate,
    Genre,
    Composer,
    ComposerSort,
    Conductor,
    Lyricist,
    Writer,
    Arranger,
    Remixer,
    Producer,
    Engineer,
    Mixer,
    DjMixer,
    /// A performer, as `Name (instrument)` or just `Name`.
    Performer,
    Label,
    CatalogNumber,
    Barcode,
    Isrc,
    Asin,
    ReleaseType,
    ReleaseStatus,
    ReleaseCountry,
    Media,
    Script,
    Language,
    MusicBrainzRecordingId,
    MusicBrainzTrackId,
    MusicBrainzAlbumId,
    MusicBrainzArtistId,
    MusicBrainzAlbumArtistId,
    MusicBrainzReleaseGroupId,
    MusicBrainzWorkId,
    MusicBrainzDiscId,
    AcoustidId,
    Comment,
    /// `1` for a compilation, `0` for not one.
    Compilation,
    Grouping,
    Work,
    Movement,
    MovementNumber,
    MovementTotal,
    ShowMovement,
    Bpm,
    Key,
    Mood,
    EncodedBy,
    EncoderSettings,
    Copyright,
    License,
    Website,
    Director,
    OriginalAlbum,
    OriginalArtist,
    ReplayGainTrackGain,
    ReplayGainTrackPeak,
    ReplayGainAlbumGain,
    ReplayGainAlbumPeak,
    /// The Opus track gain (`R128_TRACK_GAIN`): a Q7.8 integer in 1/256 dB relative
    /// to −23 LUFS, as RFC 7845 defines it. Only Opus files carry it.
    R128TrackGain,
    /// The Opus album gain (`R128_ALBUM_GAIN`), in the same form.
    R128AlbumGain,
}

impl TagField {
    /// Every field, in the order of the list above.
    pub const ALL: &'static [TagField] = &[
        TagField::Title,
        TagField::TitleSort,
        TagField::Subtitle,
        TagField::Artist,
        TagField::Artists,
        TagField::ArtistSort,
        TagField::Album,
        TagField::AlbumSort,
        TagField::AlbumArtist,
        TagField::AlbumArtists,
        TagField::AlbumArtistSort,
        TagField::TrackNumber,
        TagField::TrackTotal,
        TagField::DiscNumber,
        TagField::DiscTotal,
        TagField::DiscSubtitle,
        TagField::Date,
        TagField::OriginalDate,
        TagField::ReleaseDate,
        TagField::Genre,
        TagField::Composer,
        TagField::ComposerSort,
        TagField::Conductor,
        TagField::Lyricist,
        TagField::Writer,
        TagField::Arranger,
        TagField::Remixer,
        TagField::Producer,
        TagField::Engineer,
        TagField::Mixer,
        TagField::DjMixer,
        TagField::Performer,
        TagField::Label,
        TagField::CatalogNumber,
        TagField::Barcode,
        TagField::Isrc,
        TagField::Asin,
        TagField::ReleaseType,
        TagField::ReleaseStatus,
        TagField::ReleaseCountry,
        TagField::Media,
        TagField::Script,
        TagField::Language,
        TagField::MusicBrainzRecordingId,
        TagField::MusicBrainzTrackId,
        TagField::MusicBrainzAlbumId,
        TagField::MusicBrainzArtistId,
        TagField::MusicBrainzAlbumArtistId,
        TagField::MusicBrainzReleaseGroupId,
        TagField::MusicBrainzWorkId,
        TagField::MusicBrainzDiscId,
        TagField::AcoustidId,
        TagField::Comment,
        TagField::Compilation,
        TagField::Grouping,
        TagField::Work,
        TagField::Movement,
        TagField::MovementNumber,
        TagField::MovementTotal,
        TagField::ShowMovement,
        TagField::Bpm,
        TagField::Key,
        TagField::Mood,
        TagField::EncodedBy,
        TagField::EncoderSettings,
        TagField::Copyright,
        TagField::License,
        TagField::Website,
        TagField::Director,
        TagField::OriginalAlbum,
        TagField::OriginalArtist,
        TagField::ReplayGainTrackGain,
        TagField::ReplayGainTrackPeak,
        TagField::ReplayGainAlbumGain,
        TagField::ReplayGainAlbumPeak,
        TagField::R128TrackGain,
        TagField::R128AlbumGain,
    ];

    /// The field of the mapping tables this field is read as.
    pub(crate) fn field(self) -> Field {
        match self {
            TagField::Title => Field::Title,
            TagField::TitleSort => Field::TitleSort,
            TagField::Subtitle => Field::Subtitle,
            TagField::Artist => Field::Artist,
            TagField::Artists => Field::Artists,
            TagField::ArtistSort => Field::ArtistSort,
            TagField::Album => Field::Album,
            TagField::AlbumSort => Field::AlbumSort,
            TagField::AlbumArtist => Field::AlbumArtist,
            TagField::AlbumArtists => Field::AlbumArtists,
            TagField::AlbumArtistSort => Field::AlbumArtistSort,
            TagField::TrackNumber => Field::TrackNumber,
            TagField::TrackTotal => Field::TotalTracks,
            TagField::DiscNumber => Field::DiscNumber,
            TagField::DiscTotal => Field::TotalDiscs,
            TagField::DiscSubtitle => Field::DiscSubtitle,
            TagField::Date => Field::Date,
            TagField::OriginalDate => Field::OriginalDate,
            TagField::ReleaseDate => Field::ReleaseDate,
            TagField::Genre => Field::Genre,
            TagField::Composer => Field::Composer,
            TagField::ComposerSort => Field::ComposerSort,
            TagField::Conductor => Field::Conductor,
            TagField::Lyricist => Field::Lyricist,
            TagField::Writer => Field::Writer,
            TagField::Arranger => Field::Arranger,
            TagField::Remixer => Field::Remixer,
            TagField::Producer => Field::Producer,
            TagField::Engineer => Field::Engineer,
            TagField::Mixer => Field::Mixer,
            TagField::DjMixer => Field::DjMixer,
            TagField::Performer => Field::Performer,
            TagField::Label => Field::Label,
            TagField::CatalogNumber => Field::CatalogNumber,
            TagField::Barcode => Field::Barcode,
            TagField::Isrc => Field::Isrc,
            TagField::Asin => Field::Asin,
            TagField::ReleaseType => Field::ReleaseType,
            TagField::ReleaseStatus => Field::ReleaseStatus,
            TagField::ReleaseCountry => Field::ReleaseCountry,
            TagField::Media => Field::Media,
            TagField::Script => Field::Script,
            TagField::Language => Field::Language,
            TagField::MusicBrainzRecordingId => Field::MbRecordingId,
            TagField::MusicBrainzTrackId => Field::MbTrackId,
            TagField::MusicBrainzAlbumId => Field::MbAlbumId,
            TagField::MusicBrainzArtistId => Field::MbArtistId,
            TagField::MusicBrainzAlbumArtistId => Field::MbAlbumArtistId,
            TagField::MusicBrainzReleaseGroupId => Field::MbReleaseGroupId,
            TagField::MusicBrainzWorkId => Field::MbWorkId,
            TagField::MusicBrainzDiscId => Field::MbDiscId,
            TagField::AcoustidId => Field::AcoustidId,
            TagField::Comment => Field::Comment,
            TagField::Compilation => Field::Compilation,
            TagField::Grouping => Field::Grouping,
            TagField::Work => Field::Work,
            TagField::Movement => Field::Movement,
            TagField::MovementNumber => Field::MovementNumber,
            TagField::MovementTotal => Field::MovementTotal,
            TagField::ShowMovement => Field::ShowMovement,
            TagField::Bpm => Field::Bpm,
            TagField::Key => Field::Key,
            TagField::Mood => Field::Mood,
            TagField::EncodedBy => Field::EncodedBy,
            TagField::EncoderSettings => Field::EncoderSettings,
            TagField::Copyright => Field::Copyright,
            TagField::License => Field::License,
            TagField::Website => Field::Website,
            TagField::Director => Field::Director,
            TagField::OriginalAlbum => Field::OriginalAlbum,
            TagField::OriginalArtist => Field::OriginalArtist,
            TagField::ReplayGainTrackGain => Field::ReplayGainTrackGain,
            TagField::ReplayGainTrackPeak => Field::ReplayGainTrackPeak,
            TagField::ReplayGainAlbumGain => Field::ReplayGainAlbumGain,
            TagField::ReplayGainAlbumPeak => Field::ReplayGainAlbumPeak,
            TagField::R128TrackGain => Field::R128TrackGain,
            TagField::R128AlbumGain => Field::R128AlbumGain,
        }
    }

    /// The field that a key of the mapping tables means, if the editor writes it.
    #[cfg(test)]
    pub(crate) fn from_field(field: Field) -> Option<TagField> {
        TagField::ALL.iter().copied().find(|f| f.field() == field)
    }

    /// Picard's internal name: `title`, `albumartist`, `musicbrainz_albumid`, …
    pub fn picard_name(self) -> &'static str {
        self.field().picard_name()
    }

    /// The field with Picard's internal name `name`, compared without regard to ASCII case.
    pub fn from_picard_name(name: &str) -> Option<TagField> {
        TagField::ALL
            .iter()
            .copied()
            .find(|f| f.picard_name().eq_ignore_ascii_case(name))
    }

    /// Whether the field holds several values.
    pub fn is_multi_valued(self) -> bool {
        matches!(
            self,
            TagField::Artist
                | TagField::Artists
                | TagField::AlbumArtist
                | TagField::AlbumArtists
                | TagField::Genre
                | TagField::Composer
                | TagField::Conductor
                | TagField::Lyricist
                | TagField::Writer
                | TagField::Arranger
                | TagField::Remixer
                | TagField::Producer
                | TagField::Engineer
                | TagField::Mixer
                | TagField::DjMixer
                | TagField::Performer
                | TagField::Label
                | TagField::CatalogNumber
                | TagField::ReleaseType
                | TagField::MusicBrainzArtistId
                | TagField::MusicBrainzAlbumArtistId
                | TagField::MusicBrainzWorkId
        )
    }

    /// Whether the value must be a positive whole number.
    pub fn is_number(self) -> bool {
        matches!(
            self,
            TagField::TrackNumber
                | TagField::TrackTotal
                | TagField::DiscNumber
                | TagField::DiscTotal
                | TagField::MovementNumber
                | TagField::MovementTotal
                | TagField::Bpm
        )
    }

    /// Whether the field is an Opus R128 gain, which only Opus files carry.
    pub fn is_r128(self) -> bool {
        matches!(self, TagField::R128TrackGain | TagField::R128AlbumGain)
    }

    /// Whether the value is a date (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`, possibly with a time).
    pub fn is_date(self) -> bool {
        matches!(
            self,
            TagField::Date | TagField::OriginalDate | TagField::ReleaseDate
        )
    }
}

/// A field to change: a known one, or another tag under its own name ("Add field").
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FieldKey {
    /// A field the editor knows.
    Known(TagField),
    /// Another field, by its key: a Vorbis comment or APEv2 key, an ID3v2 `TXXX`
    /// description, an MP4 freeform name. Compared without regard to ASCII case
    /// when an existing field is replaced or removed.
    Custom(String),
}

impl From<TagField> for FieldKey {
    fn from(field: TagField) -> Self {
        FieldKey::Known(field)
    }
}

impl FieldKey {
    /// A key typed by the user: a known field by its Picard name, else a custom one.
    pub fn parse(name: &str) -> FieldKey {
        let name = name.trim();
        match TagField::from_picard_name(name) {
            Some(field) => FieldKey::Known(field),
            None => FieldKey::Custom(name.to_owned()),
        }
    }

    /// The name shown for the key: Picard's name, or the custom key.
    pub fn name(&self) -> &str {
        match self {
            FieldKey::Known(field) => field.picard_name(),
            FieldKey::Custom(key) => key,
        }
    }

    fn is_multi_valued(&self) -> bool {
        match self {
            FieldKey::Known(field) => field.is_multi_valued(),
            FieldKey::Custom(_) => true,
        }
    }
}

/// The change of one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldChange {
    /// The field gets these values (one for a single-valued field), replacing what
    /// it had. Values are trimmed and empty ones dropped; no value left clears it.
    Set(Vec<String>),
    /// The field is removed.
    Clear,
}

/// The change of the front cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverChange {
    /// Embed this JPEG or PNG picture as the front cover, replacing the front
    /// cover the file has (in MP4, every picture: `covr` has no picture types).
    SetFront(Vec<u8>),
    /// Remove the front cover (in MP4, every picture). Other pictures stay.
    RemoveFront,
    /// Remove every embedded picture.
    RemoveAll,
}

/// The changes to write into one file (or into each file of a selection).
///
/// Fields not named are left as they are, as is everything the editor does not
/// know: other frames, other keys, other pictures.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagChanges {
    fields: BTreeMap<FieldKey, FieldChange>,
    /// The front cover, when it changes.
    pub cover: Option<CoverChange>,
    /// The unsynchronized lyrics, when they change: `Some(None)` removes the lyrics
    /// (synchronized ones too).
    pub lyrics: Option<Option<String>>,
    /// The rating, when it changes: `Some(None)` removes it. Written as described
    /// in the [`rating`](crate::tags::rating) module (`POPM`, `FMPS_RATING`, `RATING`).
    pub rating: Option<Option<Rating>>,
}

impl TagChanges {
    /// No changes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `key` to `values`, replacing an earlier change of it.
    pub fn set<I, S>(&mut self, key: impl Into<FieldKey>, values: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let values = values.into_iter().map(Into::into).collect();
        self.fields.insert(key.into(), FieldChange::Set(values));
        self
    }

    /// Sets `key` to the single value `value`.
    pub fn set_one(&mut self, key: impl Into<FieldKey>, value: impl Into<String>) -> &mut Self {
        self.set(key, [value.into()])
    }

    /// Removes `key` from the file.
    pub fn clear(&mut self, key: impl Into<FieldKey>) -> &mut Self {
        self.fields.insert(key.into(), FieldChange::Clear);
        self
    }

    /// Applies `change` to `key`.
    pub fn change(&mut self, key: impl Into<FieldKey>, change: FieldChange) -> &mut Self {
        self.fields.insert(key.into(), change);
        self
    }

    /// Leaves `key` as it is in each file again.
    pub fn keep(&mut self, key: &FieldKey) -> &mut Self {
        self.fields.remove(key);
        self
    }

    /// The field changes, by key.
    pub fn fields(&self) -> impl Iterator<Item = (&FieldKey, &FieldChange)> {
        self.fields.iter()
    }

    /// The change of `key`, if it changes.
    pub fn field(&self, key: &FieldKey) -> Option<&FieldChange> {
        self.fields.get(key)
    }

    /// Whether nothing changes.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
            && self.cover.is_none()
            && self.lyrics.is_none()
            && self.rating.is_none()
    }

    /// The changes in their checked form: values trimmed, empty sets turned into
    /// clears, numbers, dates, flags and the cover checked.
    pub(crate) fn normalized(&self) -> Result<Normalized, WriteError> {
        let mut fields = BTreeMap::new();
        for (key, change) in &self.fields {
            let values = match change {
                FieldChange::Clear => Vec::new(),
                FieldChange::Set(values) => values
                    .iter()
                    .map(|v| v.trim_matches(|c: char| c.is_whitespace() || c == '\0'))
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
                    .collect(),
            };
            check_values(key, &values)?;
            fields.insert(key.clone(), values);
        }
        if let Some(CoverChange::SetFront(bytes)) = &self.cover {
            check_cover(bytes)?;
        }
        let lyrics = self.lyrics.as_ref().map(|lyrics| {
            lyrics
                .as_deref()
                .map(|text| text.replace("\r\n", "\n").trim_end().to_owned())
                .filter(|text| !text.trim().is_empty())
        });
        Ok(Normalized {
            fields,
            cover: self.cover.clone(),
            lyrics,
            rating: self.rating,
        })
    }
}

/// Checked changes: each field's new values, empty to clear it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Normalized {
    pub(crate) fields: BTreeMap<FieldKey, Vec<String>>,
    pub(crate) cover: Option<CoverChange>,
    pub(crate) lyrics: Option<Option<String>>,
    pub(crate) rating: Option<Option<Rating>>,
}

impl Normalized {
    /// The new values of a known field, if it changes.
    pub(crate) fn known(&self, field: TagField) -> Option<&[String]> {
        self.fields.get(&FieldKey::Known(field)).map(Vec::as_slice)
    }

    /// The new single number of a known field: `Some(None)` when it is cleared.
    pub(crate) fn number(&self, field: TagField) -> Option<Option<u32>> {
        self.known(field)
            .map(|values| values.first().and_then(|v| parse_number(v)))
    }
}

fn invalid(key: &FieldKey, value: &str, reason: &'static str) -> WriteError {
    WriteError::InvalidValue {
        field: key.name().to_owned(),
        value: value.to_owned(),
        reason,
    }
}

fn check_values(key: &FieldKey, values: &[String]) -> Result<(), WriteError> {
    if let FieldKey::Custom(name) = key
        && !valid_custom_key(name)
    {
        return Err(invalid(
            key,
            name,
            "a field name is 2 to 255 printable ASCII characters without `=` or `~`",
        ));
    }
    // A key the mapping tables know is read as its field (or not kept at all), so
    // it would not read back as the custom field written.
    if let FieldKey::Custom(name) = key
        && (mapping::ape::field(name).is_some() || mapping::mp4::freeform_field(name).is_some())
    {
        return Err(invalid(
            key,
            name,
            "the name belongs to a known field or to one that is not kept",
        ));
    }
    if values.len() > 1 && !key.is_multi_valued() {
        return Err(invalid(
            key,
            &values.join("; "),
            "the field takes one value",
        ));
    }
    let FieldKey::Known(field) = key else {
        return Ok(());
    };
    for value in values {
        if field.is_number() && !is_positive_number(value) {
            return Err(invalid(key, value, "not a whole number above zero"));
        }
        if field.is_date() && !is_date(value) {
            return Err(invalid(
                key,
                value,
                "not a date such as 2024, 2024-05 or 2024-05-17",
            ));
        }
        if matches!(field, TagField::Compilation | TagField::ShowMovement)
            && !matches!(value.as_str(), "0" | "1")
        {
            return Err(invalid(key, value, "the field takes 1 or 0"));
        }
        if field.is_r128() && parse_r128(value).is_none() {
            return Err(invalid(
                key,
                value,
                "not a whole number from -32768 to 32767 (1/256 dB)",
            ));
        }
    }
    Ok(())
}

/// A custom key every tag format can carry: APEv2 is the strictest (2 to 255
/// characters from 0x20 to 0x7E), and Vorbis comment names have no `=` (nor `~`,
/// which some readers reserve).
fn valid_custom_key(key: &str) -> bool {
    (2..=255).contains(&key.len())
        && key.bytes().all(|b| (0x20..=0x7e).contains(&b))
        && !key.contains(['=', '~'])
        && key.trim() == key
}

fn is_positive_number(value: &str) -> bool {
    value.bytes().all(|b| b.is_ascii_digit()) && value.parse::<u32>().is_ok_and(|n| n > 0)
}

/// `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, optionally followed by `T` and a time
/// `HH`, `HH:MM` or `HH:MM:SS` (the ID3v2.4 timestamp form every format can hold).
pub(crate) fn is_date(value: &str) -> bool {
    let (date, time) = match value.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (value, None),
    };
    let parts: Vec<&str> = date.split('-').collect();
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    let in_range = |s: &str, max: u32| s.parse::<u32>().is_ok_and(|v| (1..=max).contains(&v));
    let date_ok = match parts.as_slice() {
        [y] => digits(y, 4),
        [y, m] => digits(y, 4) && digits(m, 2) && in_range(m, 12),
        [y, m, d] => {
            digits(y, 4) && digits(m, 2) && in_range(m, 12) && digits(d, 2) && in_range(d, 31)
        }
        _ => false,
    };
    let time_ok = time.is_none_or(|time| {
        parts.len() == 3
            && time.split(':').count() <= 3
            && time
                .split(':')
                .zip([23, 59, 59])
                .all(|(part, max)| digits(part, 2) && part.parse::<u32>().is_ok_and(|v| v <= max))
    });
    date_ok && time_ok
}

fn check_cover(bytes: &[u8]) -> Result<(), WriteError> {
    if bytes.len() > CoverLimits::DEFAULT_MAX_BYTES {
        return Err(WriteError::InvalidCover(
            "the picture is larger than 32 MiB",
        ));
    }
    match PictureFormat::sniff(bytes) {
        Some(PictureFormat::Jpeg | PictureFormat::Png) => Ok(()),
        _ => Err(WriteError::InvalidCover(
            "only JPEG and PNG pictures can be embedded",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_maps_to_a_distinct_table_field() {
        for (i, a) in TagField::ALL.iter().enumerate() {
            assert_eq!(TagField::from_field(a.field()), Some(*a));
            for b in &TagField::ALL[i + 1..] {
                assert_ne!(a.field(), b.field());
            }
        }
    }

    #[test]
    fn dates() {
        for ok in [
            "2024",
            "2024-05",
            "2024-05-17",
            "2024-05-17T10",
            "2024-05-17T10:20:30",
        ] {
            assert!(is_date(ok), "{ok}");
        }
        for bad in [
            "24",
            "2024-5",
            "2024-13",
            "2024-05-32",
            "2024T10",
            "May 2024",
            "2024-05-17T25",
        ] {
            assert!(!is_date(bad), "{bad}");
        }
    }

    #[test]
    fn values_are_checked() {
        let mut c = TagChanges::new();
        c.set_one(TagField::TrackNumber, "3");
        c.set(TagField::Genre, ["Rock", " ", "Pop "]);
        c.set(TagField::Title, ["  "]);
        let n = c.normalized().unwrap_or_default();
        assert_eq!(
            n.known(TagField::Genre),
            Some(&["Rock".to_owned(), "Pop".to_owned()][..])
        );
        assert_eq!(n.known(TagField::Title), Some(&[][..]));
        assert_eq!(n.number(TagField::TrackNumber), Some(Some(3)));

        for (field, value) in [
            (TagField::TrackNumber, "3/12"),
            (TagField::Bpm, "0"),
            (TagField::Date, "yesterday"),
            (TagField::Compilation, "yes"),
        ] {
            let mut c = TagChanges::new();
            c.set_one(field, value);
            assert!(
                matches!(c.normalized(), Err(WriteError::InvalidValue { .. })),
                "{value}"
            );
        }
        let mut c = TagChanges::new();
        c.set(TagField::Title, ["a", "b"]);
        assert!(c.normalized().is_err());
        let mut c = TagChanges::new();
        c.set_one(FieldKey::Custom("a=b".into()), "x");
        assert!(c.normalized().is_err());
    }

    #[test]
    fn covers_must_be_jpeg_or_png() {
        let mut c = TagChanges::new();
        c.cover = Some(CoverChange::SetFront(b"GIF89a....".to_vec()));
        assert!(matches!(c.normalized(), Err(WriteError::InvalidCover(_))));
    }

    #[test]
    fn keys_parse_by_picard_name() {
        assert_eq!(
            FieldKey::parse("AlbumArtist"),
            FieldKey::Known(TagField::AlbumArtist)
        );
        assert_eq!(
            FieldKey::parse(" MY_FIELD "),
            FieldKey::Custom("MY_FIELD".into())
        );
    }
}
