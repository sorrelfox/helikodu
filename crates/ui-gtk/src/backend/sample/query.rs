// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The query language over the invented library, for the fields its tracks
//! have: titles, artists, albums, genres, formats, years, plays, ratings,
//! lengths, when they were added, labels and the sample's own fields. Other
//! fields match nothing. Saved searches are kept in memory.

use std::cmp::Ordering;
use std::time::{Duration, SystemTime};

use helikodu_model::query::{
    AgeUnit, Expr, Field, Flag, LimitUnit, Operator, Presence, Query, SortBy, Value,
};

use super::super::{SavedSearchKey, TrackRow};
use super::lower;

/// A saved search of the sample library.
#[derive(Debug, Clone)]
pub(super) struct SampleSearch {
    pub(super) key: SavedSearchKey,
    pub(super) name: String,
    pub(super) text: String,
}

/// The saved searches the sample library starts with.
pub(super) fn sample_searches() -> Vec<SampleSearch> {
    [
        ("Added this month", "added<30d"),
        ("Never played", "plays=0"),
        ("Jazz without fusion", "genre:jazz -genre:fusion"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (name, text))| SampleSearch {
        key: SavedSearchKey(i as u64 + 1),
        name: name.to_owned(),
        text: text.to_owned(),
    })
    .collect()
}

/// The labels of a track, as the sample's label store gives them.
pub(super) type LabelsOf<'a> = &'a dyn Fn(&TrackRow) -> Vec<String>;

/// The tracks of `rows` that `query` selects, in its order, cut to its limit.
pub(super) fn select(
    query: &Query,
    rows: Vec<TrackRow>,
    now: SystemTime,
    labels: LabelsOf<'_>,
) -> Vec<TrackRow> {
    let mut rows: Vec<TrackRow> = rows
        .into_iter()
        .filter(|row| {
            query
                .filter
                .as_ref()
                .is_none_or(|e| holds(e, row, now, labels))
        })
        .collect();
    if !query.sort.is_empty() {
        rows.sort_by(|a, b| {
            query
                .sort
                .iter()
                .filter_map(|key| match &key.by {
                    SortBy::Field(field) => {
                        let order = compare(field, a, b, labels);
                        Some(if key.descending {
                            order.reverse()
                        } else {
                            order
                        })
                    }
                    _ => None,
                })
                .find(|o| *o != Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        });
    }
    if let Some(limit) = query.limit
        && limit.unit == LimitUnit::Tracks
    {
        rows.truncate(limit.amount as usize);
    }
    rows
}

fn text_of<'a>(field: &Field, row: &'a TrackRow) -> Option<&'a str> {
    Some(match field {
        Field::Title => &row.title,
        Field::Artist | Field::AlbumArtist => &row.artist,
        Field::Album => &row.album_title,
        Field::Genre => &row.genre,
        Field::Codec => row.format.codec.as_deref().unwrap_or_default(),
        Field::Path | Field::Filename | Field::Folder => row
            .path
            .as_deref()
            .and_then(|p| p.to_str())
            .unwrap_or_default(),
        _ => return None,
    })
}

/// The values of the fields the library keeps beside the row: the labels, the
/// computed fields and the MOOD tag of the sample.
fn values_of(field: &Field, row: &TrackRow, labels: LabelsOf<'_>) -> Option<Vec<String>> {
    Some(match field {
        Field::Label => labels(row),
        Field::Computed(name) => vec![super::lists::field_value(row, name)],
        Field::Tag(tag) if tag == "MOOD" => super::lists::mood_of(row)
            .map(str::to_owned)
            .into_iter()
            .collect(),
        _ => return None,
    })
}

fn number_of(field: &Field, row: &TrackRow, now: SystemTime) -> Option<f64> {
    match field {
        Field::Year => row.year.map(f64::from),
        Field::Plays => Some(f64::from(row.plays)),
        Field::Rating => row.rating.map(f64::from),
        Field::Duration => row.duration.map(|d| d.as_secs_f64()),
        Field::Track => row.number.map(f64::from),
        Field::Disc => row.disc.map(f64::from),
        Field::SampleRate => row.format.sample_rate.map(f64::from),
        Field::BitDepth => row.format.bits.map(f64::from),
        // Seconds ago, so that an age compares as it reads.
        Field::Added => row
            .added
            .and_then(|added| now.duration_since(added).ok())
            .map(|ago| ago.as_secs_f64()),
        _ => None,
    }
}

