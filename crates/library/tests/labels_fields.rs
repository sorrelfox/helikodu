// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Labels of tracks, albums and artists, and the fields the user defines, through
//! the public API.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use helikodu_library::{
    DatabaseConfig, DefinitionPart, DefinitionProblem, FieldDefinition, FieldError, LabelChange,
    LabelError, LabelTarget, LabelUndo, Library, QueryRequest, RootId, TrackChange, UserField,
    parse_query, print_query,
};
use helikodu_model::expression::Template;
use helikodu_model::library::TrackId;
use helikodu_model::query::Field;
use support::{library, tagged};

/// The user's fields are in use for the whole program: the tests that open a
/// library take turns.
static FIELDS: Mutex<()> = Mutex::new(());

fn turn() -> MutexGuard<'static, ()> {
    FIELDS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn add(
    library: &Library,
    root: RootId,
    path: &str,
    title: &str,
    artist: &str,
    album: &str,
) -> TrackId {
    library
        .upsert_track(tagged(root, path, path, title, artist, album))
        .unwrap()
        .track
}

fn found(library: &Library, query: &str) -> Vec<TrackId> {
    let mut ids = library
        .query_track_ids(&QueryRequest::new(parse_query(query).unwrap()))
        .unwrap();
    ids.sort();
    ids
}

fn sorted(mut ids: Vec<TrackId>) -> Vec<TrackId> {
    ids.sort();
    ids
}

fn album_of(library: &Library, track: TrackId) -> helikodu_library::AlbumId {
    let request =
        QueryRequest::new(parse_query("").unwrap()).page(helikodu_library::Page::new(0, 100));
    library
        .query_tracks(&request)
        .unwrap()
        .items
        .into_iter()
        .find(|t| t.id == track)
        .unwrap()
        .album
        .unwrap()
}

#[test]
fn labels_are_given_listed_renamed_merged_and_taken() {
    let _turn = turn();
    let (library, root) = library();
    let a = add(&library, root, "/music/a.flac", "A", "Band", "One");
    let b = add(&library, root, "/music/b.flac", "B", "Band", "One");
    let c = add(&library, root, "/music/c.flac", "C", "Other", "Two");

    let tracks = LabelTarget::Tracks(vec![a, b]);
    assert_eq!(library.add_label(&tracks, "  Late   night ").unwrap(), 2);
    // The same label in another case: nothing new, the newer spelling.
    assert_eq!(library.add_label(&tracks, "late NIGHT").unwrap(), 0);
    library
        .add_label(&LabelTarget::Tracks(vec![c]), "Jazz")
        .unwrap();
    let list = library.labels().unwrap();
    let names: Vec<(&str, usize, usize)> = list
        .iter()
        .map(|l| (l.name.as_str(), l.tracks, l.own_tracks))
        .collect();
    assert_eq!(names, [("Jazz", 1, 1), ("late NIGHT", 2, 2)]);

    let of_a = library.labels_of(&LabelTarget::Tracks(vec![a])).unwrap();
    assert_eq!(of_a.own, ["late NIGHT"]);
    assert!(of_a.inherited.is_empty());

    // Renaming to another label's name fails; merging joins them.
    assert!(matches!(
        library.rename_label("late night", "jazz"),
        Err(LabelError::Exists(name)) if name == "Jazz"
    ));
    library.rename_label("late night", "Night").unwrap();
    library.merge_labels("night", "jazz").unwrap();
    let list = library.labels().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].name.as_str(), list[0].tracks), ("Jazz", 3));
    assert!(matches!(
        library.merge_labels("jazz", "Jazz"),
        Err(LabelError::SameLabel)
    ));
    assert!(matches!(
        library.rename_label("nothing", "x"),
        Err(LabelError::UnknownLabel(_))
    ));
    assert!(matches!(
        library.add_label(&tracks, " "),
        Err(LabelError::EmptyName)
    ));

    // A label nothing has any more goes.
    assert_eq!(
        library
            .remove_label(&LabelTarget::Tracks(vec![a, b, c]), "JAZZ")
            .unwrap(),
        3
    );
    assert!(library.labels().unwrap().is_empty());
}

