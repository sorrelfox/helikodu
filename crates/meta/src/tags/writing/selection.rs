// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The values a selection of files shares, field by field: one value for all, or
//! "mixed" with each value and how many files have it.

use std::collections::BTreeMap;

use crate::covers::CoverHash;
use crate::tags::rating::Rating;

use super::changes::{FieldKey, TagField};
use super::editable::EditableFile;
use super::values::field_values;

/// A value over several files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shared<T> {
    /// Every file has this value.
    Same(T),
    /// The files differ: each value with the number of files that have it, the
    /// most common first (in file order among equals).
    Mixed(Vec<(T, usize)>),
}

impl<T: PartialEq> Shared<T> {
    /// The shared value of `values`, or `None` when there are none.
    pub fn of(values: impl IntoIterator<Item = T>) -> Option<Self> {
        let mut counts: Vec<(T, usize)> = Vec::new();
        for value in values {
            match counts.iter_mut().find(|(v, _)| *v == value) {
                Some((_, n)) => *n += 1,
                None => counts.push((value, 1)),
            }
        }
        if counts.len() > 1 {
            counts.sort_by_key(|c| std::cmp::Reverse(c.1));
            return Some(Shared::Mixed(counts));
        }
        counts.pop().map(|(value, _)| Shared::Same(value))
    }

    /// The value, when every file has the same one.
    pub fn same(&self) -> Option<&T> {
        match self {
            Shared::Same(value) => Some(value),
            Shared::Mixed(_) => None,
        }
    }

    /// Whether the files differ.
    pub fn is_mixed(&self) -> bool {
        matches!(self, Shared::Mixed(_))
    }

    /// The number of different values: 1 when all are the same.
    pub fn distinct(&self) -> usize {
        match self {
            Shared::Same(_) => 1,
            Shared::Mixed(values) => values.len(),
        }
    }
}

/// What the files of a selection have in common, as the tag editor shows it: a
/// field with one value for all is filled in; a mixed field is left empty (kept
/// per file unless edited) with the number of different values beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionView {
    /// The number of files.
    pub files: usize,
    /// How many of them can be written.
    pub writable: usize,
    fields: BTreeMap<FieldKey, Shared<Vec<String>>>,
    /// The embedded front cover (by hash).
    pub front_cover: Shared<Option<CoverHash>>,
    /// The unsynchronized lyrics.
    pub lyrics: Shared<Option<String>>,
    /// The rating in the tags.
    pub rating: Shared<Option<Rating>>,
}

impl SelectionView {
    /// The view of `files`.
    pub fn of(files: &[EditableFile]) -> Self {
        let mut keys: Vec<FieldKey> = Vec::new();
        for file in files {
            for field in TagField::ALL {
                let key = FieldKey::Known(*field);
                if !keys.contains(&key) && !field_values(&file.tags, &key).is_empty() {
                    keys.push(key);
                }
            }
            for (name, _) in &file.tags.extra {
                let custom = !name.starts_with("comment:")
                    && TagField::from_picard_name(name).is_none()
                    && !keys
                        .iter()
                        .any(|k| matches!(k, FieldKey::Custom(c) if c.eq_ignore_ascii_case(name)));
                if custom {
                    keys.push(FieldKey::Custom(name.clone()));
                }
            }
        }
        let fields = keys
            .into_iter()
            .filter_map(|key| {
                let shared = Shared::of(files.iter().map(|f| field_values(&f.tags, &key)))?;
                Some((key, shared))
            })
            .collect();
        Self {
            files: files.len(),
            writable: files.iter().filter(|f| f.writable).count(),
            fields,
            front_cover: Shared::of(files.iter().map(|f| f.front_cover))
                .unwrap_or(Shared::Same(None)),
            lyrics: Shared::of(files.iter().map(|f| f.lyrics.clone()))
                .unwrap_or(Shared::Same(None)),
            rating: Shared::of(files.iter().map(|f| f.tags.rating)).unwrap_or(Shared::Same(None)),
        }
    }

    /// The shared value of `key`; `Same` with no values when no file has it.
    pub fn field(&self, key: &FieldKey) -> Shared<Vec<String>> {
        self.fields
            .get(key)
            .cloned()
            .unwrap_or(Shared::Same(Vec::new()))
    }

    /// The fields at least one file has, known fields in [`TagField::ALL`] order
    /// first, then the custom ones.
    pub fn present_fields(&self) -> impl Iterator<Item = (&FieldKey, &Shared<Vec<String>>)> {
        self.fields.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_values_count_each_value() {
        assert_eq!(Shared::<u8>::of([]), None);
        assert_eq!(Shared::of([1, 1]), Some(Shared::Same(1)));
        assert_eq!(
            Shared::of([1, 2, 2]),
            Some(Shared::Mixed(vec![(2, 2), (1, 1)]))
        );
        assert_eq!(Shared::of([1, 2, 2]).map(|s| s.distinct()), Some(2));
    }
}
