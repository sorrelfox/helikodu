// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lists of tracks in groups: by fields kept in the tables, by a label, by a
//! computed field and by an expression, through the public API.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use helikodu_library::{GroupBy, LabelTarget, Library, TrackGroup, UserField};
use helikodu_model::expression::Template;
use helikodu_model::library::TrackId;
use helikodu_model::query::Field;
use support::{library, tagged};

fn shown(groups: &[TrackGroup]) -> Vec<(&str, Vec<TrackId>)> {
    groups
        .iter()
        .map(|g| (g.value.as_str(), g.tracks.clone()))
        .collect()
}

/// Four tracks: years 2018, 1994, none and 2021, the third without a genre.
fn four(library: &Library) -> Vec<TrackId> {
    let root = library.add_root("/grouped").unwrap();
    let mut ids = Vec::new();
    for (i, (year, genre, artist)) in [
        (Some(2018), Some("Folk"), "Mari Kask"),
        (Some(1994), Some("folk"), "Lille Duo"),
        (None, None, "Mari Kask"),
        (Some(2021), Some("Jazz"), "Lille Duo"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut record = tagged(
            root,
            &format!("/grouped/{i}.flac"),
            &format!("g{i}"),
            &format!("T{i}"),
            artist,
            "Letters",
        );
        record.tags.year = year;
        record.tags.genres = genre.map(str::to_owned).into_iter().collect();
        record.audio.codec = Some(if i % 2 == 0 { "flac" } else { "mp3" }.to_owned());
        ids.push(library.upsert_track(record).unwrap().track);
    }
    ids
}

#[test]
fn fields_of_the_tables_group_in_value_order_with_no_value_first() {
    let (library, _) = library();
    let ids = four(&library);
    // The list as the columns sort it: the groups keep that order inside.
    let list = vec![ids[3], ids[2], ids[1], ids[0]];
    let by_year = library
        .group_tracks(&GroupBy::Field(Field::Year), &list)
        .unwrap();
    assert_eq!(
        shown(&by_year),
        [
            ("", vec![ids[2]]),
            ("1994", vec![ids[1]]),
            ("2018", vec![ids[0]]),
            ("2021", vec![ids[3]]),
        ]
    );
    let by_genre = library
        .group_tracks(&GroupBy::Field(Field::Genre), &list)
        .unwrap();
    // "Folk" and "folk" are one genre of the library, and one group.
    assert_eq!(
        shown(&by_genre),
        [
            ("", vec![ids[2]]),
            ("Folk", vec![ids[1], ids[0]]),
            ("Jazz", vec![ids[3]]),
        ]
    );
    let by_format = library
        .group_tracks(&GroupBy::Field(Field::Codec), &list)
        .unwrap();
    assert_eq!(
        shown(&by_format),
        [
            ("FLAC", vec![ids[2], ids[0]]),
            ("MP3", vec![ids[3], ids[1]])
        ]
    );
    let by_artist = library
        .group_tracks(&GroupBy::Field(Field::Artist), &list)
        .unwrap();
    assert_eq!(
        shown(&by_artist),
        [
            ("Lille Duo", vec![ids[3], ids[1]]),
            ("Mari Kask", vec![ids[2], ids[0]])
        ]
    );
    // A track the library does not have is left out.
    let unknown = library
        .group_tracks(&GroupBy::Field(Field::Year), &[TrackId::new(9_999)])
        .unwrap();
    assert!(unknown.is_empty());
}

#[test]
fn labels_computed_fields_and_expressions_group_too() {
    let (library, _) = library();
    let ids = four(&library);
    library
        .add_label(&LabelTarget::Tracks(vec![ids[0], ids[3]]), "late night")
        .unwrap();
    library
        .add_label(&LabelTarget::Tracks(vec![ids[3]]), "Calm")
        .unwrap();
    let by_label = library
        .group_tracks(&GroupBy::Field(Field::Label), &ids)
        .unwrap();
    // A track with several labels is in the group of the first by name.
    assert_eq!(
        shown(&by_label),
        [
            ("", vec![ids[1], ids[2]]),
            ("Calm", vec![ids[3]]),
            ("late night", vec![ids[0]]),
        ]
    );

    // A computed field: the order a query cannot sort by.
    library
        .define_field(UserField::computed("decade", "[{left(year,3)}0s]"))
        .unwrap();
    let field = Field::from_name("decade").unwrap();
    let by_decade = library.group_tracks(&GroupBy::Field(field), &ids).unwrap();
    assert_eq!(
        shown(&by_decade),
        [
            ("", vec![ids[2]]),
            ("1990s", vec![ids[1]]),
            ("2010s", vec![ids[0]]),
            ("2020s", vec![ids[3]]),
        ]
    );
    let expression = Template::parse("{if(year, \"dated\", \"\")}").unwrap();
    let by_expression = library
        .group_tracks(&GroupBy::Expression(expression), &ids)
        .unwrap();
    assert_eq!(
        shown(&by_expression),
        [("", vec![ids[2]]), ("dated", vec![ids[0], ids[1], ids[3]])]
    );
    library.remove_field("decade").unwrap();
}