#[test]
fn labels_stay_through_moves_and_rescans() {
    let _turn = turn();
    let (library, root) = library();
    let write = library
        .upsert_track(tagged(
            root,
            "/music/old/a.flac",
            "content a",
            "Song",
            "Band",
            "Album",
        ))
        .unwrap();
    library
        .add_label(&LabelTarget::Tracks(vec![write.track]), "keep")
        .unwrap();

    // A watched move, then a rescan of the same file.
    library
        .move_file(root, "/music/old/a.flac", "/music/new/a.flac")
        .unwrap();
    library
        .upsert_track(tagged(
            root,
            "/music/new/a.flac",
            "content a",
            "Song",
            "Band",
            "Album",
        ))
        .unwrap();
    assert_eq!(found(&library, "label=keep"), [write.track]);

    // The file disappears and is found again under another name.
    library
        .mark_file_missing("/music/new/a.flac", SystemTime::now())
        .unwrap();
    let again = library
        .upsert_track(tagged(
            root,
            "/music/b.flac",
            "content a",
            "Song",
            "Band",
            "Album",
        ))
        .unwrap();
    assert_eq!(again.change, TrackChange::Moved);
    assert_eq!(found(&library, "label=keep"), [again.track]);

    // It leaves the library and comes back: its identity, and its label, return.
    library.remove_file("/music/b.flac").unwrap();
    assert!(found(&library, "label=keep").is_empty());
    let back = library
        .upsert_track(tagged(
            root,
            "/music/c.flac",
            "content a",
            "Song",
            "Band",
            "Album",
        ))
        .unwrap();
    assert_eq!(back.identity, write.identity);
    assert_eq!(found(&library, "label=keep"), [back.track]);
}

#[test]
fn album_and_artist_labels_count_for_their_tracks() {
    let _turn = turn();
    let (library, root) = library();
    let a = add(&library, root, "/music/one/a.flac", "A", "Band", "One");
    let b = add(&library, root, "/music/one/b.flac", "B", "Band", "One");
    let c = add(&library, root, "/music/two/c.flac", "C", "Solo", "Two");
    let d = add(&library, root, "/music/three/d.flac", "D", "Sólo", "Three");

    let album = album_of(&library, a);
    library
        .add_label(&LabelTarget::Album(album), "favourite album")
        .unwrap();
    // Names match without regard to case and accents.
    library
        .add_label(&LabelTarget::Artist("SOLO".into()), "solo work")
        .unwrap();
    assert!(matches!(
        library.add_label(&LabelTarget::Artist("Nobody".into()), "x"),
        Err(LabelError::UnknownArtist(_))
    ));
    library
        .add_label(&LabelTarget::Tracks(vec![a]), "opener")
        .unwrap();

    assert_eq!(found(&library, "label:favourite"), sorted(vec![a, b]));
    assert_eq!(found(&library, "label=\"solo work\""), sorted(vec![c, d]));
    assert_eq!(found(&library, "-has:label"), Vec::<TrackId>::new());
    assert_eq!(found(&library, "-label:favourite"), sorted(vec![c, d]));

    let of_a = library.labels_of(&LabelTarget::Tracks(vec![a])).unwrap();
    assert_eq!(of_a.own, ["opener"]);
    assert_eq!(of_a.inherited, ["favourite album"]);
    let of_album = library.labels_of(&LabelTarget::Album(album)).unwrap();
    assert_eq!(of_album.own, ["favourite album"]);
    let of_album = library.labels_of(&LabelTarget::AlbumOf(b)).unwrap();
    assert_eq!(of_album.own, ["favourite album"]);
    // The album of a track is labelled like the album itself.
    assert_eq!(
        library
            .add_label(&LabelTarget::AlbumOf(b), "favourite album")
            .unwrap(),
        0
    );

    let list = library.labels().unwrap();
    let counts: Vec<(&str, usize, usize, usize)> = list
        .iter()
        .map(|l| (l.name.as_str(), l.tracks, l.albums, l.artists))
        .collect();
    assert_eq!(
        counts,
        [
            ("favourite album", 2, 1, 0),
            ("opener", 1, 0, 0),
            ("solo work", 2, 0, 1)
        ]
    );

    // The album leaves the library and a rescan builds it anew: its label stays.
    library.remove_file("/music/one/a.flac").unwrap();
    library.remove_file("/music/one/b.flac").unwrap();
    let a2 = add(&library, root, "/music/one/a.flac", "A", "Band", "One");
    let b2 = add(&library, root, "/music/one/b.flac", "B", "Band", "One");
    assert_eq!(found(&library, "label:favourite"), sorted(vec![a2, b2]));

    // Exported: what has each label.
    let items = library.label_items().unwrap();
    assert_eq!(items[0].albums, [album_of(&library, a2)]);
    assert_eq!(items[2].artists, ["Solo"]);
    assert_eq!(items[1].tracks, [a2]);
}

