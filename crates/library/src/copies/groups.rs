// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Groups of copies on the writer's connection: who is in a group, which file is its
//! main copy, which tracks are hidden copies, and what happens when a file joins or
//! leaves.
//!
//! - **The main copy** is kept while it is available (present, and not on a masked
//!   library folder). When it is not, the next available file by the rule becomes
//!   main: the earliest added, then the shorter path, then the lower path in byte
//!   order. A main copy that comes back does not take the role back.
//! - **Tracks.** A track of another file of the group is a hidden copy (`is_copy`)
//!   when the main copy has the same track (the same section of the file). The
//!   tracks of one section share one identity.
//! - **Copies without tracks.** When the scan setting is to skip copies, only the
//!   main copy has tracks; when another file becomes main, the tracks move to it
//!   (their ids and identities stay). The other files are recorded with the state
//!   `STATE_COPY`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use helikodu_model::settings::DuplicateScan;
use rusqlite::{Connection, OptionalExtension, params};

use super::merge::{self, Gate};
use super::settings;
use crate::catalog::RootId;
use crate::catalog::albums::Regroup;
use crate::catalog::write::{finish_tracks, refresh_album_dir};
use crate::db::path_from_blob;
use crate::scan::store::{STATE_COPY, STATE_READ};
use crate::search::index;

/// What changes to groups leave for the end of the write, and what they found.
#[derive(Debug, Default)]
pub(crate) struct Work {
    regroup: Regroup,
    reindex_files: Vec<i64>,
    /// Files whose tracks must be read: a main copy without tracks, or a copy
    /// without tracks while copies get tracks.
    pub(crate) needs_read: Vec<i64>,
    pub(crate) gate: Gate,
    /// Groups with a merge that waits for the backup.
    pub(crate) blocked: Vec<i64>,
    pub(crate) mode: DuplicateScan,
    pub(crate) now: i64,
}

impl Work {
    /// Work under the settings stored in the database.
    pub(crate) fn new(c: &Connection, now: i64) -> rusqlite::Result<Self> {
        Ok(Self {
            gate: Gate {
                allowed: settings::backup_made(c)?,
                blocked: false,
            },
            mode: settings::on_scan(c)?,
            now,
            ..Self::default()
        })
    }

    /// Notes the albums of the tracks of `file`, which is about to go, for
    /// regrouping.
    pub(crate) fn regroup_file(&mut self, c: &Connection, file: i64) -> rusqlite::Result<()> {
        self.regroup.add_tracks_where(c, "file_id = ?1", file)
    }

    /// Regroups the albums of moved tracks and refreshes the search rows.
    pub(crate) fn finish(self, c: &Connection) -> rusqlite::Result<WorkResult> {
        let Self {
            regroup,
            mut reindex_files,
            mut needs_read,
            gate,
            mut blocked,
            ..
        } = self;
        finish_tracks(c, regroup, &[])?;
        reindex_files.sort_unstable();
        reindex_files.dedup();
        for file in reindex_files {
            index::reindex_file(c, file)?;
        }
        needs_read.sort_unstable();
        needs_read.dedup();
        blocked.sort_unstable();
        blocked.dedup();
        Ok(WorkResult {
            needs_read,
            blocked,
            merge_blocked: gate.blocked,
        })
    }
}

/// What [`Work::finish`] hands back.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct WorkResult {
    pub(crate) needs_read: Vec<i64>,
    pub(crate) blocked: Vec<i64>,
    pub(crate) merge_blocked: bool,
}

/// A file of a group.
#[derive(Debug, Clone)]
struct Member {
    file: i64,
    available: bool,
    added_at: i64,
    path: Vec<u8>,
    dev: Option<i64>,
    inode: Option<i64>,
    state: i64,
}

impl Member {
    fn same_file_as(&self, other: &Self) -> bool {
        self.dev.is_some() && self.dev == other.dev && self.inode == other.inode
    }

    /// The order of the main-copy rule.
    fn rank(&self) -> (i64, usize, &[u8]) {
        (self.added_at, self.path.len(), &self.path)
    }
}

/// The SQL condition that the file `f` in the folder `d` is available: present,
/// and not in a masked library folder whose tracks are hidden.
pub(crate) const AVAILABLE: &str = "(f.missing_since IS NULL AND NOT EXISTS (
    SELECT 1 FROM roots r WHERE r.id = d.root_id AND r.masked = 1 AND r.mask_when_missing = 1))";

