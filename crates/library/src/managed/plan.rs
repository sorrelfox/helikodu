// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Plans of the managed folder: where each file goes, its sidecar files, names
//! that are taken, and the space it all needs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rustix::fs::{Access, access, statvfs};

use super::transfer::{Content, holds, numbered};
use super::{
    ItemAction, ItemKind, ManagedError, ManagedItem, ManagedOperation, ManagedOptions, ManagedPlan,
    SkipReason, Transfer, cue, naming,
};
use crate::catalog::write::file_at;
use crate::copies::FileId;
use crate::db::{Database, path_from_blob};
use crate::organize::NamingPattern;
use crate::organize::plan::{library_targets, root_of};
use crate::scan::filter::{FileClass, classify};
use crate::scan::{CancelToken, ScanOptions, read_reports};

/// The largest CUE sheet that is read and rewritten.
const MAX_SHEET_BYTES: u64 = 1 << 20;
/// How many numbered names are tried for a taken name.
const MAX_NUMBER: u32 = 9999;

/// The kind of a sidecar file, by its extension.
fn sidecar_kind(path: &Path) -> Option<ItemKind> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" => ItemKind::Cover,
        "pdf" => ItemKind::Booklet,
        "cue" => ItemKind::CueSheet,
        "lrc" => ItemKind::Lyrics,
        "log" => ItemKind::Log,
        _ => return None,
    })
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.as_bytes().first() == Some(&b'.'))
}

/// The files of one source folder.
#[derive(Debug, Default)]
struct Folder {
    /// The audio files to take.
    audio: Vec<PathBuf>,
    /// The sidecar files that may go with them.
    sidecars: Vec<PathBuf>,
    /// Whether the whole folder was given (its covers, booklets and logs go
    /// too), not only single files of it.
    whole: bool,
}

/// The folders under `dir`, walked without following links or entering hidden
/// folders.
fn walk(dir: &Path, folders: &mut BTreeMap<PathBuf, Folder>) {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if is_hidden(&path) {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                files.push(path);
            }
        }
        files.sort();
        let folder = folders.entry(dir).or_default();
        folder.whole = true;
        for path in files {
            let name = path.file_name().unwrap_or_default();
            if classify(name) == Some(FileClass::Audio) {
                if !folder.audio.contains(&path) {
                    folder.audio.push(path);
                }
            } else if sidecar_kind(&path).is_some() && !folder.sidecars.contains(&path) {
                folder.sidecars.push(path);
            }
        }
    }
}

/// The audio file `path`, given alone, with the lyrics and CUE sheets beside it.
fn single(path: &Path, folders: &mut BTreeMap<PathBuf, Folder>) {
    let Some(dir) = path.parent() else {
        return;
    };
    let folder = folders.entry(dir.to_path_buf()).or_default();
    if !folder.audio.iter().any(|p| p == path) {
        folder.audio.push(path.to_path_buf());
        folder.audio.sort();
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let side = entry.path();
        if is_hidden(&side) || !entry.file_type().is_ok_and(|k| k.is_file()) {
            continue;
        }
        if matches!(
            sidecar_kind(&side),
            Some(ItemKind::Lyrics | ItemKind::CueSheet)
        ) && !folder.sidecars.contains(&side)
        {
            folder.sidecars.push(side);
        }
    }
    folder.sidecars.sort();
}

/// What the plan knows of the destinations so far.
struct Claims<'a> {
    db: &'a Database,
    claimed: HashSet<PathBuf>,
}

/// Where a file goes in the end.
struct Resolved {
    to: PathBuf,
    identical: bool,
    renamed: bool,
}

impl Claims<'_> {
    /// The first of `want`, `want (2)`, `want (3)`, … that is free (on the disk,
    /// in the library, earlier in the plan) or holds `content` already.
    fn resolve(&mut self, want: &Path, content: Content<'_>) -> Result<Resolved, ManagedError> {
        for n in 1..=MAX_NUMBER {
            let to = if n == 1 {
                want.to_path_buf()
            } else {
                numbered(want, n)
            };
            if self.claimed.contains(&to) {
                continue;
            }
            if fs::symlink_metadata(&to).is_ok() {
                if holds(&to, content).unwrap_or(false) {
                    self.claimed.insert(to.clone());
                    return Ok(Resolved {
                        to,
                        identical: true,
                        renamed: n > 1,
                    });
                }
                continue;
            }
            let path = to.clone();
            if self.db.read(move |c| file_at(c, &path))?.is_some() {
                continue;
            }
            self.claimed.insert(to.clone());
            return Ok(Resolved {
                to,
                identical: false,
                renamed: n > 1,
            });
        }
        Err(ManagedError::NoFreeName(want.to_path_buf()))
    }
}

