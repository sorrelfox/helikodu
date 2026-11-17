// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Builds a [`TrackTags`] from mapped fields, one tag after another.
//!
//! A file can carry several tags (ID3v2, APEv2 and ID3v1 on one MP3). The readers
//! feed them in order of preference, each after [`TagBuilder::begin_tag`]. A
//! single-valued field keeps the first value it gets. A multi-valued field belongs
//! to the first tag that gives it a value: later tags add nothing to it, so an
//! ID3v1 genre never joins the genres of the ID3v2 tag.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use lofty::id3::v1::GENRES;

use super::field::Field;
use super::gain::{parse_gain_db, parse_peak, parse_r128};
use super::rating::{self, Rating};
use super::record::{Credit, Role, TrackTags};

/// The longest value kept in [`TrackTags::extra`]; longer values are binary
/// data or text the library has no use for.
const MAX_EXTRA_LEN: usize = 16 * 1024;

/// Which multi-valued list a value goes to, for ownership by one tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum List {
    Field(Field),
    Extra(String),
}

#[derive(Debug, Default)]
pub(crate) struct TagBuilder {
    tags: TrackTags,
    tag_index: u32,
    owners: HashMap<List, u32>,
    artist_values: Vec<String>,
    album_artist_values: Vec<String>,
    artists_list: Vec<String>,
    album_artists_list: Vec<String>,
    year: Option<String>,
    original_year: Option<String>,
    release_date: Option<String>,
    fmps_rating: Option<Rating>,
    popm_rating: Option<Rating>,
    field_rating: Option<Rating>,
    wmp_rating: Option<Rating>,
}

