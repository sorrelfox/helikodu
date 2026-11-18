// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Invented composers and the years and media kinds of the sample library.
//!
//! The albums of the classical genres have a composer from the list below. A media
//! kind chosen for an album is kept for as long as the backend runs.

use std::collections::BTreeMap;
use std::sync::PoisonError;

use super::{
    AlbumKey, AlbumQuery, AlbumScope, ArtistKey, ArtistRow, GENRES, SampleAlbum, SampleLibrary,
    TrackKey, sort_name,
};
use crate::backend::{MediaKind, YearRange, YearRow};

/// The composers of the sample library.
const COMPOSERS: &[&str] = &[
    "Lili Boulanger",
    "Jean Sibelius",
    "Kaija Saariaho",
    "Arvo Pärt",
    "Maurice Ravel",
    "Veljo Tormis",
];

/// The first key of a composer; artists have the keys below it.
const FIRST_COMPOSER: u64 = 10_000;

impl SampleAlbum {
    /// The index of its composer, for an album of a classical genre.
    pub(super) fn composer(&self) -> Option<usize> {
        let classical = matches!(
            GENRES[self.genre],
            "Chamber music" | "Choral" | "Classical" | "Modern classical"
        );
        classical.then(|| usize::try_from(self.row.key.0).unwrap_or(0) % COMPOSERS.len())
    }
}

/// The key of composer `index`.
fn composer_key(index: usize) -> ArtistKey {
    ArtistKey(FIRST_COMPOSER + index as u64)
}

/// The index of the composer with `key`.
pub(super) fn composer_index(key: ArtistKey) -> Option<usize> {
    let index = usize::try_from(key.0.checked_sub(FIRST_COMPOSER)?).ok()?;
    (index < COMPOSERS.len()).then_some(index)
}

impl SampleLibrary {
    /// Every composer with an album, by sort name.
    pub(super) fn composer_rows(&self) -> Vec<ArtistRow> {
        let mut rows: Vec<ArtistRow> = COMPOSERS
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let albums: Vec<&SampleAlbum> = self
                    .albums
                    .iter()
                    .filter(|a| a.composer() == Some(i))
                    .collect();
                ArtistRow {
                    key: composer_key(i),
                    name: (*name).to_owned(),
                    sort_name: sort_name(name),
                    album_count: albums.len(),
                    track_count: albums.iter().map(|a| a.tracks.len()).sum(),
                    ..ArtistRow::default()
                }
            })
            .filter(|c| c.album_count > 0)
            .collect();
        rows.sort_by(|a, b| a.sort_name.cmp(&b.sort_name));
        rows
    }

    /// Every year with albums, newest first.
    pub(super) fn year_rows(&self) -> Vec<YearRow> {
        let mut years: BTreeMap<std::cmp::Reverse<i32>, YearRow> = BTreeMap::new();
        for album in &self.albums {
            let Some(year) = album.row.year else { continue };
            let row = years.entry(std::cmp::Reverse(year)).or_insert(YearRow {
                year: Some(year),
                ..YearRow::default()
            });
            row.album_count += 1;
            row.track_count += album.tracks.len();
        }
        years.into_values().collect()
    }

    /// The tracks of the albums of `scope`, album by album; audiobooks are left out.
    pub(super) fn scope_tracks(&self, scope: AlbumScope) -> Vec<TrackKey> {
        let albums = self.sorted(&AlbumQuery {
            scope,
            ..AlbumQuery::default()
        });
        albums
            .iter()
            .filter(|a| self.kind_of(a.row.key) != MediaKind::Audiobook)
            .flat_map(|a| a.tracks.iter().map(|t| t.key))
            .collect()
    }

    /// The name of `scope` as the queue shows it.
    pub(super) fn scope_name(scope: AlbumScope) -> String {
        match scope {
            AlbumScope::Composer(key) => composer_index(key)
                .map(|i| COMPOSERS[i].to_owned())
                .unwrap_or_default(),
            AlbumScope::Years(YearRange::Span { first, last }) if first == last => {
                first.to_string()
            }
            AlbumScope::Years(YearRange::Span { first, .. }) => format!("{first}s"),
            _ => String::new(),
        }
    }

    /// The kind of the album `key`.
    pub(super) fn kind_of(&self, key: AlbumKey) -> MediaKind {
        self.kinds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .copied()
            .unwrap_or_default()
    }

    /// Gives the album `key` the kind `kind`.
    pub(super) fn set_kind(&self, key: AlbumKey, kind: MediaKind) {
        self.kinds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, kind);
    }
}
