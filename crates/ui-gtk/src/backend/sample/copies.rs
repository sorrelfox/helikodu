// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Invented copies for the sample library: some tracks have one or two
//! byte-identical copies in other folders, and the groups with two copies also
//! have a path that reaches the main copy's file a second time (a folder
//! mounted twice). None of these files exists; moving one "to the trash" only
//! forgets it, and the second copy of a group always "changed", so the windows
//! can show a file that stays.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use helikodu_model::library::TrackId;

use super::super::services::{
    CopyFile, CopyGroup, DuplicateCounts, DuplicateEntry, DuplicateGroup, FileId, KeepRule,
    MatchBy, TrashProblem, TrashResult,
};
use super::super::{TrackKey, TrackRow};
use super::SampleLibrary;

/// The copies and duplicates the owner changed: files moved to the trash and
/// main copies chosen.
#[derive(Debug, Default)]
pub(super) struct CopyState {
    removed: HashSet<FileId>,
    mains: HashMap<TrackKey, FileId>,
}

/// The kinds of files of a group, by the last digit of their id.
const MAIN: i64 = 0;
const INCOMING: i64 = 1;
const ARCHIVE: i64 = 2;
const LINK: i64 = 3;
/// A re-encoded file the "same recording" measure finds.
const REENCODE: i64 = 5;

/// How many copies the track of `key` has before anything is removed.
pub(super) fn initial_copies(key: TrackKey) -> u32 {
    if key.0 % 23 == 5 {
        2
    } else if key.0 % 9 == 2 {
        1
    } else {
        0
    }
}

fn file_id(key: TrackKey, kind: i64) -> FileId {
    FileId::from_raw(i64::try_from(key.0).unwrap_or(0) * 10 + kind)
}

fn key_and_kind(file: FileId) -> (TrackKey, i64) {
    (
        TrackKey(u64::try_from(file.get() / 10).unwrap_or(0)),
        file.get() % 10,
    )
}

fn home(rest: &str) -> PathBuf {
    gtk::glib::home_dir().join(rest)
}

