// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The facets of the track table's browser and the counts of the statistics view.

use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;

use helikodu_library::{
    FacetChoice, FacetField, FacetList, LibraryStatistics, QueryRequest, StatisticsRequest,
};
use helikodu_model::library::TrackId;
use helikodu_model::query::Query;

use super::{LibraryError, MusicLibrary};

impl MusicLibrary {
    /// The values of `field` among the tracks `query` selects that pass every choice
    /// of `chosen`, each with its number of tracks.
    pub fn facet_values(
        &self,
        query: &Query,
        chosen: &[FacetChoice],
        field: FacetField,
    ) -> Result<FacetList, LibraryError> {
        Ok(self
            .library
            .facet_values(&QueryRequest::new(query.clone()), chosen, field)?)
    }

    /// Every track `query` selects that passes every choice of `chosen`, in the
    /// query's order.
    pub fn facet_track_ids(
        &self,
        query: &Query,
        chosen: &[FacetChoice],
    ) -> Result<Vec<TrackId>, LibraryError> {
        Ok(self
            .library
            .facet_track_ids(&QueryRequest::new(query.clone()), chosen)?)
    }

    /// The counts of the statistics view.
    pub fn statistics(
        &self,
        request: &StatisticsRequest,
    ) -> Result<LibraryStatistics, LibraryError> {
        Ok(self.library.statistics(request)?)
    }

    /// How many file systems (drives) the library folders are on; folders that
    /// cannot be reached now are not counted.
    pub fn root_devices(&self) -> Result<usize, LibraryError> {
        let devices: BTreeSet<u64> = self
            .library
            .roots()?
            .iter()
            .filter_map(|root| std::fs::metadata(root).ok())
            .map(|metadata| metadata.dev())
            .collect();
        Ok(devices.len())
    }
}