impl TagBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Starts the next tag, which is preferred less than the ones before.
    pub(crate) fn begin_tag(&mut self) {
        self.tag_index += 1;
    }

    /// Whether the current tag may add to `list`.
    fn owns(&mut self, list: List) -> bool {
        match self.owners.entry(list) {
            Entry::Occupied(owner) => *owner.get() == self.tag_index,
            Entry::Vacant(entry) => {
                entry.insert(self.tag_index);
                true
            }
        }
    }

    fn push(&mut self, field: Field, value: &str) {
        if !self.owns(List::Field(field)) {
            return;
        }
        let value = value.to_owned();
        match field {
            Field::Artist => self.artist_values.push(value),
            Field::Artists => self.artists_list.push(value),
            Field::AlbumArtist => self.album_artist_values.push(value),
            Field::AlbumArtists => self.album_artists_list.push(value),
            Field::Genre => self.tags.genres.push(value),
            Field::Label => self.tags.labels.push(value),
            Field::CatalogNumber => self.tags.catalog_numbers.push(value),
            Field::ReleaseType => self.tags.release_types.push(value),
            Field::MbArtistId => self.tags.musicbrainz.artists.push(value),
            Field::MbAlbumArtistId => self.tags.musicbrainz.album_artists.push(value),
            Field::MbWorkId => self.tags.musicbrainz.works.push(value),
            _ => {}
        }
    }

    /// Adds a credited person. `field` is the role's field.
    pub(crate) fn add_credit(&mut self, field: Field, name: &str, instrument: Option<&str>) {
        let name = name.trim();
        let Some(role) = credit_role(field) else {
            return;
        };
        if name.is_empty() || !self.owns(List::Field(field)) {
            return;
        }
        let instrument = instrument.map(str::trim).filter(|i| !i.is_empty());
        self.tags.credits.push(Credit {
            role,
            name: name.to_owned(),
            instrument: instrument.map(str::to_owned),
        });
    }

    /// Adds one value of a field the tag does not map: kept in `extra` under `key`.
    pub(crate) fn apply_unknown(&mut self, key: &str, value: &str) {
        let key = key.trim().to_lowercase();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            return;
        }
        self.push_extra(key, value);
    }

    fn push_extra(&mut self, key: String, value: &str) {
        if value.len() > MAX_EXTRA_LEN || !self.owns(List::Extra(key.clone())) {
            return;
        }
        self.tags.extra.push((key, value.to_owned()));
    }

    /// The rating of an ID3v2 `POPM` frame.
    pub(crate) fn set_popm_rating(&mut self, rating: Option<Rating>) {
        if self.popm_rating.is_none() {
            self.popm_rating = rating;
        }
    }

    /// Records that the file carries lyrics.
    pub(crate) fn mark_lyrics(&mut self) {
        self.tags.has_lyrics = true;
    }

    /// Adds one value of `field`. The value is trimmed; an empty value is ignored.
    pub(crate) fn apply(&mut self, field: Field, value: &str) {
        let value = value.trim_matches(|c: char| c.is_whitespace() || c == '\0');
        if value.is_empty() {
            return;
        }
        let tags = &mut self.tags;
        match field {
            Field::Title => set(&mut tags.title, value),
            Field::TitleSort => set(&mut tags.title_sort, value),
            Field::ArtistSort => set(&mut tags.artist_sort, value),
            Field::Album => set(&mut tags.album, value),
            Field::AlbumSort => set(&mut tags.album_sort, value),
            Field::AlbumArtistSort => set(&mut tags.album_artist_sort, value),
            Field::ComposerSort => set(&mut tags.composer_sort, value),
            Field::DiscSubtitle => set(&mut tags.disc_subtitle, value),
            Field::Date => set(&mut tags.date, value),
            Field::OriginalDate => set(&mut tags.original_date, value),
            Field::Year => set(&mut self.year, value),
            Field::OriginalYear => set(&mut self.original_year, value),
            Field::ReleaseDate => {
                set(&mut self.release_date, value);
                self.push_extra(field.picard_name().to_owned(), value);
            }
            Field::Comment => set(&mut tags.comment, value),
            Field::Grouping => set(&mut tags.grouping, value),
            Field::Work => set(&mut tags.work, value),
            Field::Movement => set(&mut tags.movement, value),
            Field::TrackNumber => {
                let (n, total) = parse_pair(value);
                set_num(&mut tags.track_number, n);
                set_num(&mut tags.track_total, total);
            }
            Field::TotalTracks => set_num(&mut tags.track_total, parse_number(value)),
            Field::DiscNumber => {
                let (n, total) = parse_pair(value);
                set_num(&mut tags.disc_number, n);
                set_num(&mut tags.disc_total, total);
            }
            Field::TotalDiscs => set_num(&mut tags.disc_total, parse_number(value)),
            Field::MovementNumber => {
                let (n, total) = parse_pair(value);
                set_num(&mut tags.movement_number, n);
                set_num(&mut tags.movement_total, total);
            }
            Field::MovementTotal => set_num(&mut tags.movement_total, parse_number(value)),
            Field::Compilation => {
                if tags.compilation.is_none() {
                    tags.compilation = parse_bool(value);
                }
            }
            Field::Lyrics => tags.has_lyrics = true,
            Field::MbRecordingId => set(&mut tags.musicbrainz.recording, value),
            Field::MbTrackId => set(&mut tags.musicbrainz.release_track, value),
            Field::MbAlbumId => set(&mut tags.musicbrainz.release, value),
            Field::MbReleaseGroupId => set(&mut tags.musicbrainz.release_group, value),
            Field::MbArtistId | Field::MbAlbumArtistId | Field::MbWorkId => {
                // Identifiers never contain these separators; ID3v2.3 writers join
                // lists with a slash.
                let ids: Vec<&str> = value
                    .split(['/', ';', ','])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                for id in ids {
                    self.push(field, id);
                }
            }
            Field::Genre => {
                for genre in split_genres(value) {
                    self.push(field, &genre);
                }
            }
            Field::Artist
            | Field::Artists
            | Field::AlbumArtist
            | Field::AlbumArtists
            | Field::Label
            | Field::CatalogNumber
            | Field::ReleaseType => self.push(field, value),
            Field::Composer
            | Field::Conductor
            | Field::Lyricist
            | Field::Writer
            | Field::Arranger
            | Field::Remixer
            | Field::Producer
            | Field::Engineer
            | Field::Mixer
            | Field::DjMixer => self.add_credit(field, value, None),
            Field::Performer => {
                let (name, instrument) = split_performer(value);
                self.add_credit(field, name, instrument);
            }
            Field::ReplayGainTrackGain => {
                set_opt(&mut tags.replay_gain.track_gain_db, parse_gain_db(value))
            }
            Field::ReplayGainTrackPeak => {
                set_opt(&mut tags.replay_gain.track_peak, parse_peak(value))
            }
            Field::ReplayGainAlbumGain => {
                set_opt(&mut tags.replay_gain.album_gain_db, parse_gain_db(value))
            }
            Field::ReplayGainAlbumPeak => {
                set_opt(&mut tags.replay_gain.album_peak, parse_peak(value))
            }
            Field::R128TrackGain => {
                set_opt(&mut tags.replay_gain.r128_track_gain, parse_r128(value))
            }
            Field::R128AlbumGain => {
                set_opt(&mut tags.replay_gain.r128_album_gain, parse_r128(value))
            }
            Field::Rating => set_opt(&mut self.field_rating, rating::from_rating_field(value)),
            Field::FmpsRating => set_opt(&mut self.fmps_rating, rating::from_fmps(value)),
            Field::WmpRating => set_opt(&mut self.wmp_rating, rating::from_wmp_shared(value)),
            Field::Ignored => {}
            Field::Subtitle
            | Field::Barcode
            | Field::Isrc
            | Field::Asin
            | Field::ReleaseStatus
            | Field::ReleaseCountry
            | Field::Media
            | Field::Script
            | Field::Language
            | Field::MbDiscId
            | Field::AcoustidId
            | Field::ShowMovement
            | Field::Bpm
            | Field::Key
            | Field::Mood
            | Field::EncodedBy
            | Field::EncoderSettings
            | Field::Copyright
            | Field::License
            | Field::Website
            | Field::Director
            | Field::OriginalAlbum
            | Field::OriginalArtist => self.push_extra(field.picard_name().to_owned(), value),
        }
    }

    /// The finished record.
    pub(crate) fn finish(mut self) -> TrackTags {
        let mut tags = self.tags;
        tags.artists = if self.artists_list.is_empty() {
            self.artist_values.clone()
        } else {
            self.artists_list
        };
        tags.artist = join(&self.artist_values).or_else(|| join(&tags.artists));
        tags.album_artists = if self.album_artists_list.is_empty() {
            self.album_artist_values.clone()
        } else {
            self.album_artists_list
        };
        tags.album_artist = join(&self.album_artist_values).or_else(|| join(&tags.album_artists));
        if tags.date.is_none() {
            tags.date = self.year.take().or(self.release_date.take());
        }
        if tags.original_date.is_none() {
            tags.original_date = self.original_year.take();
        }
        tags.rating = self
            .fmps_rating
            .or(self.popm_rating)
            .or(self.field_rating)
            .or(self.wmp_rating);
        tags
    }
}