#[test]
fn track_labels_give_the_column_every_label_in_name_order() {
    let _turn = turn();
    let (library, root) = library();
    let a = add(&library, root, "/music/one/a.flac", "A", "Band", "One");
    let b = add(&library, root, "/music/one/b.flac", "B", "Band", "One");
    let c = add(&library, root, "/music/two/c.flac", "C", "Solo", "Two");
    library
        .add_label(&LabelTarget::Tracks(vec![a]), "road trip")
        .unwrap();
    library
        .add_label(&LabelTarget::AlbumOf(a), "late night")
        .unwrap();
    library
        .add_label(&LabelTarget::Artist("Band".into()), "favourite")
        .unwrap();
    // Given to the track and to its album: listed once.
    library
        .add_label(&LabelTarget::Tracks(vec![b]), "late night")
        .unwrap();

    let gone = TrackId::new(999_999);
    let columns = library.track_labels(&[c, a, gone, b]).unwrap();
    assert_eq!(
        columns,
        [
            (c, Vec::<String>::new()),
            (
                a,
                vec![
                    "favourite".to_owned(),
                    "late night".to_owned(),
                    "road trip".to_owned()
                ]
            ),
            (b, vec!["favourite".to_owned(), "late night".to_owned()]),
        ]
    );
}

#[test]
fn labels_in_the_query_language() {
    let _turn = turn();
    let (library, root) = library();
    let a = add(&library, root, "/music/a.flac", "Alpha", "Band", "One");
    let b = add(&library, root, "/music/b.flac", "Beta", "Band", "One");
    let c = add(&library, root, "/music/c.flac", "Gamma", "Band", "One");
    library
        .add_label(&LabelTarget::Tracks(vec![a]), "Jazz Night")
        .unwrap();
    library
        .add_label(&LabelTarget::Tracks(vec![b]), "jazz")
        .unwrap();

    assert_eq!(found(&library, "label:jazz"), sorted(vec![a, b]));
    assert_eq!(found(&library, "label=jazz"), [b]);
    assert_eq!(found(&library, "label!=jazz"), sorted(vec![a, c]));
    assert_eq!(found(&library, "label~Night$"), [a]);
    assert_eq!(found(&library, "labels:NIGHT"), [a]);
    assert_eq!(found(&library, "has:label"), sorted(vec![a, b]));
    // Plain text finds labels too, as it finds genres.
    assert_eq!(found(&library, "night"), [a]);
    assert_eq!(found(&library, "alpha OR jazz"), sorted(vec![a, b]));
    // Sorted by the first label in name order; tracks without one last.
    let order = library
        .query_track_ids(&QueryRequest::new(parse_query("sort:label").unwrap()))
        .unwrap();
    assert_eq!(order, [b, a, c]);
    assert_eq!(
        print_query(&parse_query("Label=\"jazz night\"").unwrap()),
        "label=\"jazz night\""
    );

    // Expressions and naming patterns read every label.
    let template = Template::parse("{join(label, \" / \")}").unwrap();
    let values = library.evaluate_expression(&template, &[a, c]).unwrap();
    assert_eq!(values, [(a, "Jazz Night".to_owned()), (c, String::new())]);
}

