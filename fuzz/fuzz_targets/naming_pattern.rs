// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads arbitrary text as a naming pattern and renders it with arbitrary field
//! values in every character set. Any panic or stack overflow is a bug, and so is
//! an error span outside the pattern, or a rendered name that is empty, `.` or
//! `..`, holds a `/` or a NUL, is longer than a file system allows, or (outside
//! the as-is set) holds a character Windows forbids or a folder name ending in a
//! dot or a space.
//!
//! The input is split at the byte 0x01: the pattern, the word for an empty field,
//! then the values of the fields in [`FIELDS`] in turn. The first byte picks the
//! track and disc counts.

#![no_main]

use helikodu_meta::naming::{CharacterSet, FieldMap, MAX_NAME_BYTES, NamingPattern};
use helikodu_model::query::Field;
use libfuzzer_sys::fuzz_target;

const FIELDS: [Field; 8] = [
    Field::Title,
    Field::Artist,
    Field::AlbumArtist,
    Field::Album,
    Field::Track,
    Field::Disc,
    Field::Year,
    Field::Genre,
];

fuzz_target!(|data: &[u8]| {
    let Some((&counts, rest)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(rest);
    let mut pieces = text.split('\u{1}');
    let pattern_text = pieces.next().unwrap_or_default();
    let unknown = pieces.next().unwrap_or("Unknown");
    let mut values = FieldMap {
        track_count: (counts & 0x0f != 0).then(|| u32::from(counts & 0x0f) * 25),
        disc_count: (counts & 0xf0 != 0).then(|| u32::from(counts >> 4)),
        ..FieldMap::default()
    };
    for (field, value) in FIELDS.into_iter().zip(pieces) {
        values.set(field, value);
    }
    let chars = pattern_text.chars().count();
    let pattern = match NamingPattern::parse(pattern_text) {
        Ok(pattern) => pattern,
        Err(error) => {
            assert!(error.start <= error.end && error.end <= chars, "{error:?}");
            let _ = error.to_string();
            return;
        }
    };
    let _ = pattern.fields();
    for set in CharacterSet::ALL {
        for extension in ["", ".flac", ".opus"] {
            let names = pattern.render(&values, set, unknown, extension);
            assert!(!names.is_empty());
            let count = names.len();
            for (i, name) in names.iter().enumerate() {
                let last = i + 1 == count;
                assert!(!name.is_empty(), "{names:?}");
                assert!(name != "." && name != "..", "{names:?}");
                assert!(!name.contains('/') && !name.contains('\0'), "{names:?}");
                let room = if last {
                    MAX_NAME_BYTES - extension.len()
                } else {
                    MAX_NAME_BYTES
                };
                assert!(name.len() <= room, "{} bytes: {name:?}", name.len());
                if set != CharacterSet::AsIs {
                    assert!(
                        !name.chars().any(|c| c.is_control()
                            || matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*')),
                        "{name:?}"
                    );
                    if !last {
                        assert!(!name.ends_with('.') && !name.ends_with(' '), "{name:?}");
                    }
                }
                if set == CharacterSet::Ascii {
                    assert!(name.is_ascii(), "{name:?}");
                }
            }
            let path = pattern.render_path(&values, set, unknown, extension);
            assert!(path.is_relative());
            assert_eq!(path.components().count(), count);
        }
    }
});
