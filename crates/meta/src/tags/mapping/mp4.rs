// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MP4 `ilst` atoms. `trkn`, `disk` and `gnre` are binary and handled by the
//! reader; freeform `----:com.apple.iTunes:NAME` atoms map like `TXXX` descriptions.

use super::{Table, id3v2, lookup};
use crate::tags::field::Field;

const ATOMS: Table = &[
    ("\u{a9}nam", Field::Title),
    ("sonm", Field::TitleSort),
    ("\u{a9}ART", Field::Artist),
    ("soar", Field::ArtistSort),
    ("\u{a9}alb", Field::Album),
    ("soal", Field::AlbumSort),
    ("aART", Field::AlbumArtist),
    ("soaa", Field::AlbumArtistSort),
    ("\u{a9}day", Field::Date),
    ("\u{a9}gen", Field::Genre),
    ("\u{a9}wrt", Field::Composer),
    ("soco", Field::ComposerSort),
    ("\u{a9}cmt", Field::Comment),
    ("\u{a9}lyr", Field::Lyrics),
    ("\u{a9}grp", Field::Grouping),
    ("\u{a9}wrk", Field::Work),
    ("\u{a9}mvn", Field::Movement),
    ("\u{a9}mvi", Field::MovementNumber),
    ("\u{a9}mvc", Field::MovementTotal),
    ("shwm", Field::ShowMovement),
    ("tmpo", Field::Bpm),
    ("cpil", Field::Compilation),
    ("\u{a9}too", Field::EncodedBy),
    ("cprt", Field::Copyright),
    ("\u{a9}dir", Field::Director),
    ("rate", Field::Rating),
    ("pgap", Field::Ignored),
    ("covr", Field::Ignored),
    ("stik", Field::Ignored),
    ("rtng", Field::Ignored),
    ("plID", Field::Ignored),
    ("cnID", Field::Ignored),
    ("atID", Field::Ignored),
    ("geID", Field::Ignored),
    ("sfID", Field::Ignored),
    ("cmID", Field::Ignored),
    ("akID", Field::Ignored),
    ("purd", Field::Ignored),
    ("apID", Field::Ignored),
    ("ownr", Field::Ignored),
    ("xid ", Field::Ignored),
];

/// The field of a four-character atom.
pub(crate) fn atom_field(fourcc: &str) -> Option<Field> {
    // Atom names are case-sensitive (`©ART` and `aART` differ only by the first
    // character, but `soar`/`SOAR` must not collide with anything either).
    ATOMS.iter().find(|(k, _)| *k == fourcc).map(|(_, f)| *f)
}

/// The field of a freeform atom name (the part after `----:com.apple.iTunes:`).
pub(crate) fn freeform_field(name: &str) -> Option<Field> {
    lookup(&[("initialkey", Field::Key)], name).or_else(|| id3v2::user_text_field(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_no_duplicates() {
        for (i, (a, _)) in ATOMS.iter().enumerate() {
            assert!(ATOMS[i + 1..].iter().all(|(b, _)| a != b), "duplicate {a}");
        }
    }

    #[test]
    fn lookups() {
        assert_eq!(atom_field("\u{a9}ART"), Some(Field::Artist));
        assert_eq!(atom_field("aART"), Some(Field::AlbumArtist));
        assert_eq!(
            freeform_field("MusicBrainz Track Id"),
            Some(Field::MbRecordingId)
        );
        assert_eq!(freeform_field("LABEL"), Some(Field::Label));
        assert_eq!(
            freeform_field("replaygain_album_peak"),
            Some(Field::ReplayGainAlbumPeak)
        );
    }
}
