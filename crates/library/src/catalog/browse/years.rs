// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The release years of the library, and the conditions that pick the albums and
//! tracks of a span of years.
//!
//! The year of an album is its original release year where the tags give one,
//! otherwise the year of the release. A track without an album has the year of its
//! own tags, with the same preference.

use rusqlite::types::Value;
use rusqlite::{Connection, params};

use super::{Params, YearQuery, YearSpan, YearSummary};
use crate::catalog::roots::visibility;

/// The year of the track `t`, with the album `a` joined to it.
const TRACK_YEAR: &str = "CASE WHEN t.album_id IS NULL THEN coalesce(t.original_year, t.year)
    ELSE (SELECT coalesce(y.original_year, y.year) FROM albums y WHERE y.id = t.album_id) END";

/// Every year with albums or tracks, newest first, then the tracks without a year.
pub(crate) fn years(c: &Connection, query: &YearQuery) -> rusqlite::Result<Vec<YearSummary>> {
    let visible = visibility(c, query.include_masked)?;
    c.prepare_cached(&format!(
        "SELECT y, count(DISTINCT album_id), count(*) FROM
             (SELECT {TRACK_YEAR} AS y, t.album_id AS album_id FROM tracks t WHERE {visible})
         GROUP BY y ORDER BY y IS NULL, y DESC"
    ))?
    .query_map(params![], |row| {
        Ok(YearSummary {
            year: row.get(0)?,
            album_count: usize::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
            track_count: usize::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
        })
    })?
    .collect()
}

/// The condition of `span` on the albums `a`, its values pushed to `params`.
pub(super) fn album_condition(span: YearSpan, params: &mut Params) -> String {
    match span {
        YearSpan::Years { first, last } => {
            let n = params.len();
            params.push(Value::Integer(first.into()));
            params.push(Value::Integer(last.into()));
            format!(
                "coalesce(a.original_year, a.year) BETWEEN ?{} AND ?{}",
                n + 1,
                n + 2
            )
        }
        YearSpan::Unknown => "coalesce(a.original_year, a.year) IS NULL".to_owned(),
    }
}

/// The condition of `span` on the tracks `t`, its values pushed to `params`.
pub(super) fn track_condition(span: YearSpan, params: &mut Params) -> String {
    match span {
        YearSpan::Years { first, last } => {
            let n = params.len();
            params.push(Value::Integer(first.into()));
            params.push(Value::Integer(last.into()));
            format!("({TRACK_YEAR}) BETWEEN ?{} AND ?{}", n + 1, n + 2)
        }
        YearSpan::Unknown => format!("({TRACK_YEAR}) IS NULL"),
    }
}
