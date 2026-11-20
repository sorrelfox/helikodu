// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What Your fields needs from the core: the fields the user defined, saving
//! and removing one, and the text an expression gives for library tracks.
//!
//! [`FieldServices`] is implemented over the running service
//! (`service/fields.rs`) and over invented content (`sample/fields.rs`). Every
//! method is blocking: it is called on worker threads only. A saved or removed
//! field is put in use at once for the whole program (searches, naming
//! patterns, expressions), and the lists are told to fetch their tracks again.

use std::fmt;

pub use helikodu_model::expression::Template;
pub use helikodu_service::library::{
    DefinitionError, DefinitionPart, DefinitionProblem, FieldDefinition, UserField,
};

use super::{BackendError, TrackKey};

/// The fields the user defines. See the module documentation.
pub trait FieldServices: fmt::Debug + Send + Sync {
    /// The fields, in their order.
    fn user_fields(&self) -> Result<Vec<UserField>, BackendError>;
    /// Saves `field`: a new one when `old` is `None`, otherwise in the place of
    /// the field `old`, which may have had another name. The definition as
    /// stored.
    fn save_field(&self, old: Option<&str>, field: UserField) -> Result<UserField, BackendError>;
    /// Removes the field `name`.
    fn remove_field(&self, name: &str) -> Result<(), BackendError>;
    /// The text `template` gives for each of `tracks`, in their order; tracks
    /// the library does not have are left out.
    fn evaluate(
        &self,
        template: &Template,
        tracks: &[TrackKey],
    ) -> Result<Vec<(TrackKey, String)>, BackendError>;
}

/// Fields without a library: there are none, and none can be made.
#[derive(Debug, Default)]
pub(crate) struct NoFields;

fn no_library() -> BackendError {
    BackendError(crate::i18n::tr("Helikodu runs without a library."))
}

impl FieldServices for NoFields {
    fn user_fields(&self) -> Result<Vec<UserField>, BackendError> {
        Ok(Vec::new())
    }

    fn save_field(&self, old: Option<&str>, field: UserField) -> Result<UserField, BackendError> {
        let _ = (old, field);
        Err(no_library())
    }

    fn remove_field(&self, name: &str) -> Result<(), BackendError> {
        let _ = name;
        Err(no_library())
    }

    fn evaluate(
        &self,
        template: &Template,
        tracks: &[TrackKey],
    ) -> Result<Vec<(TrackKey, String)>, BackendError> {
        let _ = (template, tracks);
        Ok(Vec::new())
    }
}
