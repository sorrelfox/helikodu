// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A user's field changed in place: its definition, its kind and its name, with
//! the other fields checked against the change.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use helikodu_library::{DefinitionProblem, FieldDefinition, FieldError, UserField};
use helikodu_model::query::Field;
use support::library;

#[test]
fn a_field_is_replaced_in_its_place_and_renamed_when_nothing_needs_its_name() {
    let (library, _root) = library();
    library
        .define_field(UserField::computed("decade", "{left(year,3)}0s"))
        .unwrap();
    library
        .define_field(UserField::computed("era", "{decade or \"unknown\"}"))
        .unwrap();
    library
        .define_field(UserField::tag("mood", "MOOD"))
        .unwrap();

    // A new expression, in the same place.
    let stored = library
        .replace_field("decade", UserField::computed("decade", "{left(year,2)}"))
        .unwrap();
    assert_eq!(
        stored.definition,
        FieldDefinition::Computed {
            expression: "{left(year,2)}".into()
        }
    );
    let names = |library: &helikodu_library::Library| -> Vec<String> {
        library
            .user_fields()
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect()
    };
    assert_eq!(names(&library), ["decade", "era", "mood"]);

    // Another kind and another name, where no other field uses it.
    library
        .replace_field("mood", UserField::tag("Feel", "TXXX:FEEL"))
        .unwrap();
    assert_eq!(names(&library), ["decade", "era", "feel"]);
    assert_eq!(Field::from_name("feel"), Some(Field::Tag("FEEL".into())));
    assert!(Field::from_name("mood").is_none());

    // era needs the name decade.
    assert!(matches!(
        library.replace_field("decade", UserField::computed("years", "{year}")),
        Err(FieldError::InUse { field, used_by }) if field == "decade" && used_by == "era"
    ));
    // A field that comes back to itself.
    match library.replace_field("decade", UserField::computed("decade", "{era}")) {
        Err(FieldError::Definition(error)) => {
            assert!(matches!(error.problem, DefinitionProblem::Cycle(_)));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        library.replace_field("nothing", UserField::tag("nothing", "X")),
        Err(FieldError::Unknown(_))
    ));
    assert_eq!(names(&library), ["decade", "era", "feel"]);
}
