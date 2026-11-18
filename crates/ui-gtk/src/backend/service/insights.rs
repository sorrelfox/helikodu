// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The facet browser and the Library statistics over the running service.

use helikodu_model::query::Query;
use helikodu_service::library::{StatisticsRequest, parse_query_lenient};

use super::{
    BackendError, ServiceLibrary, TrackKey, TrackQuery, album_key, browse, cover_key, error, lock,
    query_sort, seed,
};
use crate::backend::{
    ArtistKey, FacetChoice, FacetField, FacetList, LibraryStats, PlayedAlbumRow, PlayedArtistRow,
    StatisticsSince,
};
use crate::i18n::tr;

/// How many artists and albums the lists of the most played hold.
const TOP: usize = 6;

/// The query of the filter text: a query, or words searched for.
fn text_query(query: &TrackQuery) -> Query {
    let text = query.text.trim();
    if text.is_empty() {
        Query::default()
    } else {
        parse_query_lenient(text).query
    }
}

impl ServiceLibrary {
    pub(super) fn facet_list(
        &self,
        query: &TrackQuery,
        chosen: &[FacetChoice],
        field: FacetField,
    ) -> Result<FacetList, BackendError> {
        let music = self.library()?;
        music
            .facet_values(&text_query(query), chosen, field)
            .map_err(error)
    }

    pub(super) fn facet_keys(
        &self,
        query: &TrackQuery,
        chosen: &[FacetChoice],
    ) -> Result<Vec<TrackKey>, BackendError> {
        let music = self.library()?;
        let mut search = text_query(query);
        if !query.sort.is_empty() {
            search.sort = query_sort(&query.sort);
        }
        let ids = music.facet_track_ids(&search, chosen).map_err(error)?;
        Ok(ids.iter().map(|id| TrackKey(id.get())).collect())
    }

    pub(super) fn library_statistics(
        &self,
        since: StatisticsSince,
    ) -> Result<LibraryStats, BackendError> {
        let music = self.library()?;
        let overview = music.overview(0).map_err(error)?;
        let totals = overview.totals;
        let counted = music
            .statistics(&StatisticsRequest {
                added_since: since.month,
                listening_since: since.year,
                top: TOP,
            })
            .map_err(error)?;
        {
            let mut ids = lock(&self.ids);
            for album in &counted.top_albums {
                ids.insert(album_key(album.album), album.album);
            }
        }
        Ok(LibraryStats {
            tracks: totals.tracks,
            added_this_month: counted.added_since,
            albums: totals.albums,
            albums_without_cover: counted.albums_without_cover,
            artists: totals.artists,
            composers: browse::composer_count(music),
            duration: totals.duration,
            size: totals.size,
            folders: overview.roots.len(),
            drives: music.root_devices().map_err(error)?,
            formats: counted.formats,
            lossless_cd: counted.lossless_cd,
            lossless_hi_res: counted.lossless_hi_res,
            lossy: counted.lossy,
            plays: counted.plays,
            listened: counted.listened,
            skips: counted.skips,
            never_played: counted.never_played,
            top_artists: counted
                .top_artists
                .into_iter()
                .map(|artist| PlayedArtistRow {
                    key: ArtistKey(u64::try_from(artist.person.get()).unwrap_or(0)),
                    name: artist.name,
                    plays: artist.plays,
                })
                .collect(),
            top_albums: counted
                .top_albums
                .into_iter()
                .map(|album| {
                    let key = album_key(album.album);
                    PlayedAlbumRow {
                        key,
                        title: album.title.unwrap_or_else(|| tr("Unknown album")),
                        artist: album.artist.unwrap_or_else(|| tr("Various artists")),
                        cover: album.cover.as_ref().map(cover_key),
                        cover_seed: seed(key),
                        plays: album.plays,
                    }
                })
                .collect(),
        })
    }
}