/// The action for a file at `from` that transfers by `transfer`.
fn action_for(from: &Path, transfer: Transfer) -> ItemAction {
    match transfer {
        Transfer::Copy => ItemAction::Copy,
        Transfer::Move => {
            let writable = from
                .parent()
                .is_some_and(|dir| access(dir, Access::WRITE_OK).is_ok());
            if writable {
                ItemAction::Move
            } else {
                ItemAction::CopyKeepingSource
            }
        }
    }
}

/// The nearest folder at or above `path` that exists.
fn existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|p| p.is_dir())
}

/// The bytes free on the file system of `folder`, and its device.
fn free_space(folder: &Path) -> (Option<u64>, Option<u64>) {
    let Some(dir) = existing_ancestor(folder) else {
        return (None, None);
    };
    let free = statvfs(dir)
        .ok()
        .map(|s| s.f_bavail.saturating_mul(s.f_frsize));
    let dev = fs::metadata(dir).ok().map(|m| m.dev());
    (free, dev)
}

/// The audio items of one source folder that a sidecar can follow: (from, to).
type Placed = Vec<(PathBuf, PathBuf)>;

/// The sidecars of `folder` that go with its audio files `placed`.
fn sidecars(
    folder: &Folder,
    placed: &Placed,
    claims: &mut Claims<'_>,
    transfer: Transfer,
    out: &mut Vec<ManagedItem>,
) -> Result<(), ManagedError> {
    if placed.is_empty() {
        return Ok(());
    }
    // The folder that gets most of the audio files.
    let mut counts: Vec<(PathBuf, usize)> = Vec::new();
    for (_, to) in placed {
        let dir = to.parent().map(Path::to_path_buf).unwrap_or_default();
        match counts.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, n)) => *n += 1,
            None => counts.push((dir, 1)),
        }
    }
    let main = counts
        .iter()
        .fold(None::<&(PathBuf, usize)>, |best, c| match best {
            Some(b) if b.1 >= c.1 => Some(b),
            _ => Some(c),
        })
        .map(|(d, _)| d.clone())
        .unwrap_or_default();
    let froms: Vec<PathBuf> = placed.iter().map(|(from, _)| from.clone()).collect();
    let to_of: HashMap<&Path, &Path> = placed
        .iter()
        .map(|(from, to)| (from.as_path(), to.as_path()))
        .collect();
    let mut cues = Vec::new();
    for side in &folder.sidecars {
        let Some(kind) = sidecar_kind(side) else {
            continue;
        };
        let Some(name) = side.file_name() else {
            continue;
        };
        let want = match kind {
            ItemKind::CueSheet => {
                cues.push(side.clone());
                continue;
            }
            ItemKind::Lyrics => {
                let stem = side.file_stem().unwrap_or_default();
                let audio = froms
                    .iter()
                    .find(|a| a.file_stem() == Some(stem))
                    .or_else(|| {
                        froms.iter().find(|a| {
                            a.file_stem()
                                .is_some_and(|s| s.as_bytes().eq_ignore_ascii_case(stem.as_bytes()))
                        })
                    });
                match audio.and_then(|a| to_of.get(a.as_path())) {
                    Some(to) => to.with_extension(side.extension().unwrap_or_default()),
                    None if folder.whole => main.join(name),
                    None => continue,
                }
            }
            _ if folder.whole => main.join(name),
            _ => continue,
        };
        push_file(side, kind, &want, claims, transfer, out)?;
    }
    for sheet in cues {
        let Ok(m) = fs::symlink_metadata(&sheet) else {
            continue;
        };
        if m.len() > MAX_SHEET_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(&sheet) else {
            continue;
        };
        let mut matched: Vec<&PathBuf> = Vec::new();
        for name in cue::file_names(&bytes) {
            if let Some(audio) = cue::matching(&name, &froms)
                && !matched.contains(&audio)
            {
                matched.push(audio);
            }
        }
        let Some(first) = matched.first().and_then(|a| to_of.get(a.as_path())) else {
            if folder.whole
                && let Some(name) = sheet.file_name()
            {
                push_file(
                    &sheet,
                    ItemKind::CueSheet,
                    &main.join(name),
                    claims,
                    transfer,
                    out,
                )?;
            }
            continue;
        };
        let dir = first.parent().map(Path::to_path_buf).unwrap_or_default();
        let want = if matched.len() == 1 {
            first.with_extension(sheet.extension().unwrap_or_default())
        } else {
            dir.join(sheet.file_name().unwrap_or_default())
        };
        let content = rewritten(&bytes, &want, &froms, &|from| {
            to_of.get(from).map(|to| to.to_path_buf())
        });
        let resolved = claims.resolve(&want, Content::Bytes(&content))?;
        out.push(ManagedItem {
            kind: ItemKind::CueSheet,
            file: None,
            from: sheet.clone(),
            to: resolved.to,
            size: content.len() as u64,
            action: if resolved.identical {
                ItemAction::Skip(SkipReason::Identical)
            } else {
                action_for(&sheet, transfer)
            },
            renamed: resolved.renamed,
            rewritten: content != bytes,
        });
    }
    Ok(())
}

