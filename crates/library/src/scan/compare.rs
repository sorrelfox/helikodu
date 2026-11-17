// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Comparing a walk with what the library knows: what to read, what is missing.
//!
//! A file is read again when its size, modification time or inode number changed,
//! when it is new, or when a CUE sheet in its folder appeared, changed or went away.
//! A file that could not be read is not read again until it changes, unless the
//! options say so.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::filter::FileClass;
use super::store::{KnownFile, STATE_CUE_SHEET, STATE_READ, STATE_UNREADABLE};
use super::walk::{Found, Walk};
use crate::catalog::RootId;

/// A file to give to a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToRead {
    pub(crate) root: RootId,
    pub(crate) found: Found,
    /// The CUE sheets in the file's folder.
    pub(crate) cue_sheets: Vec<PathBuf>,
}

/// What a scan does, from the comparison of every root.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    pub(crate) to_read: Vec<ToRead>,
    /// CUE sheets that are new or changed, to record.
    pub(crate) cue_sheets: Vec<(RootId, Found)>,
    /// Files with tracks that disappeared.
    pub(crate) missing: Vec<i64>,
    /// Files without tracks that disappeared.
    pub(crate) forget: Vec<i64>,
    /// Files that had been missing and are back.
    pub(crate) back: Vec<i64>,
    pub(crate) found: usize,
    pub(crate) unchanged: usize,
    pub(crate) skipped_unreadable: usize,
}

impl Plan {
    /// Adds the comparison of `walk` of `root` with `known`, the library's files in
    /// that root.
    pub(crate) fn add_root(
        &mut self,
        root: RootId,
        walk: Walk,
        mut known: HashMap<PathBuf, KnownFile>,
        retry_unreadable: bool,
    ) {
        self.found += walk.files.len();
        let mut sheets: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        let mut changed_sheets: HashSet<PathBuf> = HashSet::new();
        let mut media = Vec::new();
        for found in walk.files {
            let entry = known.remove(&found.path);
            if let Some(entry) = &entry
                && entry.missing
            {
                self.back.push(entry.id);
            }
            let same = entry.as_ref().is_some_and(|e| e.stamp == Some(found.stamp));
            if found.class == FileClass::CueSheet {
                let dir = parent(&found.path);
                sheets
                    .entry(dir.clone())
                    .or_default()
                    .push(found.path.clone());
                if same && entry.as_ref().is_some_and(|e| e.state == STATE_CUE_SHEET) {
                    self.unchanged += 1;
                } else {
                    changed_sheets.insert(dir);
                    self.cue_sheets.push((root, found));
                }
            } else {
                media.push((found, entry, same));
            }
        }

        for (path, entry) in known {
            if walk.unreadable.iter().any(|dir| path.starts_with(dir)) {
                continue;
            }
            if entry.has_tracks {
                if !entry.missing {
                    self.missing.push(entry.id);
                }
            } else {
                if entry.state == STATE_CUE_SHEET {
                    changed_sheets.insert(parent(&path));
                }
                self.forget.push(entry.id);
            }
        }

        for (found, entry, same) in media {
            let dir = parent(&found.path);
            let sheet_changed = changed_sheets.contains(&dir);
            let read = match &entry {
                None => true,
                Some(_) if !same || sheet_changed => true,
                Some(e) if e.state == STATE_UNREADABLE => {
                    if !retry_unreadable {
                        self.skipped_unreadable += 1;
                    }
                    retry_unreadable
                }
                // A file recorded as read without tracks: an earlier write failed.
                Some(e) => e.state == STATE_READ && !e.has_tracks,
            };
            if read {
                self.to_read.push(ToRead {
                    root,
                    cue_sheets: sheets.get(&dir).cloned().unwrap_or_default(),
                    found,
                });
            } else if entry.is_some_and(|e| e.state != STATE_UNREADABLE) {
                self.unchanged += 1;
            }
        }
    }

    /// Puts the files to read in path order, which keeps a folder's files together.
    pub(crate) fn sort(&mut self) {
        self.to_read.sort_by(|a, b| a.found.path.cmp(&b.found.path));
    }
}

