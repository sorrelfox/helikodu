// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The artist photos chosen in the sample library: kept in memory, each shown
//! from the file that was chosen.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use super::super::{ArtistKey, ArtistRow, CoverKey};

/// The photos chosen, by artist.
#[derive(Debug, Default)]
pub(super) struct Photos(Mutex<HashMap<ArtistKey, PathBuf>>);

/// A stable key for the picture at `path`.
fn key_of(path: &Path) -> CoverKey {
    let mut key = [0_u8; 32];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, chunk) in key.chunks_mut(8).enumerate() {
        for b in path.as_os_str().as_encoded_bytes().iter().chain(&[i as u8]) {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        chunk.copy_from_slice(&h.to_le_bytes());
    }
    CoverKey(key)
}

impl Photos {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ArtistKey, PathBuf>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Gives `row` its photo, if one was chosen.
    pub(super) fn show(&self, row: &mut ArtistRow) {
        let photos = self.lock();
        let photo = photos.get(&row.key);
        row.photo = photo.map(|p| key_of(p));
        row.own_photo = photo.is_some();
    }

    /// Chooses `path` as the photo of `key`.
    pub(super) fn set(&self, key: ArtistKey, path: &Path) {
        self.lock().insert(key, path.to_path_buf());
    }

    /// Forgets the photo of `key`.
    pub(super) fn remove(&self, key: ArtistKey) {
        self.lock().remove(&key);
    }

    /// The file of the photo with `cover`, if it is one.
    pub(super) fn file(&self, cover: &CoverKey) -> Option<PathBuf> {
        self.lock().values().find(|p| key_of(p) == *cover).cloned()
    }
}
