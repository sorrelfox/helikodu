// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Running a compiled query: one page of tracks with the total, or every track id.

use helikodu_model::library::TrackId;
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, params_from_iter};

use super::compiler::{Compiled, FROM};
use crate::catalog::roots::visibility;
use crate::catalog::{COLUMNS, Page, Paged, TrackSummary, summary, track_id_from_sql};

/// The local time zone's offset from UTC at `now`, as SQLite's `localtime` sees it.
pub(crate) fn utc_offset(c: &Connection, now: i64) -> rusqlite::Result<i64> {
    c.prepare_cached("SELECT unixepoch(?1, 'unixepoch', 'localtime') - ?1")?
        .query_row([now], |row| row.get::<_, Option<i64>>(0))
        .map(Option::unwrap_or_default)
}

/// The condition with the masked library folders left out.
pub(super) fn condition(
    c: &Connection,
    compiled: &Compiled,
    include_masked: bool,
) -> rusqlite::Result<String> {
    let visible = visibility(c, include_masked)?;
    Ok(format!("({}) AND {visible}", compiled.condition))
}

/// The ids of the tracks a limit keeps, in order; `None` without a limit. A limit by
/// time or size adds up the sorted tracks until the next one would pass it.
pub(super) fn kept(
    c: &Connection,
    compiled: &Compiled,
    condition: &str,
) -> rusqlite::Result<Option<Vec<i64>>> {
    let Some(limit) = compiled.limit else {
        return Ok(None);
    };
    let mut params = compiled.params.clone();
    let order = &compiled.order;
    let Some(measure) = limit.measure else {
        params.push(SqlValue::Integer(limit.amount));
        let n = params.len();
        return c
            .prepare_cached(&format!(
                "SELECT t.id {FROM} WHERE {condition} ORDER BY {order} LIMIT ?{n}"
            ))?
            .query_map(params_from_iter(&params), |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()
            .map(Some);
    };
    let mut statement = c.prepare_cached(&format!(
        "SELECT t.id, {measure} {FROM} WHERE {condition} ORDER BY {order}"
    ))?;
    let mut rows = statement.query(params_from_iter(&params))?;
    let mut ids = Vec::new();
    let mut total: i64 = 0;
    while let Some(row) = rows.next()? {
        total = total.saturating_add(row.get(1)?);
        if total > limit.amount {
            break;
        }
        ids.push(row.get(0)?);
    }
    Ok(Some(ids))
}

/// The joins of [`COLUMNS`] onto the ids in the JSON array `?1`, in its order.
const KEPT_FROM: &str = "FROM json_each(?1) k
    JOIN tracks t ON t.id = k.value
    JOIN files f ON f.id = t.file_id
    LEFT JOIN albums a ON a.id = t.album_id";

/// The tracks with the ids `ids`, in that order.
pub(crate) fn summaries(c: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<TrackSummary>> {
    let list: Vec<String> = ids.iter().map(i64::to_string).collect();
    let json = format!("[{}]", list.join(","));
    c.prepare_cached(&format!("SELECT {COLUMNS} {KEPT_FROM} ORDER BY k.key"))?
        .query_map([json], summary)?
        .collect()
}

/// One page of the tracks of `compiled`, and how many there are in all.
pub(crate) fn tracks(
    c: &Connection,
    compiled: &Compiled,
    include_masked: bool,
    page: Page,
) -> rusqlite::Result<Paged<TrackSummary>> {
    let condition = condition(c, compiled, include_masked)?;
    let (limit, offset) = page.sql();
    if let Some(kept) = kept(c, compiled, &condition)? {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(kept.len());
        let end = start
            .saturating_add(usize::try_from(limit).unwrap_or(0))
            .min(kept.len());
        return Ok(Paged {
            items: summaries(c, &kept[start..end])?,
            total: kept.len(),
        });
    }
    let mut params = compiled.params.clone();
    let total: i64 = c
        .prepare_cached(&format!("SELECT count(*) {FROM} WHERE {condition}"))?
        .query_row(params_from_iter(&params), |row| row.get(0))?;
    let n = params.len();
    params.push(SqlValue::Integer(limit));
    params.push(SqlValue::Integer(offset));
    let items = c
        .prepare_cached(&format!(
            "SELECT {COLUMNS} {FROM} WHERE {condition} ORDER BY {} LIMIT ?{} OFFSET ?{}",
            compiled.order,
            n + 1,
            n + 2
        ))?
        .query_map(params_from_iter(&params), summary)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Paged {
        items,
        total: usize::try_from(total).unwrap_or(0),
    })
}

/// Every track of `compiled`, in order.
pub(crate) fn track_ids(
    c: &Connection,
    compiled: &Compiled,
    include_masked: bool,
) -> rusqlite::Result<Vec<TrackId>> {
    let condition = condition(c, compiled, include_masked)?;
    if let Some(kept) = kept(c, compiled, &condition)? {
        return Ok(kept.into_iter().map(track_id_from_sql).collect());
    }
    c.prepare_cached(&format!(
        "SELECT t.id {FROM} WHERE {condition} ORDER BY {}",
        compiled.order
    ))?
    .query_map(params_from_iter(&compiled.params), |row| {
        row.get(0).map(track_id_from_sql)
    })?
    .collect()
}

/// How many tracks `compiled` selects.
pub(crate) fn count(
    c: &Connection,
    compiled: &Compiled,
    include_masked: bool,
) -> rusqlite::Result<usize> {
    let condition = condition(c, compiled, include_masked)?;
    if let Some(kept) = kept(c, compiled, &condition)? {
        return Ok(kept.len());
    }
    let total: i64 = c
        .prepare_cached(&format!("SELECT count(*) {FROM} WHERE {condition}"))?
        .query_row(params_from_iter(&compiled.params), |row| row.get(0))?;
    Ok(usize::try_from(total).unwrap_or(0))
}
