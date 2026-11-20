// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Applying a plan as one import run, undoing a run, and listing the runs.

use rusqlite::{Connection, OptionalExtension, params};

use super::merge::{self, CountRule, Stats};
use super::plan::{ImportPlan, ItemWork, PlaylistWork};
use super::{ImportError, ImportRun, ImportRunId, ImportRunKind};
use crate::db::unix_time::{from_unix_seconds, to_millis, to_unix_seconds};
use crate::playlists::{self, FOLDER, NewItem, PlaylistId, REF_PATH, SMART, STATIC};

/// `import_run_rows.kind` of a playlist made.
pub(super) const ROW_PLAYLIST: i64 = 0;
/// A saved search made.
pub(super) const ROW_SAVED_SEARCH: i64 = 1;
/// A history entry added.
pub(super) const ROW_HISTORY: i64 = 2;
/// A history entry moved from `from_identity`.
pub(super) const ROW_HISTORY_MOVED: i64 = 3;
/// A playlist item moved from `from_identity`.
pub(super) const ROW_ITEM_MOVED: i64 = 4;
/// An item added to Favourites.
pub(super) const ROW_FAVOURITE: i64 = 5;

fn kind_to_sql(kind: ImportRunKind) -> i64 {
    match kind {
        ImportRunKind::Player => 0,
        ImportRunKind::UserData => 1,
        ImportRunKind::Carry => 2,
    }
}

fn kind_from_sql(kind: i64) -> ImportRunKind {
    match kind {
        1 => ImportRunKind::UserData,
        2 => ImportRunKind::Carry,
        _ => ImportRunKind::Player,
    }
}

/// A new run of `kind` from `source`.
pub(super) fn start_run(
    c: &Connection,
    kind: ImportRunKind,
    source: &str,
    now: i64,
) -> rusqlite::Result<i64> {
    c.prepare_cached("INSERT INTO import_runs (kind, source, at) VALUES (?1, ?2, ?3) RETURNING id")?
        .query_row(params![kind_to_sql(kind), source, now], |row| row.get(0))
}

/// Keeps what `run` did in figures.
pub(super) fn set_summary(
    c: &Connection,
    run: i64,
    summary: &super::ImportSummary,
) -> rusqlite::Result<()> {
    let text = serde_json::to_string(summary).unwrap_or_default();
    c.prepare_cached("UPDATE import_runs SET summary = ?2 WHERE id = ?1")?
        .execute(params![run, text])?;
    Ok(())
}

/// Records that `run` made or moved the row `row` of `kind`.
pub(super) fn record_row(
    c: &Connection,
    run: i64,
    kind: i64,
    row: i64,
    from_identity: Option<i64>,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT OR IGNORE INTO import_run_rows (run_id, kind, row_id, from_identity)
         VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![run, kind, row, from_identity])?;
    Ok(())
}

/// Writes `after` for `identity` and records the change in `run`.
pub(super) fn change_stats(
    c: &Connection,
    run: i64,
    identity: i64,
    before: &Stats,
    existed: bool,
    after: &Stats,
) -> rusqlite::Result<()> {
    if after == before {
        return Ok(());
    }
    merge::write(c, identity, after, after.rating != before.rating)?;
    c.prepare_cached(
        "INSERT INTO import_run_stats (run_id, identity_id, existed,
             before_play_count, before_skip_count, before_first_played, before_last_played,
             before_last_skipped, before_rating, before_loved,
             after_play_count, after_skip_count, after_first_played, after_last_played,
             after_last_skipped, after_rating, after_loved)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
    )?
    .execute(params![
        run,
        identity,
        existed,
        before.play_count,
        before.skip_count,
        before.first_played,
        before.last_played,
        before.last_skipped,
        before.rating,
        before.loved,
        after.play_count,
        after.skip_count,
        after.first_played,
        after.last_played,
        after.last_skipped,
        after.rating,
        after.loved,
    ])?;
    Ok(())
}

/// Sets the baseline of `source` for `identity`, recording the old one in `run`.
fn set_baseline(
    c: &Connection,
    run: i64,
    source: &str,
    identity: i64,
    plays: i64,
    skips: i64,
) -> rusqlite::Result<()> {
    let old = super::plan::baseline(c, source, identity)?;
    if old.is_some_and(|b| b.play_count == plays && b.skip_count == skips) {
        return Ok(());
    }
    c.prepare_cached(
        "INSERT OR IGNORE INTO import_run_baselines (run_id, identity_id, play_count, skip_count)
         VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![
        run,
        identity,
        old.map(|b| b.play_count),
        old.map(|b| b.skip_count)
    ])?;
    c.prepare_cached(
        "INSERT INTO import_baselines (source, identity_id, play_count, skip_count)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (source, identity_id) DO UPDATE SET
             play_count = excluded.play_count, skip_count = excluded.skip_count",
    )?
    .execute(params![source, identity, plays, skips])?;
    Ok(())
}