/// The sheet `bytes`, placed at `sheet_to`, with its FILE lines naming the new
/// paths of the audio files `froms` (where `to_of` gives them).
pub(crate) fn rewritten(
    bytes: &[u8],
    sheet_to: &Path,
    froms: &[PathBuf],
    to_of: &dyn Fn(&Path) -> Option<PathBuf>,
) -> Vec<u8> {
    let dir = sheet_to.parent().unwrap_or(Path::new("/"));
    cue::rewrite(bytes, &|name| {
        let audio = cue::matching(name, froms)?;
        let to = to_of(audio)?;
        Some(cue::bytes(&cue::relative(dir, &to)))
    })
}

fn push_file(
    from: &Path,
    kind: ItemKind,
    want: &Path,
    claims: &mut Claims<'_>,
    transfer: Transfer,
    out: &mut Vec<ManagedItem>,
) -> Result<(), ManagedError> {
    let Ok(m) = fs::symlink_metadata(from) else {
        return Ok(());
    };
    let resolved = claims.resolve(want, Content::File(from))?;
    out.push(ManagedItem {
        kind,
        file: None,
        from: from.to_path_buf(),
        to: resolved.to,
        size: m.len(),
        action: if resolved.identical {
            ItemAction::Skip(SkipReason::Identical)
        } else {
            action_for(from, transfer)
        },
        renamed: resolved.renamed,
        rewritten: false,
    });
    Ok(())
}

/// Finishes a plan: the space needed and free.
fn finish(
    operation: ManagedOperation,
    options: &ManagedOptions,
    items: Vec<ManagedItem>,
    bases: Vec<PathBuf>,
) -> ManagedPlan {
    let (free_bytes, dev) = free_space(&options.folder);
    let bytes_needed = items
        .iter()
        .filter(|i| match i.action {
            ItemAction::Copy | ItemAction::CopyKeepingSource => true,
            ItemAction::Move => {
                i.rewritten || dev.is_none() || fs::metadata(&i.from).ok().map(|m| m.dev()) != dev
            }
            ItemAction::Skip(_) => false,
        })
        .map(|i| i.size)
        .sum();
    ManagedPlan {
        operation,
        options: options.clone(),
        items,
        bytes_needed,
        free_bytes,
        bases,
    }
}

fn check_folder(options: &ManagedOptions) -> Result<(), ManagedError> {
    if !options.folder.starts_with(&options.library_folder) {
        return Err(ManagedError::OutsideLibrary {
            folder: options.folder.clone(),
            library_folder: options.library_folder.clone(),
        });
    }
    Ok(())
}

/// Every library folder, masked or not.
fn all_roots(db: &Database) -> Result<Vec<(i64, PathBuf)>, ManagedError> {
    Ok(db.read(|c| {
        c.prepare_cached("SELECT id, path FROM roots")?
            .query_map([], |row| Ok((row.get(0)?, path_from_blob(row.get(1)?))))?
            .collect()
    })?)
}

