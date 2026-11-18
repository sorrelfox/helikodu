// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The duplicate finder: groups of files that hold the same music by one of four
//! measures, with the files a keep rule suggests moving to the trash.

use std::path::PathBuf;
use std::time::Duration;

use rusqlite::{Connection, Row};

use super::FileId;
use super::groups::AVAILABLE;
use crate::catalog::LOSSLESS;
use crate::catalog::track_id_from_sql;
use crate::db::unix_time::{from_millis, from_unix_seconds};
use crate::db::{display_text, path_from_blob};
use helikodu_model::library::TrackId;

/// How files are matched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MatchBy {
    /// Byte-identical files: the groups of copies.
    #[default]
    IdenticalFiles,
    /// One file reached by two paths (a hard link, a bind mount, a folder reached
    /// through a symbolic link). Reported only: none of them is ever moved.
    SameFile,
    /// The same recording: the same title and artist, lengths within 2 s.
    SameRecording,
    /// The same track (disc and number) of the same album.
    SameAlbumTrack,
}

/// Which file of a group to keep; the others are suggested for the trash.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeepRule {
    /// The best quality: lossless first, then the larger bit depth, sample rate,
    /// bit rate and size.
    #[default]
    BestQuality,
    /// The one that entered the library first.
    Oldest,
    /// The one played most often (then the oldest).
    MostPlayed,
    /// None: the owner marks the files by hand.
    ByHand,
}

/// One file of a group of duplicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateEntry {
    /// The file.
    pub file: FileId,
    /// Its track that matched (`None` for a copy without tracks of its own).
    pub track: Option<TrackId>,
    /// The path, as the bytes the kernel returned.
    pub path: PathBuf,
    /// The path for display.
    pub display_path: String,
    /// The codec, such as `FLAC`.
    pub codec: Option<String>,
    /// The sample rate in Hz.
    pub sample_rate: Option<u32>,
    /// The bit depth.
    pub bit_depth: Option<u32>,
    /// The bit rate in bit/s.
    pub bitrate: Option<u32>,
    /// The size in bytes.
    pub size: u64,
    /// How often its track was played.
    pub plays: u64,
    /// When the file entered the library.
    pub added: Option<std::time::SystemTime>,
    /// Whether it is the main copy of its group of copies.
    pub is_main: bool,
    /// Whether another entry of the group is the same file reached by another path.
    pub same_file: bool,
    /// Whether it is missing (or on a drive that is not connected).
    pub missing: bool,
    /// Whether it may be moved to the trash from this group: present, a whole file
    /// (not one track of a CUE sheet or of chapters), not the same file as another
    /// entry, and for copies not the main copy.
    pub removable: bool,
    /// Whether the keep rule suggests moving it to the trash.
    pub marked: bool,
}

/// A group of duplicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    /// The title of the kept (or first) entry's track.
    pub title: Option<String>,
    /// Its artist.
    pub artist: Option<String>,
    /// Its length.
    pub duration: Option<Duration>,
    /// The files, in the order of the main-copy rule (the main copy first).
    pub entries: Vec<DuplicateEntry>,
    /// The file the keep rule keeps; `None` by hand.
    pub keep: Option<FileId>,
}

/// How many groups each measure finds, for the list of measures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DuplicateCounts {
    /// Groups of byte-identical files.
    pub identical_files: usize,
    /// Files reached by more than one path.
    pub same_file: usize,
    /// Groups of the same recording.
    pub same_recording: usize,
    /// Groups of the same album track.
    pub same_album_track: usize,
}

/// A row before grouping.
#[derive(Debug, Clone)]
struct Candidate {
    key: (String, String, i64),
    duration_ms: Option<i64>,
    title: Option<String>,
    artist: Option<String>,
    entry: DuplicateEntry,
    lossless: bool,
}

/// The columns of [`candidate`], from tracks `t` (may be NULL), files `f`, folders
/// `d`, groups `g` and stats `s`; the first three columns are the key.
const COLUMNS: &str = "f.id, t.id, d.path, f.name, t.codec, t.sample_rate, t.bit_depth,
    t.bitrate, f.size, coalesce(s.play_count, 0), f.added_at, f.id IS g.primary_file,
    NOT {AVAILABLE}, f.dev, f.inode,
    (t.id IS NULL OR (SELECT count(*) FROM tracks o WHERE o.file_id = f.id) = 1),
    t.title, t.artist, t.duration_ms, coalesce({LOSSLESS}, 0)";

