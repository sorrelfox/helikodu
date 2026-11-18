// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Artist photos: the one chosen in the app, else `artist.*` in the artist's
//! folder. Local files only; nothing is fetched.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helikodu_library::{ArtistPhoto, PersonId};

use super::{LibraryError, MusicLibrary};

/// The folder beside the database where the library keeps its copies of the
/// photos chosen in the app.
pub const ARTIST_PHOTO_FOLDER: &str = "artist-photos";

impl MusicLibrary {
    /// The folder of the library's copies of chosen artist photos: `artist-photos`
    /// beside the database; `None` for a library in memory.
    pub fn artist_photo_folder(&self) -> Option<PathBuf> {
        let parent = self.path().parent()?;
        (!parent.as_os_str().is_empty()).then(|| parent.join(ARTIST_PHOTO_FOLDER))
    }

    /// The photo of every artist that has one. With `look`, the artists' folders
    /// are looked in first for an `artist.*` picture that is new, changed or gone;
    /// a library opened read-only is not looked in.
    pub fn artist_photos(
        &self,
        look: bool,
    ) -> Result<HashMap<PersonId, ArtistPhoto>, LibraryError> {
        if look {
            // A read-only library keeps what it had.
            let _ = self.inner().find_artist_folder_photos();
        }
        Ok(self.inner().artist_photos()?)
    }

    /// The photo of `person`, if they have one.
    pub fn artist_photo(&self, person: PersonId) -> Result<Option<ArtistPhoto>, LibraryError> {
        Ok(self.inner().artist_photo(person)?)
    }

    /// Makes the picture at `source` the photo of `person`; the library keeps a
    /// copy in [`artist_photo_folder`](Self::artist_photo_folder) and leaves
    /// `source` as it is.
    pub fn set_artist_photo(
        &self,
        person: PersonId,
        source: &Path,
    ) -> Result<ArtistPhoto, LibraryError> {
        let folder = self
            .artist_photo_folder()
            .ok_or(LibraryError::NoDataFolder)?;
        Ok(self.inner().set_artist_photo(person, source, &folder)?)
    }

    /// Forgets the photo chosen for `person`. Returns the photo shown now: the
    /// one in the artist's folder, or none.
    pub fn remove_artist_photo(
        &self,
        person: PersonId,
    ) -> Result<Option<ArtistPhoto>, LibraryError> {
        Ok(self.inner().remove_artist_photo(person)?)
    }
}