/// How many present library files of connected library folders are outside
/// `folder`.
pub(super) fn outside(db: &Database, folder: &Path) -> Result<usize, crate::db::DbError> {
    let folder = folder.to_path_buf();
    db.read(move |c| {
        let mut statement = c.prepare_cached(
            "SELECT d.path, count(*) FROM files f
                 JOIN dirs d ON d.id = f.dir_id
                 JOIN roots r ON r.id = d.root_id
             WHERE r.masked = 0 AND f.missing_since IS NULL
             GROUP BY d.id",
        )?;
        let mut count = 0_usize;
        for row in statement.query_map([], |row| {
            Ok((path_from_blob(row.get(0)?), row.get::<_, i64>(1)?))
        })? {
            let (dir, files) = row?;
            if !dir.starts_with(&folder) {
                count += usize::try_from(files).unwrap_or(0);
            }
        }
        Ok(count)
    })
}

pub(super) fn import(
    db: &Database,
    paths: &[PathBuf],
    options: &ManagedOptions,
    scan: &ScanOptions,
    cancel: &CancelToken,
) -> Result<ManagedPlan, ManagedError> {
    check_folder(options)?;
    let pattern = NamingPattern::parse(&options.pattern)?;
    let roots = all_roots(db)?;
    let mut folders: BTreeMap<PathBuf, Folder> = BTreeMap::new();
    let mut bases = Vec::new();
    for path in paths {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
        let Ok(m) = fs::symlink_metadata(&path) else {
            continue;
        };
        if m.is_dir() {
            walk(&path, &mut folders);
            bases.push(path);
        } else if m.is_file()
            && classify(path.file_name().unwrap_or_default()) == Some(FileClass::Audio)
        {
            single(&path, &mut folders);
            bases.push(path);
        }
    }
    let mut items = Vec::new();
    // What stays out: files in the managed folder or the library.
    let mut to_read: Vec<PathBuf> = Vec::new();
    for folder in folders.values_mut() {
        let mut kept = Vec::new();
        for audio in folder.audio.drain(..) {
            let reason = if audio.starts_with(&options.folder) {
                Some(SkipReason::InManagedFolder)
            } else if root_of(&roots, &audio).is_some() {
                Some(SkipReason::InLibrary)
            } else {
                None
            };
            match reason {
                Some(reason) => items.push(skipped(&audio, reason)),
                None => {
                    to_read.push(audio.clone());
                    kept.push(audio);
                }
            }
        }
        folder.audio = kept;
    }
    let reports = read_reports(scan, &to_read, cancel)?;
    let reports: HashMap<&Path, &Result<Box<helikodu_meta::worker::FileReport>, String>> = to_read
        .iter()
        .map(PathBuf::as_path)
        .zip(reports.iter())
        .collect();
    // The number of discs of each album, from all of its files.
    let mut discs: HashMap<(String, String), u32> = HashMap::new();
    let album_key = |tags: &helikodu_meta::tags::TrackTags| {
        (
            tags.album_artist
                .clone()
                .or_else(|| tags.artist.clone())
                .unwrap_or_default(),
            tags.album.clone().unwrap_or_default(),
        )
    };
    for report in reports.values().copied().flatten() {
        let tags = &report.tags.tags;
        let most = [tags.disc_number, tags.disc_total]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(1);
        let entry = discs.entry(album_key(tags)).or_insert(1);
        *entry = (*entry).max(most);
    }
    let mut claims = Claims {
        db,
        claimed: HashSet::new(),
    };
    let mut audio_items = Vec::new();
    let mut side_items = Vec::new();
    for folder in folders.values() {
        let mut placed: Placed = Vec::new();
        for audio in &folder.audio {
            let report = match reports.get(audio.as_path()) {
                Some(Ok(report)) => report,
                Some(Err(reason)) => {
                    let reason = if reason.is_empty() {
                        "not read".to_owned()
                    } else {
                        reason.clone()
                    };
                    audio_items.push(skipped(audio, SkipReason::Unreadable(reason)));
                    continue;
                }
                None => continue,
            };
            let count = discs.get(&album_key(&report.tags.tags)).copied();
            let values = naming::values(&report.tags, audio, count);
            let want = destination(&pattern, options, &values, audio);
            let resolved = claims.resolve(&want, Content::File(audio))?;
            placed.push((audio.clone(), resolved.to.clone()));
            audio_items.push(ManagedItem {
                kind: ItemKind::Audio,
                file: None,
                from: audio.clone(),
                to: resolved.to,
                size: fs::symlink_metadata(audio).map_or(0, |m| m.len()),
                action: if resolved.identical {
                    ItemAction::Skip(SkipReason::Identical)
                } else {
                    action_for(audio, options.transfer)
                },
                renamed: resolved.renamed,
                rewritten: false,
            });
        }
        sidecars(
            folder,
            &placed,
            &mut claims,
            options.transfer,
            &mut side_items,
        )?;
    }
    items.extend(audio_items);
    items.extend(order_sidecars(side_items));
    Ok(finish(ManagedOperation::Import, options, items, bases))
}