fn candidate(row: &Row<'_>) -> rusqlite::Result<Candidate> {
    let key = (
        row.get::<_, Option<String>>(0)?.unwrap_or_default(),
        row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        row.get::<_, Option<i64>>(2)?.unwrap_or(0),
    );
    let at = 3;
    let dir = path_from_blob(row.get(at + 2)?);
    let name = path_from_blob(row.get(at + 3)?);
    let path = dir.join(name);
    let whole: bool = row.get(at + 15)?;
    let missing: bool = row.get(at + 12)?;
    Ok(Candidate {
        key,
        duration_ms: row.get(at + 18)?,
        title: row.get(at + 16)?,
        artist: row.get(at + 17)?,
        lossless: row.get(at + 19)?,
        entry: DuplicateEntry {
            file: FileId(row.get(at)?),
            track: row.get::<_, Option<i64>>(at + 1)?.map(track_id_from_sql),
            display_path: display_text(path.as_os_str()).into_owned(),
            path,
            codec: row.get(at + 4)?,
            sample_rate: row.get(at + 5)?,
            bit_depth: row.get(at + 6)?,
            bitrate: row.get(at + 7)?,
            size: u64::try_from(row.get::<_, Option<i64>>(at + 8)?.unwrap_or(0)).unwrap_or(0),
            plays: u64::try_from(row.get::<_, i64>(at + 9)?).unwrap_or(0),
            added: row.get::<_, Option<i64>>(at + 10)?.map(from_unix_seconds),
            is_main: row.get(at + 11)?,
            same_file: false,
            missing,
            removable: whole && !missing,
            marked: false,
        },
    })
}

fn columns() -> String {
    COLUMNS
        .replace("{AVAILABLE}", AVAILABLE)
        .replace("{LOSSLESS}", LOSSLESS)
}

/// A row with its file's device and inode.
type Matched = (Candidate, Option<i64>, Option<i64>);

/// The joins of the measures that compare tracks.
const TRACK_JOINS: &str = "FROM tracks t JOIN files f ON f.id = t.file_id
    JOIN dirs d ON d.id = f.dir_id
    LEFT JOIN copy_groups g ON g.id = f.copy_group
    LEFT JOIN stats s ON s.identity_id = t.identity_id";

/// The rows of `match_by`, ordered by their key.
fn rows(c: &Connection, match_by: MatchBy) -> rusqlite::Result<Vec<Matched>> {
    let columns = columns();
    let sql = match match_by {
        MatchBy::IdenticalFiles => format!(
            "SELECT CAST(f.copy_group AS TEXT), '', 0, {columns}
             FROM files f JOIN dirs d ON d.id = f.dir_id
             JOIN copy_groups g ON g.id = f.copy_group
             LEFT JOIN tracks t ON t.id = (
                 SELECT t2.id FROM tracks t2 JOIN files f2 ON f2.id = t2.file_id
                 WHERE f2.copy_group = f.copy_group
                 ORDER BY t2.file_id = f.id DESC, t2.file_id = g.primary_file DESC,
                          t2.sub_index LIMIT 1)
             LEFT JOIN stats s ON s.identity_id = t.identity_id
             ORDER BY f.copy_group, f.id = g.primary_file DESC, f.added_at,
                      length(d.path) + length(f.name), d.path, f.name"
        ),
        MatchBy::SameFile => format!(
            "SELECT CAST(f.dev AS TEXT), CAST(f.inode AS TEXT), 0, {columns}
             FROM files f JOIN dirs d ON d.id = f.dir_id
             LEFT JOIN copy_groups g ON g.id = f.copy_group
             LEFT JOIN tracks t ON t.id = (SELECT min(t2.id) FROM tracks t2 WHERE t2.file_id = f.id)
             LEFT JOIN stats s ON s.identity_id = t.identity_id
             WHERE f.dev IS NOT NULL AND EXISTS (
                 SELECT 1 FROM files o WHERE o.dev = f.dev AND o.inode = f.inode AND o.id != f.id)
             ORDER BY f.dev, f.inode, f.added_at, d.path, f.name"
        ),
        MatchBy::SameRecording => format!(
            "SELECT t.title_fold, hk_fold(coalesce(t.main_artist, t.artist, '')), 0, {columns}
             {TRACK_JOINS}
             WHERE t.title_fold IS NOT NULL AND t.title_fold != '' AND t.is_copy = 0
             ORDER BY 1, 2, t.duration_ms, f.added_at, d.path, f.name"
        ),
        MatchBy::SameAlbumTrack => format!(
            "SELECT t.album_group, CAST(coalesce(t.disc_no, 1) AS TEXT), t.track_no, {columns}
             {TRACK_JOINS}
             WHERE t.album_group IS NOT NULL AND t.track_no IS NOT NULL AND t.is_copy = 0
             ORDER BY 1, 2, 3, f.added_at, d.path, f.name"
        ),
    };
    let mut statement = c.prepare(&sql)?;
    let rows = statement.query_map([], |row| {
        let found = candidate(row)?;
        Ok((found, row.get(3 + 13)?, row.get(3 + 14)?))
    })?;
    rows.collect()
}

