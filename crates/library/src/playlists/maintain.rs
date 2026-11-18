// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keeping a static playlist tidy: removing repeated items and items that cannot
//! play, and shuffling its order for good. Each is one edit that can be undone.

use std::collections::HashSet;

use rusqlite::Connection;

use super::history::record;
use super::items::{item_ids, remove_items, renumber};
use super::store::{check_editable, check_static, touch};
use super::{CleanupCounts, PlaylistId};

/// What is needed of an item to tidy the playlist.
struct ItemFacts {
    position: usize,
    /// What makes two items the same: the identity of their track, or the path or URL
    /// of an entry that names no track.
    key: Option<(i64, Vec<u8>)>,
    playable: bool,
}

fn facts(c: &Connection, playlist: PlaylistId) -> rusqlite::Result<Vec<ItemFacts>> {
    c.prepare_cached(
        "SELECT i.position, i.identity_id, i.ref_kind, i.missing_ref,
                EXISTS (SELECT 1 FROM tracks t JOIN files f ON f.id = t.file_id
                        WHERE t.identity_id = i.identity_id AND f.missing_since IS NULL)
         FROM playlist_items i WHERE i.playlist_id = ?1 ORDER BY i.position",
    )?
    .query_map([playlist.0], |row| {
        let identity: Option<i64> = row.get(1)?;
        let reference: Option<Vec<u8>> = row.get(3)?;
        let key = match (identity, reference) {
            (Some(identity), _) => Some((-1, identity.to_le_bytes().to_vec())),
            (None, Some(bytes)) if !bytes.is_empty() => Some((row.get(2)?, bytes)),
            _ => None,
        };
        Ok(ItemFacts {
            position: usize::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
            key,
            playable: row.get(4)?,
        })
    })?
    .collect()
}

/// The positions of the items that repeat an earlier one.
fn duplicate_positions(items: &[ItemFacts]) -> Vec<usize> {
    let mut seen = HashSet::new();
    items
        .iter()
        .filter(|item| {
            item.key
                .as_ref()
                .is_some_and(|key| !seen.insert(key.clone()))
        })
        .map(|item| item.position)
        .collect()
}

/// The positions of the items that cannot play.
fn missing_positions(items: &[ItemFacts]) -> Vec<usize> {
    items
        .iter()
        .filter(|item| !item.playable)
        .map(|item| item.position)
        .collect()
}

/// What [`remove_duplicates`] and [`remove_missing`] would remove from `playlist`.
pub(crate) fn cleanup_counts(
    c: &Connection,
    playlist: PlaylistId,
) -> rusqlite::Result<CleanupCounts> {
    check_static(c, playlist)?;
    let items = facts(c, playlist)?;
    Ok(CleanupCounts {
        duplicates: duplicate_positions(&items).len(),
        missing: missing_positions(&items).len(),
    })
}

/// Removes the items that repeat an earlier one. Returns how many went.
pub(crate) fn remove_duplicates(
    c: &Connection,
    playlist: PlaylistId,
    now: i64,
) -> rusqlite::Result<usize> {
    check_editable(c, playlist)?;
    let positions = duplicate_positions(&facts(c, playlist)?);
    remove_items(c, playlist, &positions, now)
}

/// Removes the items that cannot play: entries that name no track, and tracks whose
/// file is missing. Returns how many went.
pub(crate) fn remove_missing(
    c: &Connection,
    playlist: PlaylistId,
    now: i64,
) -> rusqlite::Result<usize> {
    check_editable(c, playlist)?;
    let positions = missing_positions(&facts(c, playlist)?);
    remove_items(c, playlist, &positions, now)
}

/// The next number of a xorshift generator.
fn next(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// Puts the items of `playlist` in a random order chosen by `seed`, for good.
pub(crate) fn shuffle(
    c: &Connection,
    playlist: PlaylistId,
    seed: u64,
    now: i64,
) -> rusqlite::Result<()> {
    check_editable(c, playlist)?;
    let mut ids = item_ids(c, playlist)?;
    if ids.len() < 2 {
        return Ok(());
    }
    record(c, playlist, now)?;
    let mut state = seed | 1;
    for i in (1..ids.len()).rev() {
        let j = usize::try_from(next(&mut state) % (i as u64 + 1)).unwrap_or(0);
        ids.swap(i, j);
    }
    let len = ids.len();
    renumber(c, playlist, 0, len, &ids)?;
    touch(c, playlist, now)
}
