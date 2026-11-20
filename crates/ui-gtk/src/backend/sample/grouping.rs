// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The track table's groups over the invented library, by the rules the real
//! library follows: the groups in the order of their values (case aside,
//! numbers by their value), the tracks without a value first, the tracks of a
//! group in the order of the list. The sample has three fields of the user's:
//! `decade` and `era` (computed from the year) and `mood` (a tag), and the
//! labels of its label store.

use std::cmp::Ordering;
use std::collections::HashMap;

use helikodu_model::expression::{FieldMap, Template, compare_keys};
use helikodu_model::query::Field;

use super::SampleLibrary;
use crate::backend::{BackendError, TrackGroup, TrackGrouping, TrackKey, TrackRow};

/// The names of the sample's fields of the user's.
pub(super) const FIELDS: [&str; 3] = ["decade", "era", "mood"];

/// The decade of a year, as the field `decade` gives it: `1990s`.
fn decade(row: &TrackRow) -> String {
    row.year
        .filter(|y| *y > 0)
        .map(|y| format!("{}0s", y / 10))
        .unwrap_or_default()
}

/// The invented tag MOOD: two values, and none for every third track.
fn mood(row: &TrackRow) -> String {
    match row.key.0 % 3 {
        0 => "calm".to_owned(),
        1 => "bright".to_owned(),
        _ => String::new(),
    }
}

/// The values an expression reads of `row`.
fn values(row: &TrackRow, album_artist: &str, labels: Vec<String>) -> FieldMap {
    let mut map = FieldMap::default();
    map.set(Field::Title, row.title.clone());
    map.set(Field::Artist, row.artist.clone());
    map.set(Field::AlbumArtist, album_artist.to_owned());
    map.set(Field::Album, row.album_title.clone());
    if let Some(year) = row.year {
        map.set(Field::Year, year.to_string());
    }
    map.set(Field::Genre, row.genre.clone());
    if let Some(codec) = &row.format.codec {
        map.set(Field::Codec, codec.to_lowercase());
    }
    if let Some(number) = row.number {
        map.set(Field::Track, number.to_string());
    }
    map.set(Field::Plays, row.plays.to_string());
    map.set_list(Field::Label, labels);
    map
}

/// `values` in groups, as the module documentation describes.
fn group(values: impl IntoIterator<Item = (TrackKey, String)>) -> Vec<TrackGroup> {
    let mut groups: Vec<TrackGroup> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (key, value) in values {
        let value = value.trim();
        match index.get(&value.to_lowercase()) {
            Some(&at) => groups[at].keys.push(key),
            None => {
                index.insert(value.to_lowercase(), groups.len());
                groups.push(TrackGroup {
                    value: value.to_owned(),
                    keys: vec![key],
                });
            }
        }
    }
    groups.sort_by(|a, b| match (a.value.is_empty(), b.value.is_empty()) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => compare_keys(&a.value, &b.value),
    });
    groups
}

impl SampleLibrary {
    pub(super) fn sample_groups(
        &self,
        keys: &[TrackKey],
        by: &TrackGrouping,
    ) -> Result<Vec<TrackGroup>, BackendError> {
        let template = match by {
            TrackGrouping::Expression(text) => {
                Some(Template::parse(text).map_err(|e| BackendError(e.to_string()))?)
            }
            TrackGrouping::Field(name) if !FIELDS.contains(&name.as_str()) => {
                return Err(BackendError(format!("there is no field {name}")));
            }
            _ => None,
        };
        let value = |row: &TrackRow| -> String {
            let album_artist = || {
                self.get(row.album)
                    .map(|a| a.row.artist.clone())
                    .unwrap_or_default()
            };
            if let Some(template) = &template {
                let labels = self.labels_of_track(row.key);
                return template.evaluate(&values(row, &album_artist(), labels));
            }
            match by {
                TrackGrouping::AlbumArtist => album_artist(),
                TrackGrouping::Album => row.album_title.clone(),
                TrackGrouping::Genre => row.genre.split(", ").next().unwrap_or("").to_owned(),
                TrackGrouping::Year => row.year.map(|y| y.to_string()).unwrap_or_default(),
                TrackGrouping::Format => row.format.codec.clone().unwrap_or_default(),
                TrackGrouping::Label => self
                    .labels_of_track(row.key)
                    .into_iter()
                    .next()
                    .unwrap_or_default(),
                TrackGrouping::Field(name) => match name.as_str() {
                    "decade" => decade(row),
                    "era" => {
                        let decade = decade(row);
                        if decade.is_empty() {
                            "unknown".to_owned()
                        } else {
                            decade
                        }
                    }
                    _ => mood(row),
                },
                TrackGrouping::Expression(_) => String::new(),
            }
        };
        Ok(group(
            keys.iter()
                .filter_map(|key| self.track(*key))
                .map(|row| (row.key, value(&row))),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_are_ordered_by_value_with_no_value_first() {
        let groups = group([
            (TrackKey(1), "2010s".to_owned()),
            (TrackKey(2), String::new()),
            (TrackKey(3), "1990s".to_owned()),
            (TrackKey(4), "2010S".to_owned()),
        ]);
        let shown: Vec<(&str, usize)> = groups
            .iter()
            .map(|g| (g.value.as_str(), g.keys.len()))
            .collect();
        assert_eq!(shown, [("", 1), ("1990s", 1), ("2010s", 2)]);
    }

    #[test]
    fn the_sample_groups_by_its_fields_and_expressions() {
        let library = SampleLibrary::new(20);
        let keys: Vec<TrackKey> = library
            .tracks
            .iter()
            .enumerate()
            .map(|(i, _)| TrackKey(i as u64 + 1))
            .collect();
        let by_decade = library
            .sample_groups(&keys, &TrackGrouping::Field("decade".into()))
            .unwrap();
        assert!(by_decade.len() > 1);
        assert_eq!(
            by_decade.iter().map(|g| g.keys.len()).sum::<usize>(),
            keys.len()
        );
        assert!(by_decade.windows(2).all(|w| w[0].value < w[1].value));
        let by_mood = library
            .sample_groups(&keys, &TrackGrouping::Field("mood".into()))
            .unwrap();
        assert_eq!(by_mood[0].value, "");
        let by_expression = library
            .sample_groups(&keys, &TrackGrouping::Expression("{left(title, 1)}".into()))
            .unwrap();
        assert!(!by_expression.is_empty());
        assert!(
            library
                .sample_groups(&keys, &TrackGrouping::Expression("{left(".into()))
                .is_err()
        );
    }
}