fn compare(field: &Field, a: &TrackRow, b: &TrackRow, labels: LabelsOf<'_>) -> Ordering {
    let now = SystemTime::now();
    if let (Some(x), Some(y)) = (text_of(field, a), text_of(field, b)) {
        return lower(x).cmp(&lower(y));
    }
    if let (Some(x), Some(y)) = (values_of(field, a, labels), values_of(field, b, labels)) {
        return x
            .first()
            .map(|v| lower(v))
            .cmp(&y.first().map(|v| lower(v)));
    }
    // Missing values last; for ages, later is smaller.
    match (number_of(field, a, now), number_of(field, b, now)) {
        (Some(x), Some(y)) => {
            let order = x.partial_cmp(&y).unwrap_or(Ordering::Equal);
            if *field == Field::Added {
                order.reverse()
            } else {
                order
            }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn holds(expr: &Expr, row: &TrackRow, now: SystemTime, labels: LabelsOf<'_>) -> bool {
    match expr {
        Expr::All(all) => all.iter().all(|e| holds(e, row, now, labels)),
        Expr::Any(any) => any.iter().any(|e| holds(e, row, now, labels)),
        Expr::Not(e) => !holds(e, row, now, labels),
        Expr::Text(text) => {
            let text = lower(text);
            [&row.title, &row.artist, &row.album_title, &row.genre]
                .iter()
                .any(|field| lower(field).contains(&text))
        }
        Expr::Has(Presence::Field(field)) => {
            text_of(field, row).is_some_and(|t| !t.is_empty())
                || values_of(field, row, labels).is_some_and(|v| v.iter().any(|t| !t.is_empty()))
                || number_of(field, row, now).is_some()
        }
        Expr::Is(Flag::Lossless) => lossless(row),
        Expr::Is(Flag::HiRes) => {
            lossless(row)
                && (row.format.bits.unwrap_or(0) > 16
                    || row.format.sample_rate.unwrap_or(0) > 48_000)
        }
        Expr::Compare(c) => {
            if let Some(values) = values_of(&c.field, row, labels) {
                let Value::Text(value) = &c.value else {
                    return false;
                };
                let value = lower(value);
                let hit = |text: &String| match c.op {
                    Operator::Equals | Operator::NotEquals => lower(text) == value,
                    _ => lower(text).contains(&value),
                };
                let found = values.iter().filter(|t| !t.is_empty()).any(hit);
                return found != (c.op == Operator::NotEquals);
            }
            if let Some(text) = text_of(&c.field, row) {
                let Value::Text(value) = &c.value else {
                    return false;
                };
                let (text, value) = (lower(text), lower(value));
                return match c.op {
                    Operator::Equals => text == value,
                    Operator::NotEquals => text != value,
                    _ => text.contains(&value),
                };
            }
            let Some(x) = number_of(&c.field, row, now) else {
                return false;
            };
            match &c.value {
                Value::Range(low, high) => match (number(low), number(high)) {
                    (Some(low), Some(high)) => low <= x && x <= high,
                    _ => false,
                },
                value => number(value).is_some_and(|y| match c.op {
                    Operator::Contains | Operator::Equals => (x - y).abs() < 1e-9,
                    Operator::NotEquals => (x - y).abs() >= 1e-9,
                    Operator::Less => x < y,
                    Operator::LessOrEqual => x <= y,
                    Operator::Greater => x > y,
                    Operator::GreaterOrEqual => x >= y,
                    Operator::Matches => false,
                }),
            }
        }
        _ => false,
    }
}

/// A value as a number in the unit of [`number_of`].
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => Some(n.as_f64()),
        Value::Duration(ms) => Some(*ms as f64 / 1000.0),
        Value::Age(age) => {
            let unit = match age.unit {
                AgeUnit::Hours => 3600,
                AgeUnit::Days => 86_400,
                AgeUnit::Weeks => 7 * 86_400,
                AgeUnit::Months => 30 * 86_400,
                _ => 365 * 86_400,
            };
            Some(Duration::from_secs(u64::from(age.amount) * unit).as_secs_f64())
        }
        _ => None,
    }
}

fn lossless(row: &TrackRow) -> bool {
    matches!(
        row.format.codec.as_deref(),
        Some("FLAC" | "ALAC" | "WAV" | "AIFF" | "WavPack")
    )
}
