// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! RIFF `INFO` chunks in WAV files.

use super::{Table, lookup};
use crate::tags::field::Field;

const TABLE: Table = &[
    ("INAM", Field::Title),
    ("IART", Field::Artist),
    ("IPRD", Field::Album),
    ("ICRD", Field::Date),
    ("IGNR", Field::Genre),
    ("ICMT", Field::Comment),
    ("ITRK", Field::TrackNumber),
    ("IPRT", Field::TrackNumber),
    ("IFRM", Field::TotalTracks),
    ("ICOP", Field::Copyright),
    ("IMUS", Field::Composer),
    ("IWRI", Field::Lyricist),
    ("IENG", Field::Engineer),
    ("IPRO", Field::Producer),
    ("ILNG", Field::Language),
    ("IBPM", Field::Bpm),
    ("ISRC", Field::Ignored), // "source", not the recording code
    ("ISFT", Field::Ignored),
];

/// The field of a RIFF `INFO` chunk ID.
pub(crate) fn field(id: &str) -> Option<Field> {
    lookup(TABLE, id)
}

#[cfg(test)]
mod tests {
    #[test]
    fn table_has_no_duplicates() {
        crate::tags::mapping::assert_no_duplicates(super::TABLE);
    }
}