/// The folder at `path` (names, outermost first), made where missing; the
/// folders made are recorded in `run`.
fn folder(
    c: &Connection,
    run: i64,
    path: &[String],
    now: i64,
) -> rusqlite::Result<Option<PlaylistId>> {
    let mut parent: Option<PlaylistId> = None;
    for name in path {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let found: Option<i64> = c
            .prepare_cached(
                "SELECT id FROM playlists WHERE kind = ?1 AND parent_id IS ?2 AND name = ?3
                 ORDER BY id LIMIT 1",
            )?
            .query_row(params![FOLDER, parent.map(|p| p.get()), name], |row| {
                row.get(0)
            })
            .optional()?;
        parent = Some(match found {
            Some(id) => PlaylistId(id),
            None => {
                let id = playlists::create_row(c, name, FOLDER, parent, None, now)?;
                record_row(c, run, ROW_PLAYLIST, id.get(), None)?;
                id
            }
        });
    }
    Ok(parent)
}

fn new_item(item: &ItemWork) -> NewItem {
    NewItem {
        identity: item.identity,
        ref_kind: REF_PATH,
        reference: item.path.clone(),
        title: item.title.clone(),
        artist: item.artist.clone(),
        album: item.album.clone(),
        duration_ms: item.duration_ms,
    }
}

/// Carries out `plan` as a new run; returns the run.
pub(super) fn apply(c: &Connection, plan: &ImportPlan, now: i64) -> rusqlite::Result<i64> {
    let run = start_run(c, plan.kind, &plan.source, now)?;
    set_summary(c, run, &plan.summary())?;
    let mut added = c.prepare_cached("SELECT added_at FROM tracks WHERE id = ?1")?;
    for (identity, incoming, tracks) in &plan.work.identities {
        let identity = *identity;
        let (before, existed) = merge::read(c, identity)?;
        let base = match plan.rule {
            CountRule::AddNew => super::plan::baseline(c, &plan.source, identity)?,
            CountRule::Max => None,
        };
        let after = merge::merge(&before, incoming, plan.rule, base, plan.replace_ratings);
        change_stats(c, run, identity, &before, existed, &after)?;
        if plan.rule == CountRule::AddNew {
            set_baseline(
                c,
                run,
                &plan.source,
                identity,
                incoming.play_count,
                incoming.skip_count,
            )?;
        }
        if let Some(earlier) = incoming.added {
            for &track in tracks {
                let now_added: Option<i64> = added.query_row([track], |r| r.get(0)).optional()?;
                if let Some(was) = now_added.filter(|&was| earlier < was) {
                    c.prepare_cached("UPDATE tracks SET added_at = ?2 WHERE id = ?1")?
                        .execute(params![track, earlier])?;
                    c.prepare_cached(
                        "INSERT OR IGNORE INTO import_run_added (run_id, track_id, before, after)
                         VALUES (?1, ?2, ?3, ?4)",
                    )?
                    .execute(params![run, track, was, earlier])?;
                }
            }
        }
    }
    for (name, path, work) in &plan.work.playlists {
        let parent = folder(c, run, path, now)?;
        let name = name.trim();
        let id = match work {
            PlaylistWork::Static(items) => {
                let id = playlists::create_row(c, name, STATIC, parent, None, now)?;
                let rows: Vec<NewItem> = items.iter().map(new_item).collect();
                playlists::insert_rows(c, id, 0, &rows)?;
                id
            }
            PlaylistWork::Smart(query) => {
                playlists::create_row(c, name, SMART, parent, Some(query), now)?
            }
        };
        record_row(c, run, ROW_PLAYLIST, id.get(), None)?;
    }
    for (identity, listen) in &plan.work.history {
        let id: i64 = c
            .prepare_cached(
                "INSERT INTO history (identity_id, started_at, played_ms, counted, context)
                 VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
            )?
            .query_row(
                params![
                    identity,
                    to_unix_seconds(listen.started_at),
                    to_millis(listen.played),
                    listen.counted,
                    listen.context
                ],
                |row| row.get(0),
            )?;
        record_row(c, run, ROW_HISTORY, id, None)?;
    }
    for (name, query) in &plan.work.saved_searches {
        let id: i64 = c
            .prepare_cached(
                "INSERT INTO saved_searches (name, query, position, created_at, modified_at)
                 VALUES (?1, ?2, (SELECT count(*) FROM saved_searches), ?3, ?3) RETURNING id",
            )?
            .query_row(params![name, query, now], |row| row.get(0))?;
        record_row(c, run, ROW_SAVED_SEARCH, id, None)?;
    }
    if !plan.work.favourites.is_empty() {
        let favourites = playlists::favourites(c)?;
        for &identity in &plan.work.favourites {
            let track: Option<i64> = c
                .prepare_cached(
                    "SELECT id FROM tracks WHERE identity_id = ?1 ORDER BY is_copy, id LIMIT 1",
                )?
                .query_row([identity], |row| row.get(0))
                .optional()?;
            let Some(track) = track else {
                continue;
            };
            let at = playlists::append_tracks(
                c,
                favourites,
                &[crate::catalog::track_id_from_sql(track)],
                now,
            )?;
            let item: i64 = c
                .prepare_cached(
                    "SELECT id FROM playlist_items WHERE playlist_id = ?1 AND position = ?2",
                )?
                .query_row(
                    params![favourites.get(), i64::try_from(at).unwrap_or(i64::MAX)],
                    |row| row.get(0),
                )?;
            record_row(c, run, ROW_FAVOURITE, item, None)?;
        }
    }
    super::labels_fields::apply(c, run, &plan.work.labels, &plan.work.fields, now)?;
    Ok(run)
}