fn join(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| values.join("; "))
}

fn set(slot: &mut Option<String>, value: &str) {
    if slot.is_none() {
        *slot = Some(value.to_owned());
    }
}

fn set_opt<T>(slot: &mut Option<T>, value: Option<T>) {
    if slot.is_none() {
        *slot = value;
    }
}

fn set_num(slot: &mut Option<u32>, value: Option<u32>) {
    set_opt(slot, value);
}

fn credit_role(field: Field) -> Option<Role> {
    Some(match field {
        Field::Composer => Role::Composer,
        Field::Conductor => Role::Conductor,
        Field::Lyricist => Role::Lyricist,
        Field::Writer => Role::Writer,
        Field::Arranger => Role::Arranger,
        Field::Remixer => Role::Remixer,
        Field::Producer => Role::Producer,
        Field::Engineer => Role::Engineer,
        Field::Mixer => Role::Mixer,
        Field::DjMixer => Role::DjMixer,
        Field::Performer => Role::Performer,
        _ => return None,
    })
}

/// The leading number of `text` (`03` → 3, `7 of 9` → 7); 0 and junk are `None`.
pub(crate) fn parse_number(text: &str) -> Option<u32> {
    let digits: &str = {
        let t = text.trim();
        let end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
        &t[..end]
    };
    digits.parse().ok().filter(|n| *n > 0)
}

/// A number with an optional total: `3`, `3/12`, ` 3 / 12 `.
fn parse_pair(text: &str) -> (Option<u32>, Option<u32>) {
    match text.split_once('/') {
        Some((n, total)) => (parse_number(n), parse_number(total)),
        None => (parse_number(text), None),
    }
}

fn parse_bool(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

/// `Name (instrument)`, as Vorbis comments and APEv2 store a performer.
fn split_performer(text: &str) -> (&str, Option<&str>) {
    if let Some(inner) = text.strip_suffix(')')
        && let Some(open) = inner.rfind(" (")
    {
        let name = &inner[..open];
        let instrument = &inner[open + 2..];
        if !name.trim().is_empty() && !instrument.trim().is_empty() {
            return (name, Some(instrument));
        }
    }
    (text, None)
}

/// The genres in one value: `;`-separated lists, and ID3's numeric references
/// (`(17)`, `(17)Rock`, `17`, `RX`, `CR`).
fn split_genres(value: &str) -> Vec<String> {
    let mut genres = Vec::new();
    for part in value.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut rest = part;
        let mut referenced = Vec::new();
        while let Some(inner) = rest.strip_prefix('(') {
            // `((` escapes a genre name that starts with a parenthesis.
            if inner.starts_with('(') {
                rest = inner;
                break;
            }
            let Some(close) = inner.find(')') else {
                break;
            };
            let reference = &inner[..close];
            match genre_reference(reference) {
                Some(name) => referenced.push(name),
                None => break,
            }
            rest = &inner[close + 1..];
        }
        let rest = rest.trim();
        if !rest.is_empty() {
            // A refinement after the references replaces them, as in `(17)Rock`.
            match genre_reference(rest) {
                Some(name)
                    if referenced.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()) =>
                {
                    genres.push(name);
                }
                _ => genres.push(rest.to_owned()),
            }
        } else {
            genres.extend(referenced);
        }
    }
    genres.dedup();
    genres
}

