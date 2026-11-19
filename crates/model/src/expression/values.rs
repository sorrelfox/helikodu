// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The values of a track's fields, as an expression asks for them.

use std::collections::HashMap;

use crate::query::Field;

/// The values of one track's fields, as a template asks for them.
pub trait FieldValues {
    /// The value of `field`, or `None` (or an empty text) when it is empty.
    /// Numbers as plain numbers: templates pad them. A field of several values
    /// gives them joined with `, `.
    fn value(&self, field: &Field) -> Option<String>;

    /// Every value of a field of several values (artists, genres, a tag given
    /// more than once), in order. By default the one value of
    /// [`value`](Self::value).
    fn values(&self, field: &Field) -> Vec<String> {
        self.value(field)
            .filter(|v| !v.trim().is_empty())
            .into_iter()
            .collect()
    }

    /// The number of tracks of the track's disc, when known: three digits for
    /// `{track}` from a hundred on.
    fn track_count(&self) -> Option<u32> {
        None
    }

    /// The number of discs of the track's album (or its highest disc number),
    /// when known: `{disc}` is empty unless it is more than one.
    fn disc_count(&self) -> Option<u32> {
        None
    }
}

impl<F: Fn(&Field) -> Option<String>> FieldValues for F {
    fn value(&self, field: &Field) -> Option<String> {
        self(field)
    }
}

/// [`FieldValues`] from a map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldMap {
    /// The values.
    pub values: HashMap<Field, String>,
    /// The values of fields that have several, one by one; the field's entry
    /// in `values` holds them joined.
    pub lists: HashMap<Field, Vec<String>>,
    /// The number of tracks of the disc.
    pub track_count: Option<u32>,
    /// The number of discs of the album.
    pub disc_count: Option<u32>,
}

impl FieldMap {
    /// Sets `field` to `value`; an empty value is left out.
    #[must_use]
    pub fn with(mut self, field: Field, value: impl Into<String>) -> Self {
        self.set(field, value);
        self
    }

    /// Sets `field` to `value`; an empty value is left out.
    pub fn set(&mut self, field: Field, value: impl Into<String>) {
        let value = value.into();
        if !value.trim().is_empty() {
            self.values.insert(field, value);
        }
    }

    /// Sets `field` to several values (joined with `, ` as its one value); empty
    /// values are left out.
    pub fn set_list(&mut self, field: Field, values: Vec<String>) {
        let values: Vec<String> = values
            .into_iter()
            .filter(|v| !v.trim().is_empty())
            .collect();
        if values.is_empty() {
            return;
        }
        self.values.insert(field.clone(), values.join(", "));
        self.lists.insert(field, values);
    }
}

impl FieldValues for FieldMap {
    fn value(&self, field: &Field) -> Option<String> {
        self.values.get(field).cloned()
    }

    fn values(&self, field: &Field) -> Vec<String> {
        match self.lists.get(field) {
            Some(list) => list.clone(),
            None => self
                .values
                .get(field)
                .filter(|v| !v.trim().is_empty())
                .cloned()
                .into_iter()
                .collect(),
        }
    }

    fn track_count(&self) -> Option<u32> {
        self.track_count
    }

    fn disc_count(&self) -> Option<u32> {
        self.disc_count
    }
}
