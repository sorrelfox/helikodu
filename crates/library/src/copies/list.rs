// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading groups of copies: the group of a track, and every group.

use std::path::PathBuf;

use helikodu_model::library::TrackId;
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::groups::AVAILABLE;
use super::{CopyFile, CopyGroup, FileId};
use crate::catalog::{sql_track_id, track_id_from_sql};
use crate::db::unix_time::from_unix_seconds;
use crate::db::{display_text, path_from_blob};

/// The columns [`copy_file`] reads, from the files `f`, folders `d`, roots `r` and
/// groups `g`; `?2` is the asked track's file and `?3..?5` its section.
const FILE_COLUMNS: &str = "f.id, d.path, f.name, r.path, f.size, f.added_at,
    f.id = g.primary_file, NOT {AVAILABLE}, f.dev, f.inode,
    (SELECT t.codec FROM tracks t WHERE t.file_id = f.id ORDER BY t.sub_index LIMIT 1),
    (SELECT t.id FROM tracks t WHERE t.file_id = f.id AND t.sub_index = ?2
        AND t.start_frame IS ?3 AND t.end_frame IS ?4),
    (SELECT t.codec FROM tracks t JOIN files m ON m.id = t.file_id
        WHERE m.copy_group = g.id ORDER BY t.file_id = g.primary_file DESC LIMIT 1)";

fn copy_file(row: &Row<'_>) -> rusqlite::Result<(CopyFile, Option<i64>, Option<i64>)> {
    let dir = path_from_blob(row.get(1)?);
    let name = path_from_blob(row.get(2)?);
    let path = dir.join(name);
    let format: Option<String> = row.get::<_, Option<String>>(10)?.or(row.get(12)?);
    Ok((
        CopyFile {
            file: FileId(row.get(0)?),
            display_path: display_text(path.as_os_str()).into_owned(),
            path,
            root: path_from_blob(row.get(3)?),
            size: u64::try_from(row.get::<_, Option<i64>>(4)?.unwrap_or(0)).unwrap_or(0),
            added: row.get::<_, Option<i64>>(5)?.map(from_unix_seconds),
            is_main: row.get(6)?,
            missing: row.get(7)?,
            same_file_as: None,
            format,
            track: row.get::<_, Option<i64>>(11)?.map(track_id_from_sql),
        },
        row.get(8)?,
        row.get(9)?,
    ))
}

/// The files of the group `group`, the main copy first, then by the main-copy rule;
/// `track` names their tracks of the section `section`.
pub(crate) fn group_files(
    c: &Connection,
    group: i64,
    section: (i64, Option<i64>, Option<i64>),
) -> rusqlite::Result<Option<CopyGroup>> {
    let columns = FILE_COLUMNS.replace("{AVAILABLE}", AVAILABLE);
    let rows: Vec<(CopyFile, Option<i64>, Option<i64>)> = c
        .prepare_cached(&format!(
            "SELECT {columns}
             FROM files f JOIN dirs d ON d.id = f.dir_id JOIN roots r ON r.id = d.root_id
             JOIN copy_groups g ON g.id = f.copy_group
             WHERE f.copy_group = ?1
             ORDER BY f.id = g.primary_file DESC, f.added_at,
                      length(d.path) + length(f.name), d.path, f.name"
        ))?
        .query_map(params![group, section.0, section.1, section.2], copy_file)?
        .collect::<rusqlite::Result<_>>()?;
    let Some(main) = rows.iter().find(|(f, ..)| f.is_main).map(|(f, ..)| f.file) else {
        return Ok(None);
    };
    let keys: Vec<(FileId, Option<i64>, Option<i64>)> = rows
        .iter()
        .map(|(f, dev, inode)| (f.file, *dev, *inode))
        .collect();
    let files = rows
        .into_iter()
        .map(|(mut file, dev, inode)| {
            if dev.is_some() {
                file.same_file_as = keys
                    .iter()
                    .find(|(other, d, i)| *other != file.file && *d == dev && *i == inode)
                    .map(|(other, ..)| *other);
            }
            file
        })
        .collect();
    Ok(Some(CopyGroup { main, files }))
}

/// The group of the file of `track`, with that track's section.
pub(crate) fn copies_of(c: &Connection, track: TrackId) -> rusqlite::Result<Option<CopyGroup>> {
    // The group and the section of the track.
    type Found = (Option<i64>, i64, Option<i64>, Option<i64>);
    let found: Option<Found> = c
        .prepare_cached(
            "SELECT f.copy_group, t.sub_index, t.start_frame, t.end_frame
             FROM tracks t JOIN files f ON f.id = t.file_id WHERE t.id = ?1",
        )?
        .query_row([sql_track_id(track)], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?;
    let Some((Some(group), sub_index, start, end)) = found else {
        return Ok(None);
    };
    group_files(c, group, (sub_index, start, end))
}

/// Every group of copies, in the order of their main copies' paths.
pub(crate) fn all_groups(c: &Connection) -> rusqlite::Result<Vec<CopyGroup>> {
    let groups: Vec<i64> = c
        .prepare(
            "SELECT g.id FROM copy_groups g
             LEFT JOIN files f ON f.id = g.primary_file LEFT JOIN dirs d ON d.id = f.dir_id
             ORDER BY d.path, f.name, g.id",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::with_capacity(groups.len());
    for group in groups {
        if let Some(found) = group_files(c, group, (0, None, None))? {
            out.push(found);
        }
    }
    Ok(out)
}

/// The path of the file `file`, and its stamp: size, modification time, device,
/// inode, whether it is missing, its group.
#[derive(Debug, Clone)]
pub(crate) struct FileFacts {
    pub(crate) path: PathBuf,
    pub(crate) size: Option<i64>,
    pub(crate) mtime_ns: Option<i64>,
    pub(crate) dev: Option<i64>,
    pub(crate) inode: Option<i64>,
    pub(crate) missing: bool,
    pub(crate) group: Option<i64>,
    pub(crate) primary: Option<i64>,
}

pub(crate) fn file_facts(c: &Connection, file: i64) -> rusqlite::Result<Option<FileFacts>> {
    c.prepare_cached(&format!(
        "SELECT d.path, f.name, f.size, f.mtime_ns, f.dev, f.inode, NOT {AVAILABLE},
                f.copy_group, g.primary_file
         FROM files f JOIN dirs d ON d.id = f.dir_id
         LEFT JOIN copy_groups g ON g.id = f.copy_group
         WHERE f.id = ?1"
    ))?
    .query_row([file], |row| {
        let dir = path_from_blob(row.get(0)?);
        let name = path_from_blob(row.get(1)?);
        Ok(FileFacts {
            path: dir.join(name),
            size: row.get(2)?,
            mtime_ns: row.get(3)?,
            dev: row.get(4)?,
            inode: row.get(5)?,
            missing: row.get(6)?,
            group: row.get(7)?,
            primary: row.get(8)?,
        })
    })
    .optional()
}

/// The other library files recorded as the same file as `file` (the same device
/// and inode), and the other files of its group, by id.
pub(crate) fn related_files(c: &Connection, file: i64) -> rusqlite::Result<Vec<i64>> {
    c.prepare_cached(
        "SELECT o.id FROM files f JOIN files o ON o.dev = f.dev AND o.inode = f.inode
         WHERE f.id = ?1 AND o.id != f.id
         UNION
         SELECT o.id FROM files f JOIN files o ON o.copy_group = f.copy_group
         WHERE f.id = ?1 AND o.id != f.id",
    )?
    .query_map([file], |row| row.get(0))?
    .collect()
}
