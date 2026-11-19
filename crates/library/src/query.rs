// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The query language: reading query text into the tree of `helikodu_model::query`,
//! writing the tree back as text, compiling it to SQL and running it, and the saved
//! searches.
//!
//! - [`parse_query`] reads text strictly and reports the first problem with the byte
//!   range that fails; [`parse_query_lenient`] falls back to a plain search of the
//!   words. [`print_query`] writes a tree so that it reads back unchanged.
//! - [`Library::query_tracks`] and its siblings compile a [`QueryRequest`] and run it:
//!   every value is a bound parameter.
//! - Saved searches are stored as printed query text in `saved_searches`.

mod compiler;
mod facets;
mod parser;
mod printer;
mod run;
mod saved;
mod syntax_error;
mod values;

use std::time::SystemTime;

use helikodu_model::library::TrackId;
use helikodu_model::query::{Expr, Query, TermProblem};

pub use facets::{FacetChoice, FacetField, FacetKey, FacetList, FacetValue};
pub use parser::{ParsedQuery, parse as parse_query, parse_lenient as parse_query_lenient};
pub use printer::print as print_query;
pub use saved::{SavedSearch, SavedSearchId};
pub use syntax_error::{SyntaxError, SyntaxErrorKind};

use crate::catalog::{Page, Paged, TrackSummary};
use crate::db::DbError;
use crate::db::unix_time::to_unix_seconds;
use crate::library::Library;
use crate::user_fields::{computed_in, computed_values};
use compiler::{Compiled, Lookups, QueryContext};
pub(crate) use run::summaries as track_summaries;

/// A query to run: the tree, the page of results wanted and how to treat masked
/// library folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRequest {
    /// The query.
    pub query: Query,
    /// Which tracks of the result to return; the total counts them all.
    pub page: Page,
    /// Whether the tracks of masked library folders are found too.
    pub include_masked: bool,
    /// The seed of `sort:random`: the same seed gives the same order.
    pub seed: u64,
}

impl QueryRequest {
    /// The first 50 tracks of `query`.
    pub fn new(query: Query) -> Self {
        Self {
            query,
            page: Page::default(),
            include_masked: false,
            seed: 0,
        }
    }

    /// The same request for `page`.
    #[must_use]
    pub fn page(self, page: Page) -> Self {
        Self { page, ..self }
    }
}

/// Why a query or a saved search failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum QueryError {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// A comparison of the query does not fit together.
    #[error("the query cannot run: {0}")]
    Invalid(TermProblem),
    /// There is no such saved search.
    #[error("there is no saved search {0}")]
    UnknownSavedSearch(SavedSearchId),
    /// A saved search needs a name with more than white space.
    #[error("a saved search needs a name")]
    EmptyName,
}

impl Library {
    /// Compiles `request` for now.
    fn compile(&self, request: &QueryRequest) -> Result<Compiled, QueryError> {
        let now = to_unix_seconds(SystemTime::now());
        let (utc_offset, lookups) = self
            .database()
            .read(|c| Ok((run::utc_offset(c, now)?, lookups(c, &request.query)?)))?;
        let context = QueryContext {
            now,
            utc_offset,
            seed: request.seed,
        };
        compiler::compile(&request.query, &context, &lookups).map_err(QueryError::Invalid)
    }

    /// One page of the tracks `request` selects, in its order, with how many it
    /// selects in all.
    pub fn query_tracks(&self, request: &QueryRequest) -> Result<Paged<TrackSummary>, QueryError> {
        let compiled = self.compile(request)?;
        Ok(self
            .database()
            .read(|c| run::tracks(c, &compiled, request.include_masked, request.page))?)
    }

    /// Every track `request` selects, in its order; the page is not used.
    pub fn query_track_ids(&self, request: &QueryRequest) -> Result<Vec<TrackId>, QueryError> {
        let compiled = self.compile(request)?;
        Ok(self
            .database()
            .read(|c| run::track_ids(c, &compiled, request.include_masked))?)
    }

    /// How many tracks `request` selects.
    pub fn query_count(&self, request: &QueryRequest) -> Result<usize, QueryError> {
        let compiled = self.compile(request)?;
        Ok(self
            .database()
            .read(|c| run::count(c, &compiled, request.include_masked))?)
    }

