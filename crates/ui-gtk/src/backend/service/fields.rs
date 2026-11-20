// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Your fields over the running service: the definitions are kept in the
//! library database, and the library puts each change in use for the whole
//! program at once. After a change every list is told to fetch its tracks
//! again, so a search or a column that names the field follows it.

use std::sync::Arc;

use helikodu_model::library::TrackId;
use helikodu_service::Service;
use helikodu_service::library::{FieldError, LibraryError, MusicLibrary};

use super::super::fields::{FieldServices, Template, UserField};
use super::super::{BackendError, TrackKey};
use crate::i18n::{fill, tr};

/// Your fields over the running service.
#[derive(Debug, Clone)]
pub(in crate::backend) struct ServiceFields {
    pub(in crate::backend) service: Arc<Service>,
}

impl ServiceFields {
    fn music(&self) -> Result<&MusicLibrary, BackendError> {
        self.service
            .library()
            .map(|l| l.library())
            .ok_or_else(|| BackendError(tr("Helikodu runs without a library.")))
    }

    /// Tells every list that the fields changed.
    fn announce(&self) {
        if let Some(library) = self.service.library() {
            library.announce_change();
        }
    }
}

/// A refusal in words. Definitions are checked in the window before they are
/// saved, so these are rare.
fn refused(error: &LibraryError) -> BackendError {
    match error {
        LibraryError::Field(FieldError::InUse { field, used_by }) => BackendError(fill(
            &tr("The field {} uses {}, so it cannot be removed."),
            &[used_by, field],
        )),
        other => BackendError(other.to_string()),
    }
}

impl FieldServices for ServiceFields {
    fn user_fields(&self) -> Result<Vec<UserField>, BackendError> {
        self.music()?.user_fields().map_err(|e| refused(&e))
    }

    fn save_field(&self, old: Option<&str>, field: UserField) -> Result<UserField, BackendError> {
        let music = self.music()?;
        let stored = match old {
            Some(old) => music.replace_field(old, field),
            None => music.define_field(field),
        }
        .map_err(|e| refused(&e))?;
        self.announce();
        Ok(stored)
    }

    fn remove_field(&self, name: &str) -> Result<(), BackendError> {
        self.music()?.remove_field(name).map_err(|e| refused(&e))?;
        self.announce();
        Ok(())
    }

    fn evaluate(
        &self,
        template: &Template,
        tracks: &[TrackKey],
    ) -> Result<Vec<(TrackKey, String)>, BackendError> {
        let ids: Vec<TrackId> = tracks.iter().map(|k| TrackId::new(k.0)).collect();
        let values = self
            .music()?
            .evaluate_expression(template, &ids)
            .map_err(|e| BackendError(e.to_string()))?;
        Ok(values
            .into_iter()
            .map(|(id, value)| (TrackKey(id.get()), value))
            .collect())
    }
}