fn members(c: &Connection, group: i64) -> rusqlite::Result<Vec<Member>> {
    let mut members: Vec<Member> = c
        .prepare_cached(&format!(
            "SELECT f.id, {AVAILABLE}, coalesce(f.added_at, 9223372036854775807), d.path,
                    f.name, f.dev, f.inode, coalesce(f.scan_state, 0)
             FROM files f JOIN dirs d ON d.id = f.dir_id
             WHERE f.copy_group = ?1"
        ))?
        .query_map([group], |row| {
            let dir = path_from_blob(row.get(3)?);
            let name = path_from_blob(row.get(4)?);
            Ok(Member {
                file: row.get(0)?,
                available: row.get(1)?,
                added_at: row.get(2)?,
                path: crate::db::path_to_blob(&dir.join(name)).to_vec(),
                dev: row.get(5)?,
                inode: row.get(6)?,
                state: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    members.sort_by(|a, b| a.rank().cmp(&b.rank()));
    Ok(members)
}

/// The part of its file a track is: its sub-track number and frame range.
type Section = (i64, Option<i64>, Option<i64>);

#[derive(Debug, Clone, Copy)]
struct GroupTrack {
    id: i64,
    file: i64,
    section: Section,
    identity: i64,
    is_copy: bool,
    copies: i64,
}

fn tracks_of(c: &Connection, files: &[i64]) -> rusqlite::Result<Vec<GroupTrack>> {
    let mut statement = c.prepare_cached(
        "SELECT id, file_id, sub_index, start_frame, end_frame, identity_id, is_copy, copies
         FROM tracks WHERE file_id = ?1 ORDER BY id",
    )?;
    let mut tracks = Vec::new();
    for file in files {
        let rows = statement.query_map([file], |row| {
            Ok(GroupTrack {
                id: row.get(0)?,
                file: row.get(1)?,
                section: (row.get(2)?, row.get(3)?, row.get(4)?),
                identity: row.get(5)?,
                is_copy: row.get(6)?,
                copies: row.get(7)?,
            })
        })?;
        for row in rows {
            tracks.push(row?);
        }
    }
    Ok(tracks)
}

/// The group of `file`, if it has one.
pub(crate) fn group_of(c: &Connection, file: i64) -> rusqlite::Result<Option<i64>> {
    Ok(
        c.prepare_cached("SELECT copy_group FROM files WHERE id = ?1")?
            .query_row([file], |row| row.get::<_, Option<i64>>(0))
            .optional()?
            .flatten(),
    )
}

/// The group of the byte-identical files of `size` and `hash`, created if needed.
pub(crate) fn ensure_group(c: &Connection, size: i64, hash: &[u8]) -> rusqlite::Result<i64> {
    c.prepare_cached(
        "INSERT INTO copy_groups (size, content_hash) VALUES (?1, ?2)
         ON CONFLICT (size, content_hash) DO NOTHING",
    )?
    .execute(params![size, hash])?;
    c.prepare_cached("SELECT id FROM copy_groups WHERE size = ?1 AND content_hash = ?2")?
        .query_row(params![size, hash], |row| row.get(0))
}

/// Puts `file` into `group`.
pub(crate) fn join(c: &Connection, file: i64, group: i64) -> rusqlite::Result<()> {
    c.prepare_cached("UPDATE files SET copy_group = ?2 WHERE id = ?1")?
        .execute(params![file, group])?;
    Ok(())
}

/// Marks `file` to be read: its state says read without tracks, which the next scan
/// reads (and the scan that runs now, at its end).
fn mark_for_read(c: &Connection, file: i64, work: &mut Work) -> rusqlite::Result<()> {
    c.prepare_cached("UPDATE files SET scan_state = ?2 WHERE id = ?1")?
        .execute(params![file, STATE_READ])?;
    work.needs_read.push(file);
    Ok(())
}

/// Moves the tracks of `from` to `to`, which has none, as when a file moves: the
/// tracks keep their ids and identities. The measurements move with them.
fn transfer(c: &Connection, from: i64, to: i64, work: &mut Work) -> rusqlite::Result<()> {
    work.regroup.add_tracks_where(c, "file_id = ?1", from)?;
    c.prepare_cached(
        "UPDATE loudness SET
             file_size = (SELECT size FROM files WHERE id = ?2),
             file_mtime_ns = (SELECT mtime_ns FROM files WHERE id = ?2)
         WHERE track_id IN (SELECT id FROM tracks WHERE file_id = ?1)
           AND file_size IS (SELECT size FROM files WHERE id = ?1)
           AND file_mtime_ns IS (SELECT mtime_ns FROM files WHERE id = ?1)",
    )?
    .execute(params![from, to])?;
    c.prepare_cached(
        "UPDATE tracks SET file_id = ?2 WHERE file_id = ?1
         AND sub_index NOT IN (SELECT sub_index FROM tracks WHERE file_id = ?2)",
    )?
    .execute(params![from, to])?;
    delete_tracks(c, from, work)?;
    let (root, dir, dir_id): (i64, Vec<u8>, i64) = c
        .prepare_cached(
            "SELECT d.root_id, d.path, d.id FROM files f JOIN dirs d ON d.id = f.dir_id
             WHERE f.id = ?1",
        )?
        .query_row([to], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let dir = path_from_blob(dir);
    refresh_album_dir(
        c,
        RootId(root),
        to,
        Path::new(&dir),
        dir_id,
        &mut work.regroup,
    )?;
    work.reindex_files.push(to);
    c.prepare_cached("UPDATE files SET scan_state = ?2 WHERE id = ?1")?
        .execute(params![to, STATE_READ])?;
    c.prepare_cached("UPDATE files SET scan_state = ?2 WHERE id = ?1")?
        .execute(params![from, STATE_COPY])?;
    Ok(())
}

/// Removes the tracks of `file`; their identities stay.
fn delete_tracks(c: &Connection, file: i64, work: &mut Work) -> rusqlite::Result<()> {
    work.regroup.add_tracks_where(c, "file_id = ?1", file)?;
    // Triggers take them out of the search indexes and remove what no track refers
    // to any more.
    c.prepare_cached("DELETE FROM tracks WHERE file_id = ?1")?
        .execute([file])?;
    Ok(())
}

/// Brings `group` up to date: dissolves it when fewer than two files are left,
/// picks its main copy, moves or removes tracks for the scan setting, merges the
/// identities of each section and sets the copy marks and counts of its tracks.
pub(crate) fn refresh(c: &Connection, group: i64, work: &mut Work) -> rusqlite::Result<()> {
    let current: Option<Option<i64>> = c
        .prepare_cached("SELECT primary_file FROM copy_groups WHERE id = ?1")?
        .query_row([group], |row| row.get(0))
        .optional()?;
    let Some(current) = current else {
        return Ok(());
    };
    let members = members(c, group)?;
    if members.len() < 2 {
        return dissolve(c, group, &members, work);
    }
    let files: Vec<i64> = members.iter().map(|m| m.file).collect();
    let by_file = |file: i64| members.iter().find(|m| m.file == file);
    let first_available = members.iter().find(|m| m.available);
    let primary = match current.and_then(by_file) {
        Some(m) if m.available || first_available.is_none() => m.file,
        _ => first_available.unwrap_or(&members[0]).file,
    };
    if current != Some(primary) {
        c.prepare_cached("UPDATE copy_groups SET primary_file = ?2 WHERE id = ?1")?
            .execute(params![group, primary])?;
    }

    let mut tracks = tracks_of(c, &files)?;
    let has_tracks = |tracks: &[GroupTrack], file: i64| tracks.iter().any(|t| t.file == file);
    match work.mode {
        DuplicateScan::Skip => {
            if !has_tracks(&tracks, primary) {
                // Available files first, each kind in the order of the rule.
                let donor = members
                    .iter()
                    .filter(|m| m.available)
                    .chain(members.iter().filter(|m| !m.available))
                    .find(|m| m.file != primary && has_tracks(&tracks, m.file));
                match donor {
                    Some(donor) => {
                        transfer(c, donor.file, primary, work)?;
                        tracks = tracks_of(c, &files)?;
                    }
                    None => {
                        if by_file(primary).is_some_and(|m| m.available) {
                            mark_for_read(c, primary, work)?;
                        }
                    }
                }
            }
        }
        _ => {
            for m in &members {
                if m.available && m.state == STATE_COPY && !has_tracks(&tracks, m.file) {
                    mark_for_read(c, m.file, work)?;
                }
            }
        }
    }

    // One identity per section: the main copy's, or else the first file's.
    let rank = |file: i64| {
        if file == primary {
            0
        } else {
            1 + files.iter().position(|&f| f == file).unwrap_or(files.len())
        }
    };
    let mut sections: BTreeMap<Section, Vec<GroupTrack>> = BTreeMap::new();
    for t in &tracks {
        sections.entry(t.section).or_default().push(*t);
    }
    let mut blocked: BTreeSet<Section> = BTreeSet::new();
    for (section, list) in &sections {
        let Some(target) = list.iter().min_by_key(|t| (rank(t.file), t.id)) else {
            continue;
        };
        let others: Vec<i64> = list.iter().map(|t| t.identity).collect();
        if merge::merge(c, target.identity, &others, &mut work.gate)? {
            for t in tracks.iter_mut().filter(|t| t.section == *section) {
                t.identity = target.identity;
            }
        } else {
            blocked.insert(*section);
            work.blocked.push(group);
        }
    }

    let primary_sections: BTreeSet<Section> = tracks
        .iter()
        .filter(|t| t.file == primary)
        .map(|t| t.section)
        .collect();
    if work.mode == DuplicateScan::Skip && !primary_sections.is_empty() {
        for m in members.iter().filter(|m| m.file != primary) {
            let own: Vec<&GroupTrack> = tracks.iter().filter(|t| t.file == m.file).collect();
            let all_copies = own
                .iter()
                .all(|t| primary_sections.contains(&t.section) && !blocked.contains(&t.section));
            if all_copies {
                if !own.is_empty() {
                    delete_tracks(c, m.file, work)?;
                }
                if m.state != STATE_COPY {
                    c.prepare_cached("UPDATE files SET scan_state = ?2 WHERE id = ?1")?
                        .execute(params![m.file, STATE_COPY])?;
                }
            }
        }
        tracks = tracks_of(c, &files)?;
    }

    // The marks and counts.
    let mut update =
        c.prepare_cached("UPDATE tracks SET is_copy = ?2, copies = ?3 WHERE id = ?1")?;
    for t in &tracks {
        let is_copy = t.file != primary && primary_sections.contains(&t.section);
        let Some(own) = by_file(t.file) else {
            continue;
        };
        let copies = members
            .iter()
            .filter(|m| m.file != t.file && m.available && !m.same_file_as(own))
            .filter(|m| {
                // A copy without tracks holds every track of the main copy.
                !has_tracks(&tracks, m.file)
                    || tracks
                        .iter()
                        .any(|o| o.file == m.file && o.section == t.section)
            })
            .count();
        let copies = i64::try_from(copies).unwrap_or(i64::MAX);
        if t.is_copy != is_copy || t.copies != copies {
            update.execute(params![t.id, is_copy, copies])?;
        }
    }
    Ok(())
}

/// Ends `group`, which has fewer than two files: its file stands alone again and
/// is read if it has no tracks.
fn dissolve(
    c: &Connection,
    group: i64,
    members: &[Member],
    work: &mut Work,
) -> rusqlite::Result<()> {
    for m in members {
        c.prepare_cached("UPDATE files SET copy_group = NULL WHERE id = ?1")?
            .execute([m.file])?;
        c.prepare_cached(
            "UPDATE tracks SET is_copy = 0, copies = 0
             WHERE file_id = ?1 AND (is_copy != 0 OR copies != 0)",
        )?
        .execute([m.file])?;
        let has_tracks: bool = c
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM tracks WHERE file_id = ?1)")?
            .query_row([m.file], |row| row.get(0))?;
        if !has_tracks && m.state == STATE_COPY {
            mark_for_read(c, m.file, work)?;
        }
    }
    c.prepare_cached("DELETE FROM copy_groups WHERE id = ?1")?
        .execute([group])?;
    Ok(())
}

/// Takes `file` out of its group because it is no longer a copy (its content
/// changed). The identities it shared split: the main copy keeps them and the
/// others get new ones with a copy of the statistics, history and playlist entries
/// staying with the main copy. Its group is refreshed.
pub(crate) fn leave(c: &Connection, file: i64, work: &mut Work) -> rusqlite::Result<()> {
    let found: Option<(i64, Option<i64>)> = c
        .prepare_cached(
            "SELECT f.copy_group, g.primary_file FROM files f
             JOIN copy_groups g ON g.id = f.copy_group WHERE f.id = ?1",
        )?
        .query_row([file], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((group, primary)) = found else {
        return Ok(());
    };
    let own: BTreeSet<i64> = c
        .prepare_cached("SELECT DISTINCT identity_id FROM tracks WHERE file_id = ?1")?
        .query_map([file], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let others: BTreeSet<i64> = c
        .prepare_cached(
            "SELECT DISTINCT t.identity_id FROM tracks t JOIN files f ON f.id = t.file_id
             WHERE f.copy_group = ?1 AND f.id != ?2",
        )?
        .query_map(params![group, file], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let was_primary = primary == Some(file);
    for &identity in own.intersection(&others) {
        let new = merge::split_off(c, identity, work.now)?;
        if was_primary {
            c.prepare_cached(
                "UPDATE tracks SET identity_id = ?2 WHERE identity_id = ?1
                 AND file_id IN (SELECT id FROM files WHERE copy_group = ?3 AND id != ?4)",
            )?
            .execute(params![identity, new, group, file])?;
        } else {
            c.prepare_cached(
                "UPDATE tracks SET identity_id = ?2 WHERE identity_id = ?1 AND file_id = ?3",
            )?
            .execute(params![identity, new, file])?;
        }
    }
    detach(c, file, group, work)
}

/// Takes `file` out of `group` without touching identities, and refreshes the
/// group.
fn detach(c: &Connection, file: i64, group: i64, work: &mut Work) -> rusqlite::Result<()> {
    c.prepare_cached("UPDATE files SET copy_group = NULL WHERE id = ?1")?
        .execute([file])?;
    c.prepare_cached("UPDATE tracks SET is_copy = 0, copies = 0 WHERE file_id = ?1")?
        .execute([file])?;
    let (has_tracks, state): (bool, i64) = c
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM tracks WHERE file_id = ?1), coalesce(scan_state, 0)
             FROM files WHERE id = ?1",
        )?
        .query_row([file], |row| Ok((row.get(0)?, row.get(1)?)))?;
    if !has_tracks && state == STATE_COPY {
        mark_for_read(c, file, work)?;
    }
    refresh(c, group, work)
}

/// Prepares the removal of the files `files` from the library: a main copy's
/// tracks move to the next copy first when copies have no tracks of their own, and
/// each file leaves its group.
pub(crate) fn before_removal(
    c: &Connection,
    files: &[i64],
    work: &mut Work,
) -> rusqlite::Result<()> {
    for &file in files {
        let Some(group) = group_of(c, file)? else {
            continue;
        };
        let primary: Option<i64> = c
            .prepare_cached("SELECT primary_file FROM copy_groups WHERE id = ?1")?
            .query_row([group], |row| row.get(0))?;
        if primary == Some(file) && work.mode == DuplicateScan::Skip {
            let next = members(c, group)?
                .into_iter()
                .filter(|m| m.file != file && !files.contains(&m.file))
                .min_by_key(|m| !m.available);
            if let Some(next) = next {
                let has_tracks: bool = c
                    .prepare_cached("SELECT EXISTS (SELECT 1 FROM tracks WHERE file_id = ?1)")?
                    .query_row([next.file], |row| row.get(0))?;
                if !has_tracks {
                    transfer(c, file, next.file, work)?;
                }
                c.prepare_cached("UPDATE copy_groups SET primary_file = ?2 WHERE id = ?1")?
                    .execute(params![group, next.file])?;
            }
        }
        detach(c, file, group, work)?;
    }
    Ok(())
}

/// Refreshes the groups of the files `files`.
pub(crate) fn refresh_groups_of(
    c: &Connection,
    files: &[i64],
    work: &mut Work,
) -> rusqlite::Result<()> {
    let mut groups = BTreeSet::new();
    for &file in files {
        if let Some(group) = group_of(c, file)? {
            groups.insert(group);
        }
    }
    for group in groups {
        refresh(c, group, work)?;
    }
    Ok(())
}

/// The identity a new track of `file` takes when the file is a copy: that of the
/// same section in another file of its group, the main copy's first.
pub(crate) fn shared_identity(
    c: &Connection,
    file: i64,
    section: Section,
) -> rusqlite::Result<Option<i64>> {
    c.prepare_cached(
        "SELECT t.identity_id FROM files f
         JOIN copy_groups g ON g.id = f.copy_group
         JOIN files o ON o.copy_group = f.copy_group AND o.id != f.id
         JOIN tracks t ON t.file_id = o.id
         WHERE f.id = ?1 AND t.sub_index = ?2 AND t.start_frame IS ?3 AND t.end_frame IS ?4
         ORDER BY o.id = g.primary_file DESC, t.id LIMIT 1",
    )?
    .query_row(params![file, section.0, section.1, section.2], |row| {
        row.get(0)
    })
    .optional()
}

/// Whether the file at `file`, read with `size` and `quick_hash`, is a copy that
/// gets no tracks: copies are skipped, it is unchanged, in a group and not its
/// main copy, and the main copy has tracks.
pub(crate) fn is_skipped_copy(
    c: &Connection,
    file: i64,
    size: i64,
    quick_hash: &[u8],
) -> rusqlite::Result<bool> {
    if settings::on_scan(c)? != DuplicateScan::Skip {
        return Ok(false);
    }
    c.prepare_cached(
        "SELECT EXISTS (
             SELECT 1 FROM files f JOIN copy_groups g ON g.id = f.copy_group
             WHERE f.id = ?1 AND g.primary_file IS NOT f.id
               AND f.size = ?2 AND f.quick_hash = ?3
               AND EXISTS (SELECT 1 FROM tracks t WHERE t.file_id = g.primary_file))",
    )?
    .query_row(params![file, size, quick_hash], |row| row.get(0))
}
