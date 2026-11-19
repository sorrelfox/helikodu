// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The seam each tag format implements, and the plan that drives it.
//!
//! A file's main tag (ID3v2 in MP3, WAV and AIFF; Vorbis comments in FLAC and Ogg;
//! APEv2 in WavPack, Monkey's Audio and Musepack; `ilst` in MP4) gets every
//! change. The other tags a file already has (APEv2 and ID3v1 on MP3, RIFF `INFO`
//! in WAV, AIFF text chunks) get the changes they can hold, so that no stale value
//! in them reads back once the main tag no longer has the field.

use crate::tags::field::Field;
use crate::tags::rating::Rating;
use crate::tags::record::TrackTags;

use super::changes::{CoverChange, FieldKey, Normalized, TagField};

/// Why a tag did not take a change; reported for the main tag only.
pub(crate) type Skip = &'static str;

/// Why a tag other than the comments of an Opus file does not take an R128 gain.
pub(crate) const ONLY_OPUS: Skip = "only Opus files carry R128 gains";

/// A number with its total: track, disc or movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Pair {
    pub(crate) number: Option<u32>,
    pub(crate) total: Option<u32>,
}

impl Pair {
    /// `n/total`, `n`, or `None` when there is no number (a lone total cannot be
    /// written this way).
    pub(crate) fn joined(self) -> Option<String> {
        let n = self.number?;
        Some(match self.total {
            Some(total) => format!("{n}/{total}"),
            None => n.to_string(),
        })
    }
}

/// Which pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairKind {
    Track,
    Disc,
    Movement,
}

impl PairKind {
    pub(crate) const ALL: [PairKind; 3] = [PairKind::Track, PairKind::Disc, PairKind::Movement];

    pub(crate) fn fields(self) -> (TagField, TagField) {
        match self {
            PairKind::Track => (TagField::TrackNumber, TagField::TrackTotal),
            PairKind::Disc => (TagField::DiscNumber, TagField::DiscTotal),
            PairKind::Movement => (TagField::MovementNumber, TagField::MovementTotal),
        }
    }

    fn current(self, tags: &TrackTags) -> Pair {
        let (number, total) = match self {
            PairKind::Track => (tags.track_number, tags.track_total),
            PairKind::Disc => (tags.disc_number, tags.disc_total),
            PairKind::Movement => (tags.movement_number, tags.movement_total),
        };
        Pair { number, total }
    }
}

/// The table fields a written field replaces: its own, and the bare year beside a date.
pub(crate) fn replaced_fields(field: TagField) -> Vec<Field> {
    match field {
        TagField::Date => vec![Field::Date, Field::Year],
        TagField::OriginalDate => vec![Field::OriginalDate, Field::OriginalYear],
        _ => vec![field.field()],
    }
}

/// The table fields a pair replaces.
pub(crate) fn pair_fields(kind: PairKind) -> [Field; 2] {
    let (number, total) = kind.fields();
    [number.field(), total.field()]
}

/// What to write into one file: the checked changes with the pairs resolved
/// against the file's current values.
#[derive(Debug)]
pub(crate) struct Plan<'a> {
    pub(crate) changes: &'a Normalized,
    pub(crate) pairs: Vec<(PairKind, Pair)>,
}

impl<'a> Plan<'a> {
    pub(crate) fn new(changes: &'a Normalized, current: &TrackTags) -> Self {
        let pairs = PairKind::ALL
            .into_iter()
            .filter_map(|kind| {
                let (number, total) = kind.fields();
                let (n, t) = (changes.number(number), changes.number(total));
                if n.is_none() && t.is_none() {
                    return None;
                }
                let current = kind.current(current);
                Some((
                    kind,
                    Pair {
                        number: n.unwrap_or(current.number),
                        total: t.unwrap_or(current.total),
                    },
                ))
            })
            .collect();
        Self { changes, pairs }
    }

    /// The changed fields other than the pairs, with their new values (empty to clear).
    pub(crate) fn fields(&self) -> impl Iterator<Item = (&FieldKey, &[String])> {
        self.changes
            .fields
            .iter()
            .filter(|(key, _)| !is_pair_field(key))
            .map(|(key, values)| (key, values.as_slice()))
    }
}

fn is_pair_field(key: &FieldKey) -> bool {
    PairKind::ALL.iter().any(|kind| {
        let (number, total) = kind.fields();
        *key == FieldKey::Known(number) || *key == FieldKey::Known(total)
    })
}

/// One tag of a file, being edited in memory.
pub(crate) trait TagEditor {
    /// Writes `values` into `field`, replacing every key that means it; empty
    /// values remove it.
    fn set_field(&mut self, field: TagField, values: &[String]) -> Result<(), Skip>;

    /// Writes a custom field, replacing a key of the same name in any case.
    fn set_custom(&mut self, key: &str, values: &[String]) -> Result<(), Skip>;