fn tilde(path: &Path) -> String {
    match path.strip_prefix(gtk::glib::home_dir()) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The size of the file of `row`, from its length and format.
fn size(row: &TrackRow) -> u64 {
    let seconds = row.duration.map_or(240, |d| d.as_secs());
    match row.format.bitrate_kbps {
        Some(kbps) => seconds * u64::from(kbps) * 125,
        None => {
            let bits = u64::from(row.format.bits.unwrap_or(16));
            let rate = u64::from(row.format.sample_rate.unwrap_or(44_100));
            // Lossless at about 60 % of the PCM size.
            seconds * rate * bits * 2 / 8 * 6 / 10
        }
    }
}

fn added(key: TrackKey, kind: i64) -> SystemTime {
    let base = 1_760_000_000 - (key.0 % 400) * 86_400;
    let later = match kind {
        MAIN | LINK => 0,
        INCOMING => 210 * 86_400,
        _ => 40 * 86_400,
    };
    UNIX_EPOCH + Duration::from_secs(base + later)
}

/// The path of the file of `kind` for the track `row`.
fn path(row: &TrackRow, kind: i64) -> PathBuf {
    let main = row.path.clone().unwrap_or_else(|| home("Music/track.flac"));
    let name = main
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match kind {
        INCOMING => home(&format!("Music/Incoming/old rips/{} - {name}", row.artist)),
        ARCHIVE => PathBuf::from(format!("/mnt/archive/Lossless/{}/{name}", row.artist)),
        LINK => home(&format!("Music/Links/{}/{name}", row.artist)),
        REENCODE => home(&format!(
            "Music/Incoming/old rips/{} - {}.mp3",
            row.artist, row.title
        )),
        _ => main,
    }
}

impl SampleLibrary {
    fn copy_state(&self) -> std::sync::MutexGuard<'_, CopyState> {
        self.copies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// How many copies the track of `key` has now.
    pub(super) fn copies_now(&self, key: TrackKey) -> u32 {
        if initial_copies(key) == 0 {
            return 0;
        }
        let state = self.copy_state();
        [MAIN, INCOMING, ARCHIVE]
            .iter()
            .take(initial_copies(key) as usize + 1)
            .filter(|kind| !state.removed.contains(&file_id(key, **kind)))
            .count()
            .saturating_sub(1) as u32
    }

    /// The group of copies of the track of `key`, the main copy first.
    pub(super) fn sample_copies_of(&self, key: TrackKey) -> Option<CopyGroup> {
        let copies = initial_copies(key);
        if copies == 0 {
            return None;
        }
        let row = self.track_row_unpatched(key)?;
        let state = self.copy_state();
        let mut kinds = vec![MAIN, INCOMING];
        if copies == 2 {
            kinds.extend([ARCHIVE, LINK]);
        }
        let present: Vec<FileId> = kinds
            .into_iter()
            .map(|kind| file_id(key, kind))
            .filter(|file| !state.removed.contains(file))
            .collect();
        // A main copy that went takes the next one's place, as the library does.
        let main = state
            .mains
            .get(&key)
            .copied()
            .filter(|file| present.contains(file))
            .or_else(|| present.iter().copied().find(|f| key_and_kind(*f).1 != LINK))?;
        let mut files: Vec<CopyFile> = present
            .into_iter()
            .map(|file| {
                let kind = key_and_kind(file).1;
                let path = path(&row, kind);
                let main_file = file_id(key, MAIN);
                CopyFile {
                    file,
                    display_path: tilde(&path),
                    root: if path.starts_with("/mnt") {
                        PathBuf::from("/mnt/archive/Lossless")
                    } else {
                        home("Music")
                    },
                    path,
                    format: row.format.codec.as_ref().map(|c| c.to_lowercase()),
                    size: size(&row),
                    added: Some(added(key, kind)),
                    is_main: file == main,
                    missing: false,
                    same_file_as: match kind {
                        LINK => Some(main_file),
                        MAIN if copies == 2 => Some(file_id(key, LINK)),
                        _ => None,
                    },
                    track: (kind != LINK).then(|| TrackId::new(key.0)),
                }
            })
            .collect();
        if files.len() < 2 {
            return None;
        }
        files.sort_by_key(|f| !f.is_main);
        Some(CopyGroup { main, files })
    }

    /// Makes `file` the main copy of its group.
    pub(super) fn set_sample_main_copy(&self, file: FileId) -> Result<(), String> {
        let (key, kind) = key_and_kind(file);
        if initial_copies(key) == 0 || kind == LINK {
            return Err("the file has no copies".into());
        }
        self.copy_state().mains.insert(key, file);
        Ok(())
    }

    /// Forgets `files` as if they went to the trash: the main copy, the path
    /// that reaches another file again and the archive copy (which "changed")
    /// stay.
    pub(super) fn sample_trash(&self, files: &[FileId], copies: bool) -> Vec<TrashResult> {
        files
            .iter()
            .map(|&file| {
                let (key, kind) = key_and_kind(file);
                let row = self.track_row_unpatched(key);
                let is_main = self
                    .sample_copies_of(key)
                    .is_some_and(|group| group.main == file);
                let problem = if row.is_none() || self.copy_state().removed.contains(&file) {
                    Some(TrashProblem::Missing)
                } else if is_main && copies {
                    Some(TrashProblem::MainCopy)
                } else if kind == LINK || (kind == MAIN && initial_copies(key) == 2) {
                    Some(TrashProblem::SameFile)
                } else if kind == ARCHIVE {
                    Some(TrashProblem::Changed)
                } else {
                    None
                };
                if problem.is_none() {
                    self.copy_state().removed.insert(file);
                }
                TrashResult {
                    file,
                    path: row.map(|r| path(&r, kind)),
                    problem,
                }
            })
            .collect()
    }

    /// The groups the duplicate finder would find by `by`, marked by `keep`.
    pub(super) fn sample_duplicates(&self, by: MatchBy, keep: KeepRule) -> Vec<DuplicateGroup> {
        let mut groups = Vec::new();
        for index in 0..self.tracks.len() {
            let key = TrackKey(index as u64 + 1);
            let Some(row) = self.track_row_unpatched(key) else {
                continue;
            };
            let mut files: Vec<(FileId, bool)> = match self.sample_copies_of(key) {
                Some(group) => match by {
                    MatchBy::SameFile => group
                        .files
                        .iter()
                        .filter(|f| f.same_file_as.is_some())
                        .map(|f| (f.file, f.is_main))
                        .collect(),
                    _ => group.files.iter().map(|f| (f.file, f.is_main)).collect(),
                },
                None => Vec::new(),
            };
            let reencoded =
                key.0 % 31 == 7 && !self.copy_state().removed.contains(&file_id(key, REENCODE));
            if by == MatchBy::SameRecording && reencoded {
                if files.is_empty() {
                    files.push((file_id(key, MAIN), false));
                }
                files.push((file_id(key, REENCODE), false));
            }
            if files.len() < 2 {
                continue;
            }
            let entries: Vec<DuplicateEntry> = files
                .iter()
                .map(|&(file, is_main)| self.entry(&row, file, is_main, by))
                .collect();
            groups.push(mark(
                DuplicateGroup {
                    title: Some(row.title.clone()),
                    artist: Some(row.artist.clone()),
                    duration: row.duration,
                    entries,
                    keep: None,
                },
                keep,
            ));
        }
        groups
    }

    fn entry(&self, row: &TrackRow, file: FileId, is_main: bool, by: MatchBy) -> DuplicateEntry {
        let (key, kind) = key_and_kind(file);
        let path = path(row, kind);
        let same_file = kind == LINK || (kind == MAIN && initial_copies(key) == 2);
        let (codec, rate, bits, bitrate, size) = if kind == REENCODE {
            (
                Some("MP3".to_owned()),
                Some(44_100),
                None,
                Some(192_000),
                row.duration.map_or(240, |d| d.as_secs()) * 192 * 125,
            )
        } else {
            (
                row.format.codec.clone(),
                row.format.sample_rate,
                row.format.bits.map(u32::from),
                row.format.bitrate_kbps.map(|k| k * 1000),
                size(row),
            )
        };
        DuplicateEntry {
            file,
            track: (kind != LINK).then(|| TrackId::new(key.0)),
            display_path: tilde(&path),
            path,
            codec,
            sample_rate: rate,
            bit_depth: bits,
            bitrate,
            size,
            plays: match kind {
                MAIN => u64::from(row.plays),
                INCOMING => 3,
                _ => 0,
            },
            added: Some(added(key, kind)),
            is_main,
            same_file,
            missing: false,
            removable: by != MatchBy::SameFile
                && !same_file
                && !(is_main && by == MatchBy::IdenticalFiles),
            marked: false,
        }
    }

    /// How many groups each measure finds.
    pub(super) fn sample_duplicate_counts(&self) -> DuplicateCounts {
        let count = |by| self.sample_duplicates(by, KeepRule::ByHand).len();
        DuplicateCounts {
            identical_files: count(MatchBy::IdenticalFiles),
            same_file: count(MatchBy::SameFile),
            same_recording: count(MatchBy::SameRecording),
            same_album_track: count(MatchBy::SameAlbumTrack),
        }
    }
}

/// Marks every removable entry of `group` but the one `keep` keeps.
fn mark(mut group: DuplicateGroup, keep: KeepRule) -> DuplicateGroup {
    let lossless = |e: &DuplicateEntry| e.bitrate.is_none();
    let kept = match keep {
        KeepRule::ByHand => None,
        KeepRule::Oldest => group.entries.iter().min_by_key(|e| e.added).map(|e| e.file),
        KeepRule::MostPlayed => group
            .entries
            .iter()
            .rev()
            .max_by_key(|e| (e.plays, std::cmp::Reverse(e.added)))
            .map(|e| e.file),
        // Of equals, the first (the main copy, or the oldest) is kept.
        _ => group
            .entries
            .iter()
            .rev()
            .max_by_key(|e| (lossless(e), e.bit_depth, e.sample_rate, e.bitrate, e.size))
            .map(|e| e.file),
    };
    // A group of copies always keeps its main copy.
    let kept = kept.map(|kept| {
        group
            .entries
            .iter()
            .find(|e| e.is_main && !e.removable)
            .map_or(kept, |e| e.file)
    });
    for entry in &mut group.entries {
        entry.marked = kept.is_some_and(|kept| kept != entry.file) && entry.removable;
    }
    group.keep = kept;
    group
}
