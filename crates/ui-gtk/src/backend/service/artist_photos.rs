// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Artist photos over the service's library: the one chosen in the app, else
//! `artist.*` in the artist's folder.

use std::collections::HashMap;
use std::path::Path;

use helikodu_service::library::{ArtistPhoto, PersonId};

use super::super::{ArtistKey, ArtistRow, BackendError, LibrarySource};
use super::{ServiceLibrary, cover_key, error, lock, person};

/// Gives `row` the photo `photo`, or none.
pub(super) fn show(row: &mut ArtistRow, photo: Option<&ArtistPhoto>) {
    row.photo = photo.map(|p| cover_key(&p.hash));
    row.own_photo = photo.is_some_and(|p| p.chosen);
}

/// Gives each of `rows` its photo from `photos`.
pub(super) fn show_all(rows: &mut [ArtistRow], photos: &HashMap<PersonId, ArtistPhoto>) {
    for row in rows {
        show(row, photos.get(&person(row.key)));
    }
}

impl ServiceLibrary {
    /// Makes the picture at `file` the photo of `key`.
    pub(super) fn set_photo(&self, key: ArtistKey, file: &Path) -> Result<ArtistRow, BackendError> {
        let photo = self
            .library()?
            .set_artist_photo(person(key), file)
            .map_err(error)?;
        self.photo_changed(key, Some(&photo))
    }

    /// Forgets the photo chosen for `key`.
    pub(super) fn remove_photo(&self, key: ArtistKey) -> Result<ArtistRow, BackendError> {
        let photo = self
            .library()?
            .remove_artist_photo(person(key))
            .map_err(error)?;
        self.photo_changed(key, photo.as_ref())
    }

    /// The row of `key` with its new photo, kept in the artist list too.
    fn photo_changed(
        &self,
        key: ArtistKey,
        photo: Option<&ArtistPhoto>,
    ) -> Result<ArtistRow, BackendError> {
        if let Some(row) = lock(&self.artist_rows).iter_mut().find(|r| r.key == key) {
            show(row, photo);
            return Ok(row.clone());
        }
        let mut row = self
            .artists()?
            .into_iter()
            .find(|r| r.key == key)
            .ok_or_else(|| BackendError("the artist is gone".into()))?;
        show(&mut row, photo);
        Ok(row)
    }
}