#[test]
fn computed_fields_are_checked_evaluated_and_queried() {
    let _turn = turn();
    let (library, root) = library();
    let mut ids = Vec::new();
    for (i, year) in [1994, 1999, 2003, 0].iter().enumerate() {
        let mut record = tagged(
            root,
            &format!("/music/{i}.flac"),
            &format!("c{i}"),
            &format!("T{i}"),
            "Band",
            "Album",
        );
        if *year > 0 {
            record.tags.year = Some(*year);
        }
        ids.push(library.upsert_track(record).unwrap().track);
    }

    // Problems are found with their place.
    let error = |field| match library.define_field(field) {
        Err(FieldError::Definition(error)) => error,
        other => panic!("{other:?}"),
    };
    let clash = error(UserField::computed("Year", "{year}"));
    assert_eq!(clash.problem, DefinitionProblem::BuiltIn("year".into()));
    assert_eq!(clash.part, DefinitionPart::Name);
    let bad = error(UserField::computed("decade", "{left(year,3)0s"));
    assert_eq!(bad.part, DefinitionPart::Definition);
    // At the `0s` that does not belong after the call.
    assert_eq!((bad.start, bad.end), (13, 14));

    let stored = library
        .define_field(UserField::computed("Decade", "[{left(year,3)}0s]"))
        .unwrap();
    assert_eq!(stored.name, "decade");
    library
        .define_field(UserField::computed(
            "era",
            "{if(decade, decade, \"unknown\")}",
        ))
        .unwrap();
    assert_eq!(
        Field::from_name("DECADE"),
        Some(Field::Computed("decade".into()))
    );

    // Evaluated per track, in sorting and grouping and in naming patterns.
    let values = library.field_values("decade", &ids).unwrap();
    let texts: Vec<&str> = values.iter().map(|(_, v)| v.as_str()).collect();
    assert_eq!(texts, ["1990s", "1990s", "2000s", ""]);
    let groups = library
        .group_by_expression(&Template::parse("{era}").unwrap(), &ids)
        .unwrap();
    let names: Vec<(&str, usize)> = groups.iter().map(|(n, t)| (n.as_str(), t.len())).collect();
    assert_eq!(names, [("1990s", 2), ("2000s", 1), ("unknown", 1)]);
    let sorted_ids = library
        .sort_by_expression(&Template::parse("{decade}").unwrap(), &ids, true)
        .unwrap();
    assert_eq!(sorted_ids[0], ids[2]);
    let pattern = Template::parse_path("{era}/{title}").unwrap();
    assert_eq!(pattern.fields(), [Field::Year, Field::Title]);

    // In the query language, by their values.
    assert_eq!(
        found(&library, "decade=1990s"),
        sorted(vec![ids[0], ids[1]])
    );
    assert_eq!(found(&library, "decade:2000"), [ids[2]]);
    assert_eq!(found(&library, "-has:decade"), [ids[3]]);
    assert_eq!(found(&library, "era~^unk"), [ids[3]]);
    assert_eq!(
        found(&library, "decade!=1990s"),
        sorted(vec![ids[2], ids[3]])
    );
    // A query cannot sort by one.
    assert!(parse_query("sort:decade").is_err());

    // A field in use cannot go; the others can.
    assert!(matches!(
        library.remove_field("decade"),
        Err(FieldError::InUse { used_by, .. }) if used_by == "era"
    ));
    library.remove_field("era").unwrap();
    library.remove_field("decade").unwrap();
    assert!(Field::from_name("decade").is_none());
    assert!(matches!(
        library.remove_field("decade"),
        Err(FieldError::Unknown(_))
    ));
}

#[test]
fn custom_fields_name_tags_and_are_kept_with_the_library() {
    let _turn = turn();
    let folder = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("library-labels-fields");
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let config = DatabaseConfig::file(folder.join("library.db"));
    let library = Library::open(&config).unwrap();
    let root = library.add_root("/music").unwrap();
    let mut record = tagged(root, "/music/a.flac", "a", "A", "Band", "Album");
    record.catalog.other = vec![("MOOD".into(), "Calm".into())];
    let calm = library.upsert_track(record).unwrap().track;
    let other = add(&library, root, "/music/b.flac", "B", "Band", "Album");

    let stored = library
        .define_field(UserField::tag("mood", "TXXX:Mood"))
        .unwrap();
    assert_eq!(
        stored.definition,
        FieldDefinition::Tag { key: "MOOD".into() }
    );
    assert_eq!(Field::from_name("mood"), Some(Field::Tag("MOOD".into())));
    assert_eq!(found(&library, "mood:calm"), [calm]);
    assert_eq!(found(&library, "-has:mood"), [other]);
    // It reads as the tag does, and prints as the tag.
    assert_eq!(
        print_query(&parse_query("mood=calm").unwrap()),
        "tag:MOOD=calm"
    );
    let values = library
        .evaluate_expression(&Template::parse("{upper(mood)}").unwrap(), &[calm])
        .unwrap();
    assert_eq!(values[0].1, "CALM");
    // Its values, for the search completion: few, or none when there are more.
    let mut third = tagged(root, "/music/c.flac", "c", "C", "Band", "Album");
    third.catalog.other = vec![("MOOD".into(), "calm".into())];
    library.upsert_track(third).unwrap();
    let mut fourth = tagged(root, "/music/d.flac", "d", "D", "Band", "Album");
    fourth.catalog.other = vec![("MOOD".into(), "Bright".into())];
    library.upsert_track(fourth).unwrap();
    assert_eq!(
        library.tag_values("mood", 5).unwrap(),
        Some(vec![("Bright".to_owned(), 1), ("Calm".to_owned(), 2)])
    );
    assert_eq!(library.tag_values("MOOD", 1).unwrap(), None);
    assert_eq!(library.tag_values("NOTHING", 5).unwrap(), Some(Vec::new()));
    library
        .define_field(UserField::computed("feel", "{mood} {title}"))
        .unwrap();
    drop(library);

    // Opening the library again puts its fields in use again.
    helikodu_model::user_fields::install(Default::default());
    assert!(Field::from_name("mood").is_none());
    let library = Library::open(&config).unwrap();
    let names: Vec<String> = library
        .user_fields()
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(names, ["mood", "feel"]);
    assert_eq!(found(&library, "feel=\"calm a\""), [calm]);
    library.remove_field("feel").unwrap();
    library.remove_field("mood").unwrap();
}

