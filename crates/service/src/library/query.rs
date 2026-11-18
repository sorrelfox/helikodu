// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The query language and saved searches for the front ends.
//!
//! The text of the search entry is read with [`parse_query_lenient`]: text that cannot
//! be read still searches for its words, and the problem comes back with the byte
//! range to underline. The query tree is that of `helikodu_model::query`; the rule
//! builder edits it and [`print_query`] writes it back as text.

use helikodu_library::{QueryRequest, SavedSearch, SavedSearchId};
use helikodu_model::library::TrackId;
use helikodu_model::query::Query;

pub use helikodu_library::{
    ParsedQuery, QueryError, SyntaxError, SyntaxErrorKind, parse_query, parse_query_lenient,
    print_query,
};

use super::{LibraryError, LibraryService, MusicLibrary, Page, Paged, TrackSummary};

/// A saved search with the number of tracks it finds now, as the side column shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSearchEntry {
    /// The saved search.
    pub search: SavedSearch,
    /// How many tracks it finds.
    pub count: usize,
}

impl MusicLibrary {
    /// `limit` of the tracks `query` selects after the first `offset`, in its order,
    /// with how many it selects in all. At most 1000 are returned at once.
    pub fn query_tracks(
        &self,
        query: &Query,
        offset: usize,
        limit: usize,
    ) -> Result<Paged<TrackSummary>, LibraryError> {
        let request = QueryRequest::new(query.clone()).page(Page::new(offset, limit));
        Ok(self.library.query_tracks(&request)?)
    }

    /// Every track `query` selects, in its order; `seed` fixes a shuffled order.
    pub fn query_track_ids(&self, query: &Query, seed: u64) -> Result<Vec<TrackId>, LibraryError> {
        let mut request = QueryRequest::new(query.clone());
        request.seed = seed;
        Ok(self.library.query_track_ids(&request)?)
    }

    /// How many tracks `query` selects.
    pub fn query_count(&self, query: &Query) -> Result<usize, LibraryError> {
        Ok(self
            .library
            .query_count(&QueryRequest::new(query.clone()))?)
    }

    /// Every saved search in the user's order, each with the number of tracks it
    /// finds.
    pub fn saved_searches(&self) -> Result<Vec<SavedSearchEntry>, LibraryError> {
        self.library
            .saved_searches()?
            .into_iter()
            .map(|search| {
                let count = self.query_count(&search.query)?;
                Ok(SavedSearchEntry { search, count })
            })
            .collect()
    }

    /// Saves `query` as `name`, after the other saved searches.
    pub fn save_search(&self, name: &str, query: &Query) -> Result<SavedSearchId, LibraryError> {
        Ok(self.library.save_search(name, query)?)
    }

    /// Renames the saved search `id`.
    pub fn rename_saved_search(&self, id: SavedSearchId, name: &str) -> Result<(), LibraryError> {
        Ok(self.library.rename_saved_search(id, name)?)
    }

    /// Replaces the query of the saved search `id`.
    pub fn set_saved_search_query(
        &self,
        id: SavedSearchId,
        query: &Query,
    ) -> Result<(), LibraryError> {
        Ok(self.library.set_saved_search_query(id, query)?)
    }

    /// Deletes the saved search `id`.
    pub fn delete_saved_search(&self, id: SavedSearchId) -> Result<(), LibraryError> {
        Ok(self.library.delete_saved_search(id)?)
    }

    /// Moves the saved search `id` to `position` (from 0; past the end is the end).
    pub fn move_saved_search(
        &self,
        id: SavedSearchId,
        position: usize,
    ) -> Result<(), LibraryError> {
        Ok(self.library.move_saved_search(id, position)?)
    }
}

impl LibraryService {
    /// A page of the tracks `query` selects, with the total.
    pub fn query_tracks(
        &self,
        query: Query,
        offset: usize,
        limit: usize,
        reply: impl FnOnce(Result<Paged<TrackSummary>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(move |l| l.query_tracks(&query, offset, limit), reply)
    }

    /// The saved searches with their counts.
    pub fn saved_searches(
        &self,
        reply: impl FnOnce(Result<Vec<SavedSearchEntry>, LibraryError>) + Send + 'static,
    ) -> Result<(), LibraryError> {
        self.run(MusicLibrary::saved_searches, reply)
    }
}
