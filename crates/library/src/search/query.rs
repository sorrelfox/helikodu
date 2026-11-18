// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Plain text search: the matching tracks, ranked and paged.

use std::time::Duration;

use helikodu_model::library::TrackId;
use rusqlite::Connection;
use rusqlite::types::Value;

use super::rank::MatchRank;
use super::terms::{self, Terms};
use crate::catalog::roots::visibility;
use crate::catalog::track_id_from_sql;
use crate::db::unix_time;

/// The most results one page can hold.
const MAX_PAGE: usize = 1000;

/// A search: the text typed and the page of results wanted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// The text. Case, accents and the width of characters do not matter.
    pub text: String,
    /// How many results to skip.
    pub offset: usize,
    /// How many results to return, at most 1000.
    pub limit: usize,
    /// Whether the tracks of masked library folders are found too. Off by default,
    /// as in the browsing queries.
    pub include_masked: bool,
}

impl SearchRequest {
    /// The first 50 results for `text`.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            offset: 0,
            limit: 50,
            include_masked: false,
        }
    }

    /// The same search, for `limit` results after the first `offset`.
    #[must_use]
    pub fn page(self, offset: usize, limit: usize) -> Self {
        Self {
            offset,
            limit,
            ..self
        }
    }
}

/// One page of search results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults {
    /// The results on this page, best first.
    pub hits: Vec<SearchHit>,
    /// The number of matching tracks on all pages.
    pub total: usize,
}

/// One matching track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// The track.
    pub track: TrackId,
    /// How it matched.
    pub rank: MatchRank,
    /// Its title.
    pub title: Option<String>,
    /// Its artists, separated by commas.
    pub artist: Option<String>,
    /// Its album title.
    pub album: Option<String>,
    /// Its length.
    pub duration: Option<Duration>,
}

/// Whether track `t` has a short term in its folded title, artists, album or genres.
/// `?{n}` is the term.
const SHORT_TERM: &str = "(instr(t.title_fold, ?{n}) > 0
    OR instr(t.artists_fold, ?{n}) > 0
    OR t.album_id IN (SELECT id FROM albums WHERE instr(title_fold, ?{n}) > 0)
    OR t.id IN (SELECT tg.track_id FROM genres g JOIN track_genres tg ON tg.genre_id = g.id
                WHERE instr(g.fold, ?{n}) > 0))";

/// The best rank of the title, artists, album artists and album against `?1`.
const RANK: &str = "min(
    coalesce(hk_match_rank(t.title_fold, ?1), 4),
    coalesce(hk_match_rank(t.artists_fold, ?1), 4),
    coalesce(hk_match_rank(a.title_fold, ?1), 4))";

/// Runs `request` on `c`.
pub(crate) fn search(c: &Connection, request: &SearchRequest) -> rusqlite::Result<SearchResults> {
    let Some(terms) = Terms::parse(&request.text) else {
        return Ok(SearchResults::default());
    };
    let limit = request.limit.min(MAX_PAGE);
    let mut params: Vec<Value> = vec![Value::Text(terms.whole.clone())];
    let visible = visibility(c, request.include_masked)?;
    let mut matched = matching_condition(&terms, &mut params);
    if visible != "1" {
        matched = format!("{matched} AND {visible}");
    }
    let mut total = 0;
    let mut hits = Vec::new();
    if limit > 0 {
        let limit_param = params.len() + 1;
        let mut page_params = params.clone();
        page_params.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        page_params.push(Value::Integer(
            i64::try_from(request.offset).unwrap_or(i64::MAX),
        ));
        // One pass finds the page and counts every match.
        let sql = format!(
            "WITH page AS (
                 SELECT t.id AS id, {RANK} AS rank, t.title_sort AS sort,
                        count(*) OVER () AS total
                 FROM tracks t
                 LEFT JOIN albums a ON a.id = t.album_id
                 WHERE {matched}
                 ORDER BY rank, sort, t.id
                 LIMIT ?{limit_param} OFFSET ?{offset_param})
             SELECT page.id, page.rank, t.title,
                 (SELECT group_concat(name, ', ') FROM
                     (SELECT p.name AS name
                      FROM track_people tp JOIN people p ON p.id = tp.person_id
                      WHERE tp.track_id = t.id AND tp.role = 0 ORDER BY tp.position)),
                 a.title, t.duration_ms, page.total
             FROM page
             JOIN tracks t ON t.id = page.id
             LEFT JOIN albums a ON a.id = t.album_id
             ORDER BY page.rank, page.sort, page.id",
            offset_param = limit_param + 1,
        );
        let mut statement = c.prepare_cached(&sql)?;
        let mut rows = statement.query(rusqlite::params_from_iter(&page_params))?;
        while let Some(row) = rows.next()? {
            total = row.get(6)?;
            hits.push(SearchHit {
                track: track_id_from_sql(row.get(0)?),
                rank: MatchRank::from_sql(row.get(1)?),
                title: row.get(2)?,
                artist: row.get(3)?,
                album: row.get(4)?,
                duration: row.get::<_, Option<i64>>(5)?.map(unix_time::from_millis),
            });
        }
    }
    if hits.is_empty() && (limit == 0 || request.offset > 0) {
        // The page is past the end, or none was asked for: count on its own. `?1` is
        // not used by the count, but binding it keeps the numbering.
        total = c
            .prepare_cached(&format!("SELECT count(*) FROM tracks t WHERE {matched}"))?
            .query_row(rusqlite::params_from_iter(&params), |row| row.get(0))?;
    }
    Ok(SearchResults {
        hits,
        total: usize::try_from(total).unwrap_or(0),
    })
}

/// The condition on track `t` that every term matches. Adds the terms to `params`,
/// numbered from 2 (`?1` is the whole query, for ranking). Index lookups come first, so
/// that a query with a long term starts from the index.
fn matching_condition(terms: &Terms, params: &mut Vec<Value>) -> String {
    let mut push = |value: String| {
        params.push(Value::Text(value));
        params.len()
    };
    let mut long = Vec::new();
    let mut short = Vec::new();
    for term in &terms.terms {
        if terms::is_long(term) {
            let n = push(terms::fts_string(term));
            let mut set = format!("SELECT rowid FROM tracks_tri WHERE tracks_tri MATCH ?{n}");
            if let Some(prefix) = terms::word_prefix(term) {
                let n = push(prefix);
                set.push_str(&format!(
                    " UNION SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ?{n}"
                ));
            }
            long.push(format!("t.id IN ({set})"));
        } else {
            let n = push(term.clone());
            short.push(SHORT_TERM.replace("{n}", &n.to_string()));
        }
    }
    long.extend(short);
    long.join(" AND ")
}

/// The condition on track `t` that one folded `term` matches, as plain search matches
/// each of its terms. Its values are pushed to `params`, numbered by their position.
/// The query language uses this for plain text.
pub(crate) fn term_condition(term: &str, params: &mut Vec<Value>) -> String {
    let mut push = |value: String| {
        params.push(Value::Text(value));
        params.len()
    };
    if terms::is_long(term) {
        let n = push(terms::fts_string(term));
        let mut set = format!("SELECT rowid FROM tracks_tri WHERE tracks_tri MATCH ?{n}");
        if let Some(prefix) = terms::word_prefix(term) {
            let n = push(prefix);
            set.push_str(&format!(
                " UNION SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ?{n}"
            ));
        }
        format!("t.id IN ({set})")
    } else {
        let n = push(term.to_owned());
        SHORT_TERM.replace("{n}", &n.to_string())
    }
}