/// How long a query on a computed field takes on a few thousand tracks: every track's
/// value is worked out first.
#[test]
fn computed_fields_query_a_few_thousand_tracks_quickly() {
    let _turn = turn();
    let (library, root) = library();
    let records: Vec<_> = (0..3000)
        .map(|i| {
            let mut record = tagged(
                root,
                &format!("/music/{}/{i}.flac", i % 50),
                &format!("content {i}"),
                &format!("Title {i}"),
                &format!("Artist {}", i % 40),
                &format!("Album {}", i % 300),
            );
            record.tags.year = Some(1960 + (i % 60));
            record
        })
        .collect();
    for chunk in records.chunks(500) {
        library.upsert_tracks(chunk.to_vec()).unwrap();
    }
    library
        .define_field(UserField::computed("decade", "{left(year,3)}0s"))
        .unwrap();
    let start = Instant::now();
    let count = library
        .query_count(&QueryRequest::new(parse_query("decade=1990s").unwrap()))
        .unwrap();
    let elapsed = start.elapsed();
    println!("decade=1990s over 3000 tracks: {count} found in {elapsed:?}");
    assert_eq!(count, 500);
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    library.remove_field("decade").unwrap();
}

/// Every label with the tracks that have it, for comparing states.
fn label_state(library: &Library) -> Vec<(i64, String, Vec<TrackId>)> {
    library
        .labels()
        .unwrap()
        .into_iter()
        .map(|l| {
            let name = l.name.clone();
            let mut ids = library
                .query_track_ids(&QueryRequest::new(
                    parse_query(&format!("label=\"{name}\"")).unwrap(),
                ))
                .unwrap();
            ids.sort();
            (l.id.get(), name, ids)
        })
        .collect()
}

fn give(target: &LabelTarget, name: &str) -> LabelChange {
    LabelChange::Give {
        target: target.clone(),
        name: name.to_owned(),
    }
}

#[test]
fn each_label_change_can_be_undone() {
    let _turn = turn();
    let (library, root) = library();
    let a = add(&library, root, "/music/one/a.flac", "A", "Band", "One");
    let b = add(&library, root, "/music/one/b.flac", "B", "Band", "One");
    let c = add(&library, root, "/music/two/c.flac", "C", "Solo", "Two");
    let album = album_of(&library, c);
    library
        .add_label(&LabelTarget::Tracks(vec![a, b]), "late night")
        .unwrap();
    library
        .add_label(&LabelTarget::Album(album), "road trip")
        .unwrap();
    library
        .add_label(&LabelTarget::Artist("Band".into()), "favourite")
        .unwrap();
    let before = label_state(&library);

    let changes = [
        give(&LabelTarget::Tracks(vec![c]), "late night"),
        give(&LabelTarget::Tracks(vec![a]), "brand new"),
        give(&LabelTarget::Tracks(vec![a]), "LATE NIGHT"),
        LabelChange::Take {
            target: LabelTarget::Tracks(vec![a, b]),
            name: "late night".into(),
        },
        LabelChange::Take {
            target: LabelTarget::Album(album),
            name: "road trip".into(),
        },
        LabelChange::Rename {
            name: "favourite".into(),
            new_name: "Loved".into(),
        },
        LabelChange::Merge {
            from: "late night".into(),
            into: "road trip".into(),
        },
        LabelChange::Delete {
            name: "favourite".into(),
        },
    ];
    for change in &changes {
        let (_, undo) = library.change_labels(change).unwrap();
        assert!(!undo.is_empty(), "{change:?}");
        assert_ne!(label_state(&library), before, "{change:?} changed nothing");
        library.undo_labels(&undo).unwrap();
        // The labels are back with their ids, spelling and tracks.
        assert_eq!(label_state(&library), before, "after undoing {change:?}");
    }
}

