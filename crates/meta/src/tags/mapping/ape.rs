// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! APEv2 items (Monkey's Audio, WavPack, Musepack, and APE tags on MP3): the keys
//! that differ from the Vorbis comment names, as Picard writes them.

use super::{Table, lookup, vorbis};
use crate::tags::field::Field;

const TABLE: Table = &[
    ("Track", Field::TrackNumber),
    ("Disc", Field::DiscNumber),
    ("Year", Field::Date),
    ("Album Artist", Field::AlbumArtist),
    ("MixArtist", Field::Remixer),
    ("Weblink", Field::Website),
    ("Catalog", Field::CatalogNumber),
    ("Record Date", Field::Date),
    ("Original Year", Field::OriginalYear),
    ("Cover Art (Front)", Field::Ignored),
    ("Cover Art (Back)", Field::Ignored),
];

/// The field of an APEv2 item key.
pub(crate) fn field(key: &str) -> Option<Field> {
    lookup(TABLE, key).or_else(|| vorbis::field(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_no_duplicates() {
        crate::tags::mapping::assert_no_duplicates(TABLE);
    }

    #[test]
    fn lookups() {
        assert_eq!(field("Year"), Some(Field::Date));
        assert_eq!(field("Track"), Some(Field::TrackNumber));
        assert_eq!(field("MUSICBRAINZ_ALBUMID"), Some(Field::MbAlbumId));
        assert_eq!(field("Artist"), Some(Field::Artist));
    }
}