fn parent(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::walk::Stamp;

    fn stamp(size: u64) -> Stamp {
        Stamp {
            size,
            mtime_ns: 1,
            inode: 2,
        }
    }

    fn found(path: &str, size: u64) -> Found {
        let class = if path.ends_with(".cue") {
            FileClass::CueSheet
        } else {
            FileClass::Audio
        };
        Found {
            path: path.into(),
            stamp: stamp(size),
            class,
        }
    }

    fn known(id: i64, size: u64, state: i64, has_tracks: bool, missing: bool) -> KnownFile {
        KnownFile {
            id,
            stamp: Some(stamp(size)),
            state,
            missing,
            has_tracks,
        }
    }

    fn reads(plan: &Plan) -> Vec<String> {
        plan.to_read
            .iter()
            .map(|r| r.found.path.display().to_string())
            .collect()
    }

    #[test]
    fn new_changed_unchanged_missing_and_back() {
        let walk = Walk {
            files: vec![
                found("/r/new.flac", 1),
                found("/r/same.flac", 1),
                found("/r/changed.flac", 2),
                found("/r/back.flac", 1),
                found("/r/bad.flac", 1),
            ],
            unreadable: vec!["/r/locked".into()],
        };
        let known: HashMap<PathBuf, KnownFile> = [
            ("/r/same.flac", known(1, 1, STATE_READ, true, false)),
            ("/r/changed.flac", known(2, 1, STATE_READ, true, false)),
            ("/r/back.flac", known(3, 1, STATE_READ, true, true)),
            ("/r/bad.flac", known(4, 1, STATE_UNREADABLE, false, false)),
            ("/r/gone.flac", known(5, 1, STATE_READ, true, false)),
            (
                "/r/gone-bad.flac",
                known(6, 1, STATE_UNREADABLE, false, false),
            ),
            ("/r/locked/x.flac", known(7, 1, STATE_READ, true, false)),
            ("/r/still-gone.flac", known(8, 1, STATE_READ, true, true)),
        ]
        .into_iter()
        .map(|(p, k)| (PathBuf::from(p), k))
        .collect();
        let mut plan = Plan::default();
        plan.add_root(RootId(1), walk, known, false);
        plan.sort();
        assert_eq!(reads(&plan), ["/r/changed.flac", "/r/new.flac"]);
        assert_eq!(plan.unchanged, 2);
        assert_eq!(plan.skipped_unreadable, 1);
        assert_eq!(plan.missing, [5]);
        assert_eq!(plan.forget, [6]);
        assert_eq!(plan.back, [3]);
        assert_eq!(plan.found, 5);
    }

    #[test]
    fn unreadable_files_can_be_retried() {
        let walk = Walk {
            files: vec![found("/r/bad.flac", 1)],
            unreadable: Vec::new(),
        };
        let known = [(
            PathBuf::from("/r/bad.flac"),
            known(4, 1, STATE_UNREADABLE, false, false),
        )]
        .into();
        let mut plan = Plan::default();
        plan.add_root(RootId(1), walk, known, true);
        assert_eq!(reads(&plan), ["/r/bad.flac"]);
        assert_eq!(plan.skipped_unreadable, 0);
    }

    #[test]
    fn a_changed_sheet_rereads_its_folder() {
        let walk = Walk {
            files: vec![
                found("/r/a/album.cue", 9),
                found("/r/a/album.flac", 1),
                found("/r/b/other.flac", 1),
                found("/r/c/c.cue", 3),
                found("/r/c/c.flac", 1),
            ],
            unreadable: Vec::new(),
        };
        let known = [
            ("/r/a/album.cue", known(1, 8, STATE_CUE_SHEET, false, false)),
            ("/r/a/album.flac", known(2, 1, STATE_READ, true, false)),
            ("/r/b/other.flac", known(3, 1, STATE_READ, true, false)),
            (
                "/r/b/removed.cue",
                known(4, 1, STATE_CUE_SHEET, false, false),
            ),
            ("/r/c/c.cue", known(5, 3, STATE_CUE_SHEET, false, false)),
            ("/r/c/c.flac", known(6, 1, STATE_READ, true, false)),
        ]
        .into_iter()
        .map(|(p, k)| (PathBuf::from(p), k))
        .collect();
        let mut plan = Plan::default();
        plan.add_root(RootId(1), walk, known, false);
        plan.sort();
        assert_eq!(reads(&plan), ["/r/a/album.flac", "/r/b/other.flac"]);
        assert_eq!(
            plan.to_read[0].cue_sheets,
            [PathBuf::from("/r/a/album.cue")]
        );
        assert!(plan.to_read[1].cue_sheets.is_empty());
        assert_eq!(plan.cue_sheets.len(), 1);
        assert_eq!(plan.forget, [4]);
        assert_eq!(plan.unchanged, 2);
    }
}