#[test]
fn changes_joined_are_undone_together() {
    let _turn = turn();
    let (library, root) = library();
    let a = add(&library, root, "/music/one/a.flac", "A", "Band", "One");
    let b = add(&library, root, "/music/one/b.flac", "B", "Band", "One");
    library
        .add_label(&LabelTarget::Tracks(vec![a]), "late night")
        .unwrap();
    let before = label_state(&library);

    // As in the Labels popover: several clicks, one Undo.
    let tracks = LabelTarget::Tracks(vec![a, b]);
    let mut undo = LabelUndo::default();
    for change in [
        give(&tracks, "late night"),
        give(&tracks, "night drive"),
        LabelChange::Take {
            target: tracks.clone(),
            name: "late night".into(),
        },
        give(&tracks, "late night"),
        LabelChange::Rename {
            name: "night drive".into(),
            new_name: "late drive".into(),
        },
        give(&LabelTarget::Tracks(vec![b]), "night drive"),
    ] {
        undo.absorb(library.change_labels(&change).unwrap().1);
    }
    assert_eq!(library.labels().unwrap().len(), 3);
    library.undo_labels(&undo).unwrap();
    assert_eq!(label_state(&library), before);

    // A label removed and made again in between comes back as it was.
    let mut undo = LabelUndo::default();
    for change in [
        LabelChange::Delete {
            name: "late night".into(),
        },
        give(&tracks, "late night"),
    ] {
        undo.absorb(library.change_labels(&change).unwrap().1);
    }
    library.undo_labels(&undo).unwrap();
    assert_eq!(label_state(&library), before);
}

#[test]
fn marks_tell_own_and_inherited_labels() {
    let _turn = turn();
    let (library, root) = library();
    let a = add(&library, root, "/music/one/a.flac", "A", "Band", "One");
    let b = add(&library, root, "/music/one/b.flac", "B", "Band", "One");
    let c = add(&library, root, "/music/two/c.flac", "C", "Solo", "Two");
    library
        .add_label(&LabelTarget::Tracks(vec![a, b]), "late night")
        .unwrap();
    library
        .add_label(&LabelTarget::Tracks(vec![a]), "night drive")
        .unwrap();
    library
        .add_label(&LabelTarget::AlbumOf(a), "night shift")
        .unwrap();
    library
        .add_label(&LabelTarget::Artist("Solo".into()), "solo")
        .unwrap();

    let marks = library
        .label_marks(&LabelTarget::Tracks(vec![a, b, c]))
        .unwrap();
    assert_eq!(marks.items, 3);
    let seen: Vec<(&str, usize, usize, usize)> = marks
        .labels
        .iter()
        .map(|m| (m.name.as_str(), m.own, m.from_album, m.from_artist))
        .collect();
    assert_eq!(
        seen,
        [
            ("late night", 2, 0, 0),
            ("night drive", 1, 0, 0),
            ("night shift", 0, 2, 0),
            ("solo", 0, 0, 1),
        ]
    );
    let album = library.label_marks(&LabelTarget::AlbumOf(b)).unwrap();
    assert_eq!(album.items, 1);
    assert_eq!(album.labels.len(), 1);
    assert_eq!(album.labels[0].name, "night shift");
    let nobody = library
        .label_marks(&LabelTarget::Artist("Nobody".into()))
        .unwrap();
    assert_eq!(nobody.items, 0);

    let items = library.label_items_of("NIGHT SHIFT").unwrap().unwrap();
    assert_eq!(items.albums, [album_of(&library, a)]);
    assert!(items.tracks.is_empty());
    assert!(library.label_items_of("none").unwrap().is_none());
}
