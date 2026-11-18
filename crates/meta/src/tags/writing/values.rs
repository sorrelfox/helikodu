// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A field's values as the editor shows them, taken from a read [`TrackTags`].

use crate::tags::builder::TagBuilder;
use crate::tags::record::{Role, TrackTags};

use super::changes::{FieldKey, TagField};

/// The values of `key` in `tags`, in tag order; empty when the field is not set.
///
/// Numbers are decimal, the compilation flag `1` or `0`, loudness gains
/// `-6.50 dB` and peaks with six decimals; a performer with an instrument is
/// `Name (instrument)`.
pub fn field_values(tags: &TrackTags, key: &FieldKey) -> Vec<String> {
    match key {
        FieldKey::Known(field) => known_values(tags, *field),
        FieldKey::Custom(name) => tags.extra_values(name).map(str::to_owned).collect(),
    }
}

fn one(value: &Option<String>) -> Vec<String> {
    value.iter().cloned().collect()
}

fn number(value: Option<u32>) -> Vec<String> {
    value.iter().map(u32::to_string).collect()
}

fn credits(tags: &TrackTags, role: Role) -> Vec<String> {
    tags.credits_with(role)
        .map(|c| match &c.instrument {
            Some(instrument) => format!("{} ({instrument})", c.name),
            None => c.name.clone(),
        })
        .collect()
}

fn gain(value: Option<f64>) -> Vec<String> {
    value.iter().map(|g| format!("{g:.2} dB")).collect()
}

fn peak(value: Option<f64>) -> Vec<String> {
    value.iter().map(|p| format!("{p:.6}")).collect()
}

fn known_values(tags: &TrackTags, field: TagField) -> Vec<String> {
    let mb = &tags.musicbrainz;
    let rg = &tags.replay_gain;
    match field {
        TagField::Title => one(&tags.title),
        TagField::TitleSort => one(&tags.title_sort),
        TagField::Artist => one(&tags.artist),
        TagField::Artists => tags.artists.clone(),
        TagField::ArtistSort => one(&tags.artist_sort),
        TagField::Album => one(&tags.album),
        TagField::AlbumSort => one(&tags.album_sort),
        TagField::AlbumArtist => one(&tags.album_artist),
        TagField::AlbumArtists => tags.album_artists.clone(),
        TagField::AlbumArtistSort => one(&tags.album_artist_sort),
        TagField::TrackNumber => number(tags.track_number),
        TagField::TrackTotal => number(tags.track_total),
        TagField::DiscNumber => number(tags.disc_number),
        TagField::DiscTotal => number(tags.disc_total),
        TagField::DiscSubtitle => one(&tags.disc_subtitle),
        TagField::Date => one(&tags.date),
        TagField::OriginalDate => one(&tags.original_date),
        TagField::Genre => tags.genres.clone(),
        TagField::Composer => credits(tags, Role::Composer),
        TagField::ComposerSort => one(&tags.composer_sort),
        TagField::Conductor => credits(tags, Role::Conductor),
        TagField::Lyricist => credits(tags, Role::Lyricist),
        TagField::Writer => credits(tags, Role::Writer),
        TagField::Arranger => credits(tags, Role::Arranger),
        TagField::Remixer => credits(tags, Role::Remixer),
        TagField::Producer => credits(tags, Role::Producer),
        TagField::Engineer => credits(tags, Role::Engineer),
        TagField::Mixer => credits(tags, Role::Mixer),
        TagField::DjMixer => credits(tags, Role::DjMixer),
        TagField::Performer => credits(tags, Role::Performer),
        TagField::Label => tags.labels.clone(),
        TagField::CatalogNumber => tags.catalog_numbers.clone(),
        TagField::ReleaseType => tags.release_types.clone(),
        TagField::MusicBrainzRecordingId => one(&mb.recording),
        TagField::MusicBrainzTrackId => one(&mb.release_track),
        TagField::MusicBrainzAlbumId => one(&mb.release),
        TagField::MusicBrainzReleaseGroupId => one(&mb.release_group),
        TagField::MusicBrainzArtistId => mb.artists.clone(),
        TagField::MusicBrainzAlbumArtistId => mb.album_artists.clone(),
        TagField::MusicBrainzWorkId => mb.works.clone(),
        TagField::Comment => one(&tags.comment),
        TagField::Compilation => tags
            .compilation
            .iter()
            .map(|c| if *c { "1" } else { "0" }.to_owned())
            .collect(),
        TagField::Grouping => one(&tags.grouping),
        TagField::Work => one(&tags.work),
        TagField::Movement => one(&tags.movement),
        TagField::MovementNumber => number(tags.movement_number),
        TagField::MovementTotal => number(tags.movement_total),
        TagField::ReplayGainTrackGain => gain(rg.track_gain_db),
        TagField::ReplayGainTrackPeak => peak(rg.track_peak),
        TagField::ReplayGainAlbumGain => gain(rg.album_gain_db),
        TagField::ReplayGainAlbumPeak => peak(rg.album_peak),
        TagField::R128TrackGain => rg.r128_track_gain.iter().map(i16::to_string).collect(),
        TagField::R128AlbumGain => rg.r128_album_gain.iter().map(i16::to_string).collect(),
        TagField::Subtitle
        | TagField::ReleaseDate
        | TagField::Barcode
        | TagField::Isrc
        | TagField::Asin
        | TagField::ReleaseStatus
        | TagField::ReleaseCountry
        | TagField::Media
        | TagField::Script
        | TagField::Language
        | TagField::MusicBrainzDiscId
        | TagField::AcoustidId
        | TagField::ShowMovement
        | TagField::Bpm
        | TagField::Key
        | TagField::Mood
        | TagField::EncodedBy
        | TagField::EncoderSettings
        | TagField::Copyright
        | TagField::License
        | TagField::Website
        | TagField::Director
        | TagField::OriginalAlbum
        | TagField::OriginalArtist => tags
            .extra_values(field.picard_name())
            .map(str::to_owned)
            .collect(),
    }
}