/// Splits sorted rows into groups of equal keys, and for the same recording into
/// runs whose lengths lie within 2 s of the run's first.
fn group_rows(rows: Vec<Matched>, match_by: MatchBy) -> Vec<Vec<Matched>> {
    let mut groups: Vec<Vec<Matched>> = Vec::new();
    for row in rows {
        let joins = groups.last().is_some_and(|g| {
            let first = &g[0].0;
            first.key == row.0.key
                && (match_by != MatchBy::SameRecording
                    || match (first.duration_ms, row.0.duration_ms) {
                        (Some(a), Some(b)) => (b - a).abs() <= 2000,
                        (None, None) => true,
                        _ => false,
                    })
        });
        if joins {
            if let Some(g) = groups.last_mut() {
                g.push(row);
            }
        } else {
            groups.push(vec![row]);
        }
    }
    groups.retain(|g| {
        let mut files: Vec<FileId> = g.iter().map(|(c, ..)| c.entry.file).collect();
        files.sort_unstable();
        files.dedup();
        files.len() > 1
    });
    groups
}

/// The groups of duplicates by `match_by`, with the files `keep` suggests moving.
pub(crate) fn find(
    c: &Connection,
    match_by: MatchBy,
    keep: KeepRule,
) -> rusqlite::Result<Vec<DuplicateGroup>> {
    let grouped = group_rows(rows(c, match_by)?, match_by);
    Ok(grouped
        .into_iter()
        .map(|rows| build(rows, match_by, keep))
        .collect())
}

/// How many groups each measure finds.
pub(crate) fn counts(c: &Connection) -> rusqlite::Result<DuplicateCounts> {
    let count = |m| -> rusqlite::Result<usize> { Ok(group_rows(rows(c, m)?, m).len()) };
    Ok(DuplicateCounts {
        identical_files: count(MatchBy::IdenticalFiles)?,
        same_file: count(MatchBy::SameFile)?,
        same_recording: count(MatchBy::SameRecording)?,
        same_album_track: count(MatchBy::SameAlbumTrack)?,
    })
}

fn build(rows: Vec<Matched>, match_by: MatchBy, keep: KeepRule) -> DuplicateGroup {
    let keys: Vec<(FileId, Option<i64>, Option<i64>)> = rows
        .iter()
        .map(|(c, dev, inode)| (c.entry.file, *dev, *inode))
        .collect();
    let mut candidates: Vec<Candidate> = rows
        .into_iter()
        .map(|(mut c, dev, inode)| {
            c.entry.same_file = dev.is_some()
                && keys
                    .iter()
                    .any(|(f, d, i)| *f != c.entry.file && *d == dev && *i == inode);
            if c.entry.same_file || match_by == MatchBy::SameFile {
                c.entry.removable = false;
            }
            if match_by == MatchBy::IdenticalFiles && c.entry.is_main {
                c.entry.removable = false;
            }
            c
        })
        .collect();
    let present: Vec<&Candidate> = candidates.iter().filter(|c| !c.entry.missing).collect();
    let kept = match (keep, match_by) {
        (KeepRule::ByHand, _) | (_, MatchBy::SameFile) => None,
        (KeepRule::BestQuality, _) => present
            .iter()
            .max_by_key(|c| {
                (
                    c.lossless,
                    c.entry.bit_depth,
                    c.entry.sample_rate,
                    c.entry.bitrate,
                    c.entry.size,
                    std::cmp::Reverse(c.entry.added),
                )
            })
            .map(|c| c.entry.file),
        (KeepRule::Oldest, _) => present
            .iter()
            .min_by_key(|c| (c.entry.added.is_none(), c.entry.added))
            .map(|c| c.entry.file),
        (KeepRule::MostPlayed, _) => present
            .iter()
            .max_by_key(|c| (c.entry.plays, std::cmp::Reverse(c.entry.added)))
            .map(|c| c.entry.file),
    };
    if let Some(kept) = kept {
        // Copies of the kept file's group are its own: never marked with it.
        for c in &mut candidates {
            c.entry.marked = c.entry.removable && c.entry.file != kept;
        }
    }
    let head = kept
        .and_then(|k| candidates.iter().find(|c| c.entry.file == k))
        .or(candidates.first());
    let (title, artist, duration) = head.map_or((None, None, None), |c| {
        (
            c.title.clone(),
            c.artist.clone(),
            c.duration_ms.map(from_millis),
        )
    });
    // One entry per file: a file with several matching tracks shows once.
    let mut entries: Vec<DuplicateEntry> = Vec::with_capacity(candidates.len());
    for c in candidates {
        if !entries.iter().any(|e| e.file == c.entry.file) {
            entries.push(c.entry);
        }
    }
    DuplicateGroup {
        title,
        artist,
        duration,
        entries,
        keep: kept,
    }
}
