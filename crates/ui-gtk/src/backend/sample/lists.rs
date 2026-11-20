// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Labels and the user's fields of the invented library, for the track table's
//! columns, the facet browser and the search completion: the labels of the
//! design's sample ("favourite", "late night" and so on), given to albums,
//! artists and tracks by a fixed rule, and three fields: `decade` and `era`,
//! computed from the year, and `mood`, the tag MOOD.

use std::collections::BTreeMap;

use helikodu_model::user_fields::{UserField, UserFields};

use super::super::{
    BackendError, ExtraColumns, ExtraValues, FieldName, FieldWords, TrackKey, TrackRow, Vocabulary,
    YourField,
};
use super::SampleLibrary;

/// The labels given to albums, by a rule on the album's number.
const ALBUM_LABELS: &[(u64, u64, &str)] = &[
    (5, 0, "favourite"),
    (9, 1, "road trip"),
    (11, 2, "for the car"),
    (17, 3, "winter"),
];

/// The labels given to artists, by a rule on their name.
const ARTIST_LABELS: &[(u64, u64, &str)] = &[
    (6, 0, "late night"),
    (10, 3, "work"),
    (23, 5, "evening walk"),
];

/// The labels given to tracks themselves, by a rule on their number.
const TRACK_LABELS: &[(u64, u64, &str)] = &[
    (7, 0, "to sort"),
    (31, 4, "after hours"),
    (41, 6, "live"),
    (97, 8, "night drive"),
    (151, 9, "night shift"),
];

/// The moods the MOOD tags of the sample hold.
const MOODS: &[&str] = &["bright", "calm", "dark", "warm"];

/// A stable number for a name.
fn name_number(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Every label of `row`, own and inherited, in name order.
pub(super) fn labels_of(row: &TrackRow) -> Vec<&'static str> {
    let artist = row.artist.split(" & ").next().unwrap_or_default();
    let artist = name_number(artist);
    let mut labels: Vec<&'static str> = Vec::new();
    for (rules, number) in [
        (ALBUM_LABELS, row.album.0),
        (ARTIST_LABELS, artist),
        (TRACK_LABELS, row.key.0),
    ] {
        for (every, at, label) in rules {
            if number % every == *at && !labels.contains(label) {
                labels.push(label);
            }
        }
    }
    labels.sort_unstable();
    labels
}

/// The fields of the sample, as the user would define them.
fn definitions() -> Vec<UserField> {
    vec![
        UserField::computed("decade", "{left(year, 3)}0s"),
        UserField::computed("era", "{decade or \"unknown\"}"),
        UserField::tag("mood", "MOOD"),
    ]
}

/// Puts the fields of the sample in use, so that queries read their names.
pub(super) fn install_fields() {
    let (fields, _) = UserFields::build_lenient(&definitions());
    helikodu_model::user_fields::install(fields);
}

/// The value of the sample's field `name` for `row`; empty when it has none.
pub(super) fn field_value(row: &TrackRow, name: &str) -> String {
    match name {
        "decade" | "era" => match row.year {
            Some(year) => format!("{}0s", year.div_euclid(10)),
            None if name == "era" => "unknown".to_owned(),
            None => String::new(),
        },
        "mood" => mood_of(row).unwrap_or_default().to_owned(),
        _ => String::new(),
    }
}

/// The MOOD tag of `row`, which two tracks in three have.
pub(super) fn mood_of(row: &TrackRow) -> Option<&'static str> {
    (!row.key.0.is_multiple_of(3)).then(|| MOODS[(row.key.0 / 3) as usize % MOODS.len()])
}

impl SampleLibrary {
    pub(super) fn sample_fields(&self) -> Vec<YourField> {
        definitions()
            .into_iter()
            .map(|field| YourField {
                tag: match field.definition {
                    helikodu_model::user_fields::FieldDefinition::Tag { key } => Some(key),
                    helikodu_model::user_fields::FieldDefinition::Computed { .. } => None,
                },
                name: field.name,
            })
            .collect()
    }

    pub(super) fn sample_extra_values(
        &self,
        keys: &[TrackKey],
        columns: &ExtraColumns,
    ) -> Result<Vec<(TrackKey, ExtraValues)>, BackendError> {
        Ok(keys
            .iter()
            .filter_map(|key| {
                let row = self.track(*key)?;
                let labels = if columns.labels {
                    labels_of(&row).join(", ")
                } else {
                    String::new()
                };
                let fields = columns
                    .fields
                    .iter()
                    .map(|name: &FieldName| (*name, field_value(&row, name.as_str())))
                    .collect();
                Some((*key, ExtraValues { labels, fields }))
            })
            .collect())
    }

    pub(super) fn sample_vocabulary(&self) -> Result<Vocabulary, BackendError> {
        let mut labels: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut decade = None;
        for index in 1..=self.tracks.len() {
            let Some(row) = self.track_row_unpatched(TrackKey(index as u64)) else {
                continue;
            };
            for label in labels_of(&row) {
                *labels.entry(label).or_default() += 1;
            }
            if decade.is_none() && row.year.is_some() {
                decade = Some(field_value(&row, "decade"));
            }
        }
        let fields = self
            .sample_fields()
            .into_iter()
            .map(|field| match field.tag {
                Some(_) => FieldWords {
                    field,
                    example: Some(MOODS[1].to_owned()),
                    values: MOODS.iter().map(|m| (*m).to_owned()).collect(),
                },
                None => FieldWords {
                    field,
                    example: decade.clone(),
                    values: Vec::new(),
                },
            })
            .collect();
        Ok(Vocabulary {
            labels: labels
                .into_iter()
                .map(|(name, tracks)| (name.to_owned(), tracks))
                .collect(),
            fields,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_their_rules_in_name_order() {
        let row = TrackRow {
            key: TrackKey(7),
            album: crate::backend::AlbumKey(5),
            artist: "Mari Kask & Okto".into(),
            year: Some(2018),
            ..TrackRow::default()
        };
        let labels = labels_of(&row);
        assert!(labels.contains(&"favourite"));
        assert!(labels.contains(&"to sort"));
        assert!(labels.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(field_value(&row, "decade"), "2010s");
        assert_eq!(field_value(&row, "era"), "2010s");
        assert_eq!(field_value(&row, "mood"), "dark");
    }

    #[test]
    fn the_sample_gives_column_values_and_completion_words() {
        use crate::backend::LibrarySource;
        let library = SampleLibrary::new(40);
        let keys = [TrackKey(1), TrackKey(2), TrackKey(99_999)];
        let columns = ExtraColumns {
            labels: true,
            fields: vec![FieldName::new("decade")],
        };
        let values = library.extra_values(&keys, &columns).unwrap();
        assert_eq!(values.len(), 2);
        assert!(
            values[0]
                .1
                .field(FieldName::new("decade"))
                .unwrap()
                .ends_with("0s")
        );
        let words = library.vocabulary().unwrap();
        assert!(words.labels.iter().any(|(l, n)| l == "favourite" && *n > 0));
        assert_eq!(words.fields.len(), 3);
        assert_eq!(words.fields[2].values.len(), MOODS.len());
    }
}