/// The values `key` reads back as after `values` were written: the same rules
/// the reader applies (genre lists split, numbers without leading zeros, credits
/// with instruments, several artist values joined).
pub(crate) fn read_back_form(key: &FieldKey, values: &[String]) -> Vec<String> {
    let mut builder = TagBuilder::new();
    builder.begin_tag();
    for value in values {
        match key {
            FieldKey::Known(field) => builder.apply(field.field(), value),
            FieldKey::Custom(name) => builder.apply_unknown(name, value),
        }
    }
    field_values(&builder.finish(), key)
}

/// Whether `read` is what writing `expected` should read back as. Dates match
/// when one starts with the other: ID3v2.3 keeps only the year of some dates.
pub(crate) fn same_values(key: &FieldKey, expected: &[String], read: &[String]) -> bool {
    if expected == read {
        return true;
    }
    match key {
        FieldKey::Known(field) if field.is_date() => match (expected, read) {
            ([e], [r]) => e.starts_with(r.as_str()) || r.starts_with(e.as_str()),
            _ => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn read_back_follows_the_reader() {
        let genre = FieldKey::Known(TagField::Genre);
        assert_eq!(
            read_back_form(&genre, &s(&["Rock; Pop"])),
            s(&["Rock", "Pop"])
        );
        let track = FieldKey::Known(TagField::TrackNumber);
        assert_eq!(read_back_form(&track, &s(&["03"])), s(&["3"]));
        let artist = FieldKey::Known(TagField::Artist);
        assert_eq!(read_back_form(&artist, &s(&["A", "B"])), s(&["A; B"]));
        let performer = FieldKey::Known(TagField::Performer);
        assert_eq!(
            read_back_form(&performer, &s(&["Jaco (bass)", "Wayne"])),
            s(&["Jaco (bass)", "Wayne"])
        );
        let custom = FieldKey::Custom("My Field".into());
        assert_eq!(read_back_form(&custom, &s(&["x"])), s(&["x"]));
        let gain = FieldKey::Known(TagField::ReplayGainTrackGain);
        assert_eq!(read_back_form(&gain, &s(&["-6.5 dB"])), s(&["-6.50 dB"]));
    }

    #[test]
    fn dates_may_lose_their_tail() {
        let date = FieldKey::Known(TagField::OriginalDate);
        assert!(same_values(&date, &s(&["2001-05-03"]), &s(&["2001"])));
        assert!(!same_values(&date, &s(&["2001-05-03"]), &s(&["2002"])));
        let title = FieldKey::Known(TagField::Title);
        assert!(!same_values(&title, &s(&["Ab"]), &s(&["A"])));
    }
}