/// Sidecars with the CUE sheets last, so that a run writes the sheets once the
/// audio files they name are in place.
fn order_sidecars(mut items: Vec<ManagedItem>) -> Vec<ManagedItem> {
    items.sort_by_key(|i| i.kind == ItemKind::CueSheet);
    items
}

fn skipped(from: &Path, reason: SkipReason) -> ManagedItem {
    ManagedItem {
        kind: ItemKind::Audio,
        file: None,
        from: from.to_path_buf(),
        to: PathBuf::new(),
        size: fs::symlink_metadata(from).map_or(0, |m| m.len()),
        action: ItemAction::Skip(reason),
        renamed: false,
        rewritten: false,
    }
}

/// Where `pattern` puts the file at `from` with the values `values`.
fn destination(
    pattern: &NamingPattern,
    options: &ManagedOptions,
    values: &crate::organize::FieldMap,
    from: &Path,
) -> PathBuf {
    let ext = from
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let names = pattern.render(values, options.characters, &options.unknown, &ext);
    let mut to = options.folder.clone();
    let count = names.len();
    for (i, name) in names.into_iter().enumerate() {
        if i + 1 == count {
            to.push(format!("{name}{ext}"));
        } else {
            to.push(name);
        }
    }
    to
}

pub(super) fn consolidate(
    db: &Database,
    options: &ManagedOptions,
) -> Result<ManagedPlan, ManagedError> {
    check_folder(options)?;
    let pattern = NamingPattern::parse(&options.pattern)?;
    let (folder, characters, unknown) = (
        options.folder.clone(),
        options.characters,
        options.unknown.clone(),
    );
    let targets =
        db.read(|c| library_targets(c, &pattern, &folder, &folder, characters, &unknown))?;
    let mut claims = Claims {
        db,
        claimed: HashSet::new(),
    };
    let mut items = Vec::new();
    let mut side_items = Vec::new();
    let mut folders: BTreeMap<PathBuf, (Folder, Placed)> = BTreeMap::new();
    for target in targets {
        if target.missing {
            let mut item = skipped(&target.path, SkipReason::Missing);
            item.file = Some(FileId(target.file));
            items.push(item);
            continue;
        }
        let resolved = claims.resolve(&target.to, Content::File(&target.path))?;
        if let Some(dir) = target.path.parent() {
            let entry = folders.entry(dir.to_path_buf()).or_default();
            entry.0.audio.push(target.path.clone());
            entry.1.push((target.path.clone(), resolved.to.clone()));
        }
        items.push(ManagedItem {
            kind: ItemKind::Audio,
            file: Some(FileId(target.file)),
            size: fs::symlink_metadata(&target.path).map_or(0, |m| m.len()),
            action: if resolved.identical {
                ItemAction::Skip(SkipReason::Identical)
            } else {
                action_for(&target.path, options.transfer)
            },
            from: target.path,
            to: resolved.to,
            renamed: resolved.renamed,
            rewritten: false,
        });
    }
    for (dir, (mut folder, placed)) in folders {
        folder.whole = true;
        if let Ok(entries) = fs::read_dir(&dir) {
            let mut found: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|k| k.is_file()))
                .map(|e| e.path())
                .filter(|p| !is_hidden(p) && sidecar_kind(p).is_some())
                .collect();
            found.sort();
            folder.sidecars = found;
        }
        sidecars(
            &folder,
            &placed,
            &mut claims,
            options.transfer,
            &mut side_items,
        )?;
    }
    items.extend(order_sidecars(side_items));
    Ok(finish(
        ManagedOperation::Consolidate,
        options,
        items,
        Vec::new(),
    ))
}