    /// Every saved search, in the user's order.
    pub fn saved_searches(&self) -> Result<Vec<SavedSearch>, DbError> {
        self.database().read(saved::list)
    }

    /// The saved search `id`, if there is one.
    pub fn saved_search(&self, id: SavedSearchId) -> Result<Option<SavedSearch>, DbError> {
        self.database().read(|c| saved::get(c, id))
    }

    /// Saves `query` as `name`, after the other saved searches.
    pub fn save_search(&self, name: &str, query: &Query) -> Result<SavedSearchId, QueryError> {
        let name = check_name(name)?;
        let text = print_query(query);
        let now = to_unix_seconds(SystemTime::now());
        Ok(self
            .database()
            .writer()?
            .write(move |c| saved::create(c, &name, &text, now))?)
    }

    /// Renames the saved search `id`.
    pub fn rename_saved_search(&self, id: SavedSearchId, name: &str) -> Result<(), QueryError> {
        let name = check_name(name)?;
        let now = to_unix_seconds(SystemTime::now());
        let found = self
            .database()
            .writer()?
            .write(move |c| saved::rename(c, id, &name, now))?;
        found_or(found, id)
    }

    /// Replaces the query of the saved search `id`.
    pub fn set_saved_search_query(
        &self,
        id: SavedSearchId,
        query: &Query,
    ) -> Result<(), QueryError> {
        let text = print_query(query);
        let now = to_unix_seconds(SystemTime::now());
        let found = self
            .database()
            .writer()?
            .write(move |c| saved::set_query(c, id, &text, now))?;
        found_or(found, id)
    }

    /// Deletes the saved search `id`.
    pub fn delete_saved_search(&self, id: SavedSearchId) -> Result<(), QueryError> {
        let found = self
            .database()
            .writer()?
            .write(move |c| saved::delete(c, id))?;
        found_or(found, id)
    }

    /// Moves the saved search `id` to `position` (from 0; past the end is the end).
    pub fn move_saved_search(&self, id: SavedSearchId, position: usize) -> Result<(), QueryError> {
        let found = self
            .database()
            .writer()?
            .write(move |c| saved::move_to(c, id, position))?;
        found_or(found, id)
    }
}

/// Every track `query` selects now, in its order, on the connection `c`; the problem
/// when a comparison of the query does not fit together.
pub(crate) fn track_ids_on(
    c: &rusqlite::Connection,
    query: &Query,
    now: i64,
) -> rusqlite::Result<Result<Vec<TrackId>, TermProblem>> {
    let context = QueryContext {
        now,
        utc_offset: run::utc_offset(c, now)?,
        seed: 0,
    };
    match compiler::compile(query, &context, &lookups(c, query)?) {
        Ok(compiled) => run::track_ids(c, &compiled, false).map(Ok),
        Err(problem) => Ok(Err(problem)),
    }
}

/// What `query` needs looked up in the library before it is compiled: the labels
/// when it has plain text, and the values of the computed fields it compares.
fn lookups(c: &rusqlite::Connection, query: &Query) -> rusqlite::Result<Lookups> {
    fn has_text(expr: &Expr) -> bool {
        match expr {
            Expr::Text(_) => true,
            Expr::All(children) | Expr::Any(children) => children.iter().any(has_text),
            Expr::Not(inner) => has_text(inner),
            _ => false,
        }
    }
    let labels = if query.filter.as_ref().is_some_and(has_text) {
        c.prepare_cached("SELECT id, fold FROM labels")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    } else {
        Vec::new()
    };
    Ok(Lookups {
        labels,
        computed: computed_values(c, &computed_in(query))?,
    })
}

fn check_name(name: &str) -> Result<String, QueryError> {
    let name = name.trim();
    if name.is_empty() {
        Err(QueryError::EmptyName)
    } else {
        Ok(name.to_owned())
    }
}

fn found_or(found: bool, id: SavedSearchId) -> Result<(), QueryError> {
    if found {
        Ok(())
    } else {
        Err(QueryError::UnknownSavedSearch(id))
    }
}
