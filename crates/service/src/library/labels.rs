// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Labels of tracks, albums and artists, and the fields the user defines:
//! computed fields and custom fields.
//!
//! Labels live in the library and are never written to a file; a label of an
//! album or an artist counts for its tracks, and `label:` finds them in the query
//! language. A computed field is a name for an expression of the expression
//! language, a custom field a name for a tag; both can be named wherever a field
//! can (queries, naming patterns, sorting and grouping), except that a query cannot
//! sort by a computed field. A custom field is written as its tag is: its
//! [`FieldKey`] in the tag editor is [`MusicLibrary::field_key`].

use helikodu_library::tag_editing::FieldKey;
use helikodu_library::{
    FieldDefinition, ItemLabels, LabelItems, LabelSummary, LabelTarget, UserField,
};
use helikodu_model::library::TrackId;
use helikodu_model::query::Field;

use super::{LibraryError, MusicLibrary};

impl MusicLibrary {
    /// Every label, in name order, with how many tracks, albums and artists have it.
    pub fn labels(&self) -> Result<Vec<LabelSummary>, LibraryError> {
        Ok(self.inner().labels()?)
    }

    /// Gives the label `name` to `target`, making it if there is none. How many
    /// tracks, albums or artists did not have it before.
    pub fn add_label(&self, target: &LabelTarget, name: &str) -> Result<usize, LibraryError> {
        Ok(self.inner().add_label(target, name)?)
    }

    /// Takes the label `name` from `target`. How many had it.
    pub fn remove_label(&self, target: &LabelTarget, name: &str) -> Result<usize, LibraryError> {
        Ok(self.inner().remove_label(target, name)?)
    }

    /// Renames the label `name` to `new_name`, which no other label may have.
    pub fn rename_label(&self, name: &str, new_name: &str) -> Result<(), LibraryError> {
        Ok(self.inner().rename_label(name, new_name)?)
    }

    /// Merges the label `from` into `into`.
    pub fn merge_labels(&self, from: &str, into: &str) -> Result<(), LibraryError> {
        Ok(self.inner().merge_labels(from, into)?)
    }

    /// Removes the label `name` from everything that has it.
    pub fn delete_label(&self, name: &str) -> Result<(), LibraryError> {
        Ok(self.inner().delete_label(name)?)
    }

    /// The labels of `target`: its own, and for tracks those from their albums and
    /// artists.
    pub fn labels_of(&self, target: &LabelTarget) -> Result<ItemLabels, LibraryError> {
        Ok(self.inner().labels_of(target)?)
    }

    /// Every label with what it was given to, for an export of the user's data.
    pub fn label_items(&self) -> Result<Vec<LabelItems>, LibraryError> {
        Ok(self.inner().label_items()?)
    }

    /// The fields the user defined, in their order.
    pub fn user_fields(&self) -> Result<Vec<UserField>, LibraryError> {
        Ok(self.inner().user_fields()?)
    }

    /// Defines `field` or replaces the field of its name; the definition as stored.
    pub fn define_field(&self, field: UserField) -> Result<UserField, LibraryError> {
        Ok(self.inner().define_field(field)?)
    }

    /// Puts `field` in the place of the field `old`, which may have another
    /// name; the definition as stored. A rename another field needs the old
    /// name for is refused.
    pub fn replace_field(&self, old: &str, field: UserField) -> Result<UserField, LibraryError> {
        Ok(self.inner().replace_field(old, field)?)
    }

    /// Removes the field `name`, unless a computed field uses it.
    pub fn remove_field(&self, name: &str) -> Result<(), LibraryError> {
        Ok(self.inner().remove_field(name)?)
    }

    /// The value of the field `name` (one of the user's or any other) for each of
    /// `tracks`, in their order.
    pub fn field_values(
        &self,
        name: &str,
        tracks: &[TrackId],
    ) -> Result<Vec<(TrackId, String)>, LibraryError> {
        Ok(self.inner().field_values(name, tracks)?)
    }

    /// Every label of each of `tracks`, own and inherited, in name order, as a
    /// track list's labels column shows them.
    pub fn track_labels(
        &self,
        tracks: &[TrackId],
    ) -> Result<Vec<(TrackId, Vec<String>)>, LibraryError> {
        Ok(self.inner().track_labels(tracks)?)
    }

    /// The values of the tag `key` with their numbers of tracks, in name order,
    /// when there are at most `limit`; `None` when there are more.
    pub fn tag_values(
        &self,
        key: &str,
        limit: usize,
    ) -> Result<Option<Vec<(String, usize)>>, LibraryError> {
        Ok(self.inner().tag_values(key, limit)?)
    }

    /// The text `expression` gives for each of `tracks`, in their order; tracks
    /// the library does not have are left out.
    pub fn evaluate_expression(
        &self,
        expression: &helikodu_model::expression::Template,
        tracks: &[TrackId],
    ) -> Result<Vec<(TrackId, String)>, LibraryError> {
        Ok(self.inner().evaluate_expression(expression, tracks)?)
    }

    /// The key the tag editor writes the custom field `name` under (see
    /// [`tag_editing`](super::tag_editing)); `None` when `name` is no custom field.
    pub fn field_key(&self, name: &str) -> Option<FieldKey> {
        let field = helikodu_model::user_fields::current().get(name)?.clone();
        match (field.definition, Field::from_name(name)) {
            (FieldDefinition::Tag { key }, Some(Field::Tag(_))) => Some(FieldKey::parse(&key)),
            _ => None,
        }
    }
}