    /// Writes a number and its total.
    fn set_pair(&mut self, kind: PairKind, pair: Pair) -> Result<(), Skip>;

    /// Writes or removes the unsynchronized lyrics.
    fn set_lyrics(&mut self, lyrics: Option<&str>) -> Result<(), Skip>;

    /// Writes or removes the rating.
    fn set_rating(&mut self, rating: Option<Rating>) -> Result<(), Skip>;

    /// Writes or removes the play count.
    fn set_play_count(&mut self, _count: Option<u64>) -> Result<(), Skip> {
        Err("the tag cannot hold it")
    }

    /// Changes the pictures, when the tag holds them.
    fn set_cover(&mut self, change: &CoverChange) -> Result<(), Skip>;
}

/// A change the main tag did not take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedChange {
    /// What was not written: a field name, `lyrics`, `rating`, `play count` or
    /// `cover`.
    pub what: String,
    /// Why.
    pub reason: &'static str,
}

/// Applies `plan` to `editor`. Changes the tag cannot take are returned.
pub(crate) fn apply(editor: &mut dyn TagEditor, plan: &Plan<'_>) -> Vec<SkippedChange> {
    let mut skipped = Vec::new();
    let mut note = |what: &str, result: Result<(), Skip>| {
        if let Err(reason) = result {
            skipped.push(SkippedChange {
                what: what.to_owned(),
                reason,
            });
        }
    };
    for (key, values) in plan.fields() {
        let result = match key {
            FieldKey::Known(field) => editor.set_field(*field, values),
            FieldKey::Custom(name) => editor.set_custom(name, values),
        };
        note(key.name(), result);
    }
    for (kind, pair) in &plan.pairs {
        note(kind.fields().0.picard_name(), editor.set_pair(*kind, *pair));
    }
    if let Some(lyrics) = &plan.changes.lyrics {
        note("lyrics", editor.set_lyrics(lyrics.as_deref()));
    }
    // Before the rating: the ID3v2 rating keeps the counter it finds.
    if let Some(count) = plan.changes.play_count {
        note("play count", editor.set_play_count(count));
    }
    if let Some(rating) = plan.changes.rating {
        note("rating", editor.set_rating(rating));
    }
    if let Some(cover) = &plan.changes.cover {
        note("cover", editor.set_cover(cover));
    }
    skipped
}

/// The ratings as written to files. `POPM` has no half stars: a half star is
/// written as the whole star above it, and `FMPS_RATING` keeps the exact value.
pub(crate) mod rating_values {
    use crate::tags::rating::Rating;

    /// The e-mail field of the `POPM` frames this program writes.
    pub(crate) const POPM_EMAIL: &str = "Helikodu";

    /// The `POPM` value: 1, 64, 128, 196 or 255 for one to five stars; `None`
    /// for no stars, which `POPM` cannot express (0 means "unknown").
    pub(crate) fn popm(rating: Rating) -> Option<u8> {
        match rating.half_stars().div_ceil(2) {
            0 => None,
            1 => Some(1),
            2 => Some(64),
            3 => Some(128),
            4 => Some(196),
            _ => Some(255),
        }
    }

    /// The name of the play count field (`FMPS_PLAYCOUNT`), as `TXXX` frames and
    /// MP4 freeform atoms spell it.
    pub(crate) const FMPS_PLAYCOUNT: &str = "FMPS_Playcount";

    /// The `FMPS_PLAYCOUNT` values of `count`: none to remove it.
    pub(crate) fn play_count(count: Option<u64>) -> Vec<String> {
        count.map(|c| c.to_string()).into_iter().collect()
    }

    /// The `FMPS_RATING` value, 0.0 to 1.0.
    pub(crate) fn fmps(rating: Rating) -> String {
        format!("{:.1}", f64::from(rating.half_stars()) / 10.0)
    }

    /// The `RATING` value on the 0 to 100 scale; `None` for no stars (0 means
    /// "not rated").
    pub(crate) fn percent(rating: Rating) -> Option<String> {
        (rating.half_stars() > 0).then(|| (u32::from(rating.half_stars()) * 10).to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::tags::rating::{from_fmps, from_popm, from_rating_field};

        #[test]
        fn written_ratings_read_back() {
            for half in 0..=10u8 {
                let rating = Rating::from_half_stars(half).unwrap_or(Rating::MAX);
                assert_eq!(from_fmps(&fmps(rating)), Some(rating));
                match percent(rating) {
                    Some(p) => assert_eq!(from_rating_field(&p), Some(rating)),
                    None => assert_eq!(half, 0),
                }
                match popm(rating) {
                    Some(v) => assert_eq!(
                        from_popm(POPM_EMAIL, v).map(Rating::half_stars),
                        Some(half.div_ceil(2) * 2)
                    ),
                    None => assert_eq!(half, 0),
                }
            }
        }
    }
}
