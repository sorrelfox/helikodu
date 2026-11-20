// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The facet browser and the Library statistics over the invented library, counted
//! in memory.

use std::collections::HashMap;
use std::time::Duration;

use super::{
    AlbumKey, ArtistKey, BackendError, LibrarySource, SampleLibrary, TrackKey, TrackQuery,
};
use crate::backend::insights::{facets_of, passes};

/// The labels of a sample track, as the facets list them.
fn labels(row: &TrackRow) -> Vec<String> {
    super::lists::labels_of(row)
        .into_iter()
        .map(str::to_owned)
        .collect()
}
use crate::backend::{
    FacetChoice, FacetField, FacetList, LibraryStats, PlayedAlbumRow, PlayedArtistRow,
    StatisticsSince, TrackRow,
};

impl SampleLibrary {
    fn album_artist_of(&self, album: AlbumKey) -> String {
        self.get(album)
            .map(|a| a.row.artist.clone())
            .unwrap_or_default()
    }

    fn rows_of(&self, query: &TrackQuery) -> Result<Vec<TrackRow>, BackendError> {
        let keys = self.track_keys(query)?;
        self.track_rows(&keys)
    }

    pub(super) fn sample_facets(
        &self,
        query: &TrackQuery,
        chosen: &[FacetChoice],
        field: FacetField,
    ) -> Result<FacetList, BackendError> {
        let artist = |album| self.album_artist_of(album);
        let rows: Vec<TrackRow> = self
            .rows_of(&TrackQuery {
                sort: Vec::new(),
                ..query.clone()
            })?
            .into_iter()
            .filter(|row| passes(row, chosen, &artist, &labels))
            .collect();
        Ok(facets_of(&rows, field, artist, labels))
    }

    pub(super) fn sample_facet_keys(
        &self,
        query: &TrackQuery,
        chosen: &[FacetChoice],
    ) -> Result<Vec<TrackKey>, BackendError> {
        let artist = |album| self.album_artist_of(album);
        Ok(self
            .rows_of(query)?
            .into_iter()
            .filter(|row| passes(row, chosen, &artist, &labels))
            .map(|row| row.key)
            .collect())
    }

    pub(super) fn sample_statistics(
        &self,
        since: StatisticsSince,
    ) -> Result<LibraryStats, BackendError> {
        let _ = since;
        let rows = self.rows_of(&TrackQuery::default())?;
        let counts = self.counts()?;
        let mut formats: HashMap<Option<String>, usize> = HashMap::new();
        let (mut cd, mut hi_res, mut lossy) = (0, 0, 0);
        let mut by_artist: HashMap<String, u64> = HashMap::new();
        let mut by_album: HashMap<AlbumKey, u64> = HashMap::new();
        let mut plays = 0u64;
        let mut never = 0;
        for row in &rows {
            let codec = row.format.codec.as_ref().map(|c| c.to_lowercase());
            let lossless = matches!(codec.as_deref(), Some("flac" | "alac"));
            let high =
                row.format.sample_rate.unwrap_or(0) > 48_000 || row.format.bits.unwrap_or(0) > 16;
            match (lossless, high) {
                (true, false) => cd += 1,
                (true, true) => hi_res += 1,
                (false, _) => lossy += 1,
            }
            *formats.entry(codec).or_default() += 1;
            let p = u64::from(row.plays);
            plays += p;
            if p == 0 {
                never += 1;
            } else {
                *by_artist.entry(row.artist.clone()).or_default() += p;
                *by_album.entry(row.album).or_default() += p;
            }
        }
        let mut formats: Vec<(Option<String>, usize)> = formats.into_iter().collect();
        formats.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut artists: Vec<(String, u64)> = by_artist.into_iter().collect();
        artists.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut albums: Vec<(AlbumKey, u64)> = by_album.into_iter().collect();
        albums.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let top_artists = artists
            .into_iter()
            .take(6)
            .map(|(name, plays)| PlayedArtistRow {
                key: self
                    .artists
                    .iter()
                    .position(|a| *a == name)
                    .map_or(ArtistKey(0), |i| ArtistKey(i as u64 + 1)),
                name,
                plays,
            })
            .collect();
        let top_albums = albums
            .into_iter()
            .take(6)
            .filter_map(|(key, plays)| {
                let album = self.get(key)?;
                Some(PlayedAlbumRow {
                    key,
                    title: album.row.title.clone(),
                    artist: album.row.artist.clone(),
                    cover: album.row.cover,
                    cover_seed: album.row.cover_seed,
                    plays,
                })
            })
            .collect();
        let duration: Duration = rows.iter().filter_map(|r| r.duration).sum();
        Ok(LibraryStats {
            tracks: rows.len(),
            added_this_month: rows.len() / 150,
            albums: counts.albums,
            albums_without_cover: counts.albums / 35,
            artists: counts.artists,
            composers: counts.composers,
            duration,
            size: duration.as_secs() * 36_000,
            folders: 2,
            drives: 1,
            formats,
            lossless_cd: cd,
            lossless_hi_res: hi_res,
            lossy,
            plays,
            listened: Duration::from_secs(plays * 240),
            skips: plays / 13,
            never_played: never,
            top_artists,
            top_albums,
        })
    }
}
