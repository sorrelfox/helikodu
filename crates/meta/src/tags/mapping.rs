// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which tag key of each format means which field.
//!
//! The tables follow MusicBrainz Picard's documented tag mapping, so that files
//! tagged by Picard (and by the players that copy its names) read the same in every
//! format. Keys are compared without regard to ASCII case. Each format falls back to
//! the Vorbis comment names for keys it has no entry for, as Picard does for APEv2,
//! `TXXX` frames and MP4 freeform atoms.

pub(crate) mod ape;
pub(crate) mod ffmpeg;
pub(crate) mod id3v2;
pub(crate) mod mp4;
pub(crate) mod riff;
pub(crate) mod vorbis;

use super::field::Field;

/// A mapping table: tag key to field.
pub(crate) type Table = &'static [(&'static str, Field)];

/// The field of `key` in `table`, compared without regard to ASCII case.
pub(crate) fn lookup(table: Table, key: &str) -> Option<Field> {
    table
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, f)| *f)
}

#[cfg(test)]
pub(crate) fn assert_no_duplicates(table: Table) {
    for (i, (a, _)) in table.iter().enumerate() {
        for (b, _) in &table[i + 1..] {
            assert!(!a.eq_ignore_ascii_case(b), "duplicate key {a}");
        }
    }
}
