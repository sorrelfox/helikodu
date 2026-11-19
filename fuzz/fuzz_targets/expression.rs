// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads arbitrary text as a template of the expression language, as plain
//! text and as a path, and evaluates it with arbitrary field values. Any panic
//! or stack overflow is a bug, and so is an error span outside the text, or a
//! value or result longer than the limit. Every text that the naming patterns
//! of before read (kept in `legacy.rs` beside the language) must read the same
//! now: the same fields and the same pieces for the same values.
//!
//! The input is split at the byte 0x01: the template, then the values of the
//! fields in [`FIELDS`] in turn, each split at 0x02 into several values. The
//! first byte picks the track and disc counts.

#![no_main]

use helikodu_model::expression::{FieldMap, MAX_TEXT_BYTES, Piece, Template};
use helikodu_model::query::Field;
use libfuzzer_sys::fuzz_target;

// The old reader, which names these two from the module that includes it.
#[allow(unused_imports)]
use helikodu_model::expression::FieldValues;

#[path = "../../crates/model/src/expression/legacy.rs"]
#[allow(dead_code, unreachable_pub)]
mod legacy;

const FIELDS: [Field; 9] = [
    Field::Title,
    Field::Artist,
    Field::AlbumArtist,
    Field::Album,
    Field::Track,
    Field::Disc,
    Field::Year,
    Field::Duration,
    Field::Composer,
];

fn pieces(template: &Template, values: &FieldMap) -> Vec<legacy::Piece> {
    let mut out = Vec::new();
    template.pieces(values, &mut |piece| {
        out.push(match piece {
            Piece::Text(t) => legacy::Piece::Text(t.to_owned()),
            Piece::Value(v) => {
                if let Some(v) = &v {
                    assert!(!v.is_empty() && v.len() <= MAX_TEXT_BYTES);
                }
                legacy::Piece::Value(v)
            }
            Piece::Folder => legacy::Piece::Folder,
        });
    });
    out
}

fuzz_target!(|data: &[u8]| {
    let Some((&counts, rest)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(rest);
    let mut split = text.split('\u{1}');
    let template_text = split.next().unwrap_or_default();
    let mut values = FieldMap {
        track_count: (counts & 0x0f != 0).then(|| u32::from(counts & 0x0f) * 25),
        disc_count: (counts & 0xf0 != 0).then(|| u32::from(counts >> 4)),
        ..FieldMap::default()
    };
    for (field, value) in FIELDS.into_iter().zip(split) {
        values.set_list(field, value.split('\u{2}').map(str::to_owned).collect());
    }
    let chars = template_text.chars().count();
    for path in [false, true] {
        let read = if path {
            Template::parse_path(template_text)
        } else {
            Template::parse(template_text)
        };
        let template = match read {
            Ok(template) => template,
            Err(error) => {
                assert!(error.start <= error.end && error.end <= chars, "{error:?}");
                let _ = error.to_string();
                if path {
                    assert!(
                        legacy::Pattern::parse(template_text).is_none(),
                        "{template_text:?} read before: {error}"
                    );
                }
                continue;
            }
        };
        let _ = template.fields();
        let evaluated = template.evaluate(&values);
        assert!(evaluated.len() <= MAX_TEXT_BYTES);
        let new = pieces(&template, &values);
        if path && let Some(old) = legacy::Pattern::parse(template_text) {
            assert_eq!(template.fields(), old.fields(), "{template_text:?}");
            assert_eq!(new, old.pieces(&values), "{template_text:?}");
        }
    }
});
