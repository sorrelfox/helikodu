// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Labels and the user's fields through the library facade.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use helikodu_service::library::tag_editing::{FieldKey, TagField};
use helikodu_service::library::{
    LabelError, LabelTarget, LibraryError, MusicLibrary, UserField, parse_query,
};

#[test]
fn labels_and_fields_through_the_facade() {
    let library = MusicLibrary::in_memory().unwrap();

    // Labels: an unknown artist fails with the label's error.
    assert!(matches!(
        library.add_label(&LabelTarget::Artist("Nobody".into()), "x"),
        Err(LibraryError::Label(LabelError::UnknownArtist(_)))
    ));
    assert_eq!(
        library
            .add_label(&LabelTarget::Tracks(Vec::new()), "unused")
            .unwrap(),
        0
    );
    assert!(library.labels().unwrap().is_empty());

    // A custom field reads in queries and writes as its tag.
    library
        .define_field(UserField::tag("mood", "----:com.apple.iTunes:MOOD"))
        .unwrap();
    library
        .define_field(UserField::tag("energy", "ENERGY"))
        .unwrap();
    assert!(parse_query("mood:calm energy=high").is_ok());
    assert_eq!(
        library.field_key("Mood"),
        Some(FieldKey::Known(TagField::Mood))
    );
    assert_eq!(
        library.field_key("energy"),
        Some(FieldKey::Custom("ENERGY".into()))
    );
    assert_eq!(library.field_key("title"), None);

    // A computed field with a problem is refused with its place.
    match library.define_field(UserField::computed("decade", "{left(yer,3)}0s")) {
        Err(LibraryError::Field(error)) => {
            let text = error.to_string();
            assert!(text.contains("yer"), "{text}");
        }
        other => panic!("{other:?}"),
    }
    let names: Vec<String> = library
        .user_fields()
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(names, ["mood", "energy"]);
    library.remove_field("mood").unwrap();
    library.remove_field("energy").unwrap();
}
