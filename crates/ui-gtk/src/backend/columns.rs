// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The columns of a track list beyond its tags (the labels and the user's own
//! fields), and the words the search completion offers beyond the built-in
//! fields: the labels and the user's fields with an example and their values.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

/// The name of one of the user's fields as a track column holds it: a copy that
/// lives as long as the program, so that a column stays a small `Copy` value. A
/// name is kept once however often it is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldName(&'static str);

/// `text` as a copy that lives as long as the program, made once.
fn keep(text: String) -> &'static str {
    static KEPT: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    let kept = kept.get_or_insert_with(HashSet::new);
    if let Some(known) = kept.get(text.as_str()) {
        return known;
    }
    let leaked: &'static str = Box::leak(text.into_boxed_str());
    kept.insert(leaked);
    leaked
}

impl FieldName {
    /// The name `name`, in lower case.
    pub fn new(name: &str) -> Self {
        Self(keep(name.trim().to_lowercase()))
    }

    /// The name.
    pub fn as_str(self) -> &'static str {
        self.0
    }

    /// A name for an action or a widget: `field-` and the name, each character
    /// other than an ASCII letter or digit as `-`.
    pub fn id(self) -> &'static str {
        let name: String = self
            .0
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        keep(format!("field-{name}"))
    }
}

/// A field the user defined.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct YourField {
    /// Its name, in lower case.
    pub name: String,
    /// The tag a custom field stands for (`MOOD`); `None` for a computed field.
    pub tag: Option<String>,
}

impl YourField {
    /// Whether an expression gives its value.
    pub fn is_computed(&self) -> bool {
        self.tag.is_none()
    }
}

/// The columns beyond the track's row that a track list shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExtraColumns {
    /// The labels, own and inherited.
    pub labels: bool,
    /// The user's fields, by name.
    pub fields: Vec<FieldName>,
}

impl ExtraColumns {
    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        !self.labels && self.fields.is_empty()
    }
}

/// What the columns of [`ExtraColumns`] show for one track.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExtraValues {
    /// The labels in name order, joined with `, `; empty when they were not
    /// asked for or there are none.
    pub labels: String,
    /// The value of each field asked for, by name.
    pub fields: Vec<(FieldName, String)>,
}

impl ExtraValues {
    /// The value of the field `name`, if it was asked for.
    pub fn field(&self, name: FieldName) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, value)| value.as_str())
    }
}

/// One of the user's fields as the search completion offers it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FieldWords {
    /// The field.
    pub field: YourField,
    /// One of its values, for the example; `None` when no track has one.
    pub example: Option<String>,
    /// Its values when they are few (a custom field only), in name order.
    pub values: Vec<String>,
}

/// What the search completion offers beyond the built-in fields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Vocabulary {
    /// Every label in name order, with the number of tracks it covers.
    pub labels: Vec<(String, usize)>,
    /// The user's fields, in their order.
    pub fields: Vec<FieldWords>,
}

/// How many values of a custom field the completion offers; a field with more
/// offers none.
pub const FEW_VALUES: usize = 20;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_names_are_kept_once_in_lower_case() {
        let a = FieldName::new("Decade");
        let b = FieldName::new(" decade ");
        assert_eq!(a, b);
        assert!(std::ptr::eq(a.as_str(), b.as_str()));
        assert_eq!(a.as_str(), "decade");
        assert_ne!(a, FieldName::new("era"));
        assert_eq!(FieldName::new("by_ear").id(), "field-by-ear");
    }
}