fn stats_columns(row: &rusqlite::Row<'_>, from: usize) -> rusqlite::Result<Stats> {
    Ok(Stats {
        play_count: row.get(from)?,
        skip_count: row.get(from + 1)?,
        first_played: row.get(from + 2)?,
        last_played: row.get(from + 3)?,
        last_skipped: row.get(from + 4)?,
        rating: row.get(from + 5)?,
        loved: row.get(from + 6)?,
    })
}

/// The newest run that is not undone.
fn newest(c: &Connection) -> rusqlite::Result<Option<i64>> {
    c.prepare_cached("SELECT max(id) FROM import_runs WHERE undone = 0")?
        .query_row([], |row| row.get(0))
}

/// Undoes `run`, which must be the newest that is not undone.
pub(super) fn undo(
    c: &Connection,
    run: i64,
    now: i64,
) -> rusqlite::Result<Result<(), ImportError>> {
    match newest(c)? {
        Some(newest) if newest == run => {}
        Some(_) if run_exists(c, run)? => return Ok(Err(ImportError::NotNewest)),
        _ => return Ok(Err(ImportError::UnknownRun)),
    }
    // Statistics.
    let changes: Vec<(i64, Stats, Stats)> = c
        .prepare(
            "SELECT identity_id,
                    before_play_count, before_skip_count, before_first_played, before_last_played,
                    before_last_skipped, before_rating, before_loved,
                    after_play_count, after_skip_count, after_first_played, after_last_played,
                    after_last_skipped, after_rating, after_loved
             FROM import_run_stats WHERE run_id = ?1",
        )?
        .query_map([run], |row| {
            Ok((row.get(0)?, stats_columns(row, 1)?, stats_columns(row, 8)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (identity, before, after) in changes {
        let (current, _) = merge::read(c, identity)?;
        let undone = merge::undo(&current, &before, &after);
        if undone != current {
            merge::write(c, identity, &undone, undone.rating != current.rating)?;
        }
    }
    // Baselines.
    let baselines: Vec<(i64, Option<i64>, Option<i64>)> = c
        .prepare(
            "SELECT identity_id, play_count, skip_count FROM import_run_baselines
             WHERE run_id = ?1",
        )?
        .query_map([run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let source: String =
        c.query_row("SELECT source FROM import_runs WHERE id = ?1", [run], |r| {
            r.get(0)
        })?;
    for (identity, plays, skips) in baselines {
        match (plays, skips) {
            (Some(plays), Some(skips)) => {
                c.prepare_cached(
                    "UPDATE import_baselines SET play_count = ?3, skip_count = ?4
                     WHERE source = ?1 AND identity_id = ?2",
                )?
                .execute(params![source, identity, plays, skips])?;
            }
            _ => {
                c.prepare_cached(
                    "DELETE FROM import_baselines WHERE source = ?1 AND identity_id = ?2",
                )?
                .execute(params![source, identity])?;
            }
        }
    }
    // Dates added.
    c.execute(
        "UPDATE tracks SET added_at = (SELECT a.before FROM import_run_added a
                                       WHERE a.run_id = ?1 AND a.track_id = tracks.id)
         WHERE id IN (SELECT track_id FROM import_run_added WHERE run_id = ?1)
           AND added_at = (SELECT a.after FROM import_run_added a
                           WHERE a.run_id = ?1 AND a.track_id = tracks.id)",
        [run],
    )?;
    // Rows made or moved, the newest first.
    let rows: Vec<(i64, i64, Option<i64>)> = c
        .prepare(
            "SELECT kind, row_id, from_identity FROM import_run_rows WHERE run_id = ?1
             ORDER BY kind = 0, row_id DESC",
        )?
        .query_map([run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (kind, row, from) in rows {
        match kind {
            ROW_PLAYLIST => {
                let exists: Option<i64> = c
                    .prepare_cached("SELECT builtin FROM playlists WHERE id = ?1")?
                    .query_row([row], |r| r.get(0))
                    .optional()?;
                if exists == Some(0) {
                    playlists::delete(c, PlaylistId(row))?;
                }
            }
            ROW_SAVED_SEARCH => {
                c.prepare_cached("DELETE FROM saved_searches WHERE id = ?1")?
                    .execute([row])?;
                // Positions without gaps.
                c.execute(
                    "UPDATE saved_searches SET position = (SELECT count(*) FROM saved_searches s
                         WHERE s.position < saved_searches.position)",
                    [],
                )?;
            }
            ROW_HISTORY => {
                c.prepare_cached("DELETE FROM history WHERE id = ?1")?
                    .execute([row])?;
            }
            ROW_HISTORY_MOVED => {
                if let Some(from) = from {
                    c.prepare_cached("UPDATE history SET identity_id = ?2 WHERE id = ?1")?
                        .execute(params![row, from])?;
                }
            }
            ROW_ITEM_MOVED => {
                if let Some(from) = from {
                    c.prepare_cached("UPDATE playlist_items SET identity_id = ?2 WHERE id = ?1")?
                        .execute(params![row, from])?;
                }
            }
            ROW_FAVOURITE => {
                let at: Option<(i64, i64)> = c
                    .prepare_cached(
                        "SELECT playlist_id, position FROM playlist_items WHERE id = ?1",
                    )?
                    .query_row([row], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                if let Some((playlist, position)) = at {
                    playlists::remove_items(
                        c,
                        PlaylistId(playlist),
                        &[usize::try_from(position).unwrap_or(0)],
                        now,
                    )?;
                }
            }
            _ => {}
        }
    }
    super::labels_fields::undo(c, run)?;
    c.prepare_cached("UPDATE import_runs SET undone = 1, undone_at = ?2 WHERE id = ?1")?
        .execute(params![run, now])?;
    Ok(Ok(()))
}

fn run_exists(c: &Connection, run: i64) -> rusqlite::Result<bool> {
    c.prepare_cached("SELECT EXISTS (SELECT 1 FROM import_runs WHERE id = ?1 AND undone = 0)")?
        .query_row([run], |row| row.get(0))
}

/// Every run, newest first.
pub(super) fn runs(c: &Connection) -> rusqlite::Result<Vec<ImportRun>> {
    c.prepare(
        "SELECT r.id, r.kind, r.source, r.at, r.undone,
                (SELECT count(*) FROM import_run_stats s WHERE s.run_id = r.id),
                (SELECT count(*) FROM import_run_rows w WHERE w.run_id = r.id)
                + (SELECT count(*) FROM import_run_labels l WHERE l.run_id = r.id)
                + (SELECT count(*) FROM import_run_fields f WHERE f.run_id = r.id),
                r.undone_at, r.summary
         FROM import_runs r ORDER BY r.id DESC",
    )?
    .query_map([], |row| {
        Ok(ImportRun {
            id: ImportRunId(row.get(0)?),
            kind: kind_from_sql(row.get(1)?),
            source: row.get(2)?,
            at: from_unix_seconds(row.get(3)?),
            undone: row.get(4)?,
            tracks: usize::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
            rows: usize::try_from(row.get::<_, i64>(6)?).unwrap_or(0),
            undone_at: row.get::<_, Option<i64>>(7)?.map(from_unix_seconds),
            summary: row
                .get::<_, Option<String>>(8)?
                .and_then(|text| serde_json::from_str(&text).ok()),
        })
    })?
    .collect()
}