fn genre_reference(reference: &str) -> Option<String> {
    match reference {
        "RX" => Some("Remix".to_owned()),
        "CR" => Some("Cover".to_owned()),
        _ if !reference.is_empty() && reference.bytes().all(|b| b.is_ascii_digit()) => reference
            .parse::<usize>()
            .ok()
            .and_then(|i| GENRES.get(i))
            .map(|g| (*g).to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(parse_pair("3/12"), (Some(3), Some(12)));
        assert_eq!(parse_pair(" 03 / 012 "), (Some(3), Some(12)));
        assert_eq!(parse_pair("7"), (Some(7), None));
        assert_eq!(parse_pair("A1"), (None, None));
        assert_eq!(parse_pair("0/0"), (None, None));
    }

    #[test]
    fn genres() {
        assert_eq!(split_genres("Rock"), ["Rock"]);
        assert_eq!(split_genres("(17)"), ["Rock"]);
        assert_eq!(split_genres("17"), ["Rock"]);
        assert_eq!(split_genres("(17)Hard Rock"), ["Hard Rock"]);
        assert_eq!(split_genres("(17)(18)"), ["Rock", "Techno"]);
        assert_eq!(split_genres("Rock; Pop"), ["Rock", "Pop"]);
        assert_eq!(split_genres("(RX)"), ["Remix"]);
        assert_eq!(split_genres("Drum & Bass"), ["Drum & Bass"]);
        assert_eq!(split_genres("((Parenthesised)"), ["(Parenthesised)"]);
        assert_eq!(split_genres("AC/DC Tribute"), ["AC/DC Tribute"]);
        assert_eq!(split_genres("999"), ["999"]);
    }

    #[test]
    fn performers() {
        assert_eq!(
            split_performer("Jaco Pastorius (bass)"),
            ("Jaco Pastorius", Some("bass"))
        );
        assert_eq!(split_performer("Prince"), ("Prince", None));
        assert_eq!(split_performer("(hed) p.e."), ("(hed) p.e.", None));
    }

    #[test]
    fn first_tag_owns_multi_valued_fields() {
        let mut b = TagBuilder::new();
        b.begin_tag();
        b.apply(Field::Genre, "Jazz");
        b.apply(Field::Genre, "Fusion");
        b.apply(Field::Title, "First");
        b.begin_tag();
        b.apply(Field::Genre, "Blues");
        b.apply(Field::Title, "Second");
        b.apply(Field::Album, "From the second tag");
        let t = b.finish();
        assert_eq!(t.genres, ["Jazz", "Fusion"]);
        assert_eq!(t.title.as_deref(), Some("First"));
        assert_eq!(t.album.as_deref(), Some("From the second tag"));
    }

    #[test]
    fn artists_list_and_credit() {
        let mut b = TagBuilder::new();
        b.begin_tag();
        b.apply(Field::Artist, "A feat. B");
        b.apply(Field::Artists, "A");
        b.apply(Field::Artists, "B");
        b.apply(Field::AlbumArtist, "X");
        b.apply(Field::AlbumArtist, "Y");
        b.apply(Field::MbArtistId, "id-a/id-b");
        let t = b.finish();
        assert_eq!(t.artist.as_deref(), Some("A feat. B"));
        assert_eq!(t.artists, ["A", "B"]);
        assert_eq!(t.album_artists, ["X", "Y"]);
        assert_eq!(t.album_artist.as_deref(), Some("X; Y"));
        assert_eq!(t.musicbrainz.artists, ["id-a", "id-b"]);
    }

    #[test]
    fn dates_and_rating_preference() {
        let mut b = TagBuilder::new();
        b.begin_tag();
        b.apply(Field::Year, "1999");
        b.apply(Field::OriginalYear, "1970");
        b.apply(Field::Rating, "60");
        b.apply(Field::FmpsRating, "0.9");
        let t = b.finish();
        assert_eq!(t.date.as_deref(), Some("1999"));
        assert_eq!(t.original_date.as_deref(), Some("1970"));
        assert_eq!(t.rating.map(Rating::half_stars), Some(9));
    }

    #[test]
    fn unknown_and_extra_fields() {
        let mut b = TagBuilder::new();
        b.begin_tag();
        b.apply(Field::Bpm, "120");
        b.apply_unknown("My Field", "value");
        b.apply(Field::Ignored, "x");
        let t = b.finish();
        assert_eq!(t.extra_values("bpm").collect::<Vec<_>>(), ["120"]);
        assert_eq!(t.extra_values("my field").collect::<Vec<_>>(), ["value"]);
        assert_eq!(t.extra.len(), 2);
    }
}
