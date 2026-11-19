// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The managed media folder: plans, imports by copying and by moving, sidecar
//! files and the CUE sheet rewrite, taken names and identical files,
//! consolidation that keeps identities and statistics, runs cut short that go on
//! or roll back, and read-only sources. Every test works in its own folder under
//! the target folder.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[path = "support/scanning.rs"]
#[allow(dead_code)]
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use helikodu_library::raw::rusqlite;
use helikodu_library::{
    CancelToken, CharacterSet, DatabaseConfig, ItemAction, ItemKind, ItemResult, Library, Listen,
    ListenEnd, ManagedError, ManagedItem, ManagedOperation, ManagedOptions, ManagedPlan, PlayRules,
    Rating, ScanOptions, SkipReason, Transfer,
};
use helikodu_model::library::TrackId;
use support::{flac, folder, have_ffmpeg, worker_program, write};

const PATTERN: &str = "{albumartist}/{album}/[{disc}-]{track} {title}";

fn scan_options(root: &Path) -> ScanOptions {
    let mut options = ScanOptions::new([root]);
    options.workers.program = Some(worker_program());
    options.workers.count = 2;
    options
}

fn options(root: &Path, transfer: Transfer) -> ManagedOptions {
    ManagedOptions {
        folder: root.join("Library"),
        library_folder: root.to_path_buf(),
        pattern: PATTERN.into(),
        characters: CharacterSet::Windows,
        unknown: "Unknown".into(),
        transfer,
    }
}

/// A downloads folder with an album (two tracks, lyrics for one, a cover and a
/// rip log) and a CUE image (one file, its sheet naming it as a WAV file).
fn downloads(base: &Path) -> PathBuf {
    let dir = base.join("Downloads");
    let album = dir.join("Mari Kask - Tidewater Rooms");
    for (n, title) in [(1, "Tidewater"), (2, "Glass Harbour")] {
        let track = n.to_string();
        flac(
            album.join(format!("0{n} {title}.flac")),
            440 + n * 10,
            1,
            &[
                ("artist", "Mari Kask"),
                ("album", "Tidewater Rooms"),
                ("title", title),
                ("track", &track),
            ],
        );
    }
    write(album.join("01 Tidewater.lrc"), "[00:00.00]Tide");
    write(album.join("cover.jpg"), b"not really a jpeg");
    write(album.join("rip.log"), "rip log");
    write(album.join("notes.txt"), "stays");
    let live = dir.join("Okto Live");
    flac(
        live.join("image.flac"),
        330,
        2,
        &[
            ("artist", "Okto"),
            ("album", "Live"),
            ("title", "Live"),
            ("track", "1"),
        ],
    );
    write(
        live.join("image.cue"),
        "PERFORMER \"Okto\"\r\nTITLE \"Live\"\r\nFILE \"image.wav\" WAVE\r\n  TRACK 01 AUDIO\r\n    TITLE \"One\"\r\n    INDEX 01 00:00:00\r\n  TRACK 02 AUDIO\r\n    TITLE \"Two\"\r\n    INDEX 01 00:01:00\r\n",
    );
    dir
}

/// A library folder (with nothing in it yet) and the library.
fn library_at(name: &str) -> (PathBuf, PathBuf, Library) {
    let base = folder(name);
    let root = base.join("Music");
    fs::create_dir_all(&root).unwrap();
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    (base, root, library)
}

fn plan_import(library: &Library, root: &Path, from: &Path, transfer: Transfer) -> ManagedPlan {
    library
        .plan_managed_import(
            &[from.to_path_buf()],
            &options(root, transfer),
            &scan_options(root),
            &CancelToken::new(),
        )
        .unwrap()
}

fn item<'a>(plan: &'a ManagedPlan, name: &str) -> &'a ManagedItem {
    plan.items
        .iter()
        .find(|i| i.from.file_name().is_some_and(|n| n == name))
        .unwrap_or_else(|| panic!("no item for {name}: {:#?}", plan.items))
}

fn track(library: &Library, path: &Path) -> TrackId {
    library
        .track_at(path, 0)
        .unwrap()
        .unwrap_or_else(|| panic!("no track at {}", path.display()))
}

/// Every file below `dir`, sorted.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn a_plan_lists_each_file_with_its_sidecars_and_the_space() {
    if !have_ffmpeg() {
        return;
    }
    let (base, root, library) = library_at("managed-plan");
    let from = downloads(&base);
    let plan = plan_import(&library, &root, &from, Transfer::Copy);
    assert_eq!(plan.operation, ManagedOperation::Import);
    let album = root.join("Library/Mari Kask/Tidewater Rooms");
    let first = item(&plan, "01 Tidewater.flac");
    assert_eq!(first.to, album.join("01 Tidewater.flac"));
    assert_eq!(
        (first.kind, &first.action),
        (ItemKind::Audio, &ItemAction::Copy)
    );
    assert_eq!(
        item(&plan, "01 Tidewater.lrc").to,
        album.join("01 Tidewater.lrc")
    );
    assert_eq!(item(&plan, "cover.jpg").to, album.join("cover.jpg"));
    assert_eq!(item(&plan, "rip.log").kind, ItemKind::Log);
    assert!(
        plan.items
            .iter()
            .all(|i| i.from.file_name().unwrap() != "notes.txt"),
        "other files stay"
    );
    // The sheet goes beside its file, named like it, and names it.
    let image = item(&plan, "image.flac");
    let sheet = item(&plan, "image.cue");
    assert_eq!(sheet.to, image.to.with_extension("cue"));
    assert!(sheet.rewritten);
    assert_eq!(plan.items.last().unwrap().kind, ItemKind::CueSheet);
    assert_eq!(plan.to_transfer(), plan.items.len());
    assert_eq!(plan.renamed(), 0);
    assert_eq!(plan.total_bytes(), plan.bytes_needed, "a copy needs it all");
    assert!(plan.free_bytes.is_some() && plan.fits());
    assert!(
        !root.join("Library").exists(),
        "a plan changes nothing on the disk"
    );
}

#[test]
fn importing_copies_names_sidecars_and_rewrites_the_cue_sheet() {
    if !have_ffmpeg() {
        return;
    }
    let (base, root, library) = library_at("managed-copy");
    let from = downloads(&base);
    let before = files(&from);
    let plan = plan_import(&library, &root, &from, Transfer::Copy);
    let report = library
        .apply_managed(&plan, &scan_options(&root), &|_, _| {}, &CancelToken::new())
        .unwrap();
    assert!(report.finished, "{report:#?}");
    assert_eq!(report.library_error, None);
    assert_eq!(report.transferred(), plan.items.len());
    assert_eq!(files(&from), before, "a copy leaves the sources");
    let album = root.join("Library/Mari Kask/Tidewater Rooms");
    for name in [
        "01 Tidewater.flac",
        "02 Glass Harbour.flac",
        "01 Tidewater.lrc",
        "cover.jpg",
        "rip.log",
    ] {
        assert!(album.join(name).is_file(), "{name} is there");
    }
    // The library learnt the new files without a scan of its own.
    track(&library, &album.join("01 Tidewater.flac"));
    let image = item(&plan, "image.flac").to.clone();
    let sheet = fs::read_to_string(image.with_extension("cue")).unwrap();
    let name = image.file_name().unwrap().to_string_lossy();
    assert!(
        sheet.contains(&format!("FILE \"{name}\" WAVE\r\n")),
        "{sheet}"
    );
    assert!(
        sheet.contains("TITLE \"Two\"\r\n"),
        "the rest stays: {sheet}"
    );
    assert!(
        library.track_at(&image, 2).unwrap().is_some(),
        "the library reads the sheet's tracks"
    );
    assert!(
        files(&root)
            .iter()
            .all(|p| !p.to_string_lossy().ends_with(".helikodu-part"))
    );
    assert!(library.interrupted_managed_run().unwrap().is_none());
}

#[test]
fn importing_by_moving_removes_the_sources_and_their_emptied_folders() {
    if !have_ffmpeg() {
        return;
    }
    let (base, root, library) = library_at("managed-move");
    let from = downloads(&base);
    let plan = plan_import(&library, &root, &from.join("Okto Live"), Transfer::Move);
    assert!(plan.items.iter().all(|i| i.action == ItemAction::Move));
    let report = library
        .apply_managed(&plan, &scan_options(&root), &|_, _| {}, &CancelToken::new())
        .unwrap();
    assert!(report.finished);
    assert!(
        report
            .outcomes
            .iter()
            .all(|o| o.result == ItemResult::Moved),
        "{report:#?}"
    );
    assert!(!from.join("Okto Live").exists(), "the emptied folder went");
    assert!(from.join("Mari Kask - Tidewater Rooms").exists());
    let image = item(&plan, "image.flac").to.clone();
    assert!(image.is_file() && image.with_extension("cue").is_file());
    assert!(library.track_at(&image, 1).unwrap().is_some());
}

#[test]
fn a_taken_name_gets_a_number_and_an_identical_file_is_skipped() {
    if !have_ffmpeg() {
        return;
    }
    let (base, root, library) = library_at("managed-clash");
    let from = downloads(&base);
    let album = root.join("Library/Mari Kask/Tidewater Rooms");
    let source = from.join("Mari Kask - Tidewater Rooms");
    // Another file under the first track's name, the second track itself.
    write(album.join("01 Tidewater.flac"), b"another recording");
    fs::copy(
        source.join("02 Glass Harbour.flac"),
        album.join("02 Glass Harbour.flac"),
    )
    .unwrap();
    let plan = plan_import(&library, &root, &source, Transfer::Copy);
    let first = item(&plan, "01 Tidewater.flac");
    assert_eq!(first.to, album.join("01 Tidewater (2).flac"));
    assert!(first.renamed && first.action == ItemAction::Copy);
    let second = item(&plan, "02 Glass Harbour.flac");
    assert_eq!(second.action, ItemAction::Skip(SkipReason::Identical));
    assert_eq!(second.to, album.join("02 Glass Harbour.flac"));
    // The lyrics follow the numbered name.
    assert_eq!(
        item(&plan, "01 Tidewater.lrc").to,
        album.join("01 Tidewater (2).lrc")
    );
    assert_eq!(plan.renamed(), 1);
    library
        .apply_managed(&plan, &scan_options(&root), &|_, _| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(
        fs::read(album.join("01 Tidewater.flac")).unwrap(),
        b"another recording",
        "nothing is ever replaced"
    );
    assert!(album.join("01 Tidewater (2).flac").is_file());
}

/// A library folder with two albums outside its managed folder, scanned, with a
/// rating and a play on one track.
fn untidy_library(name: &str) -> (PathBuf, Library, PathBuf, TrackId) {
    let (base, root, library) = library_at(name);
    let from = downloads(&base);
    let incoming = root.join("Incoming");
    fs::rename(&from, &incoming).unwrap();
    library
        .scan(&scan_options(&root), None, &CancelToken::new())
        .unwrap();
    let path = incoming.join("Mari Kask - Tidewater Rooms/01 Tidewater.flac");
    let id = track(&library, &path);
    library.set_rating(id, Rating::from_stars(4)).unwrap();
    library
        .record_listen(
            id,
            Listen {
                started_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
                played: Duration::from_secs(1),
                end: ListenEnd::Finished,
                position: None,
                context: None,
            },
            PlayRules::default(),
        )
        .unwrap();
    (root, library, path, id)
}

#[test]
fn consolidating_keeps_identities_and_statistics() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library, path, id) = untidy_library("managed-consolidate");
    let before = library.stats(id).unwrap();
    assert_eq!(before.play_count, 1);
    let plan = library
        .plan_consolidate(&options(&root, Transfer::Move))
        .unwrap();
    assert_eq!(plan.operation, ManagedOperation::Consolidate);
    let moved = item(&plan, "01 Tidewater.flac");
    assert!(moved.file.is_some());
    let to = root.join("Library/Mari Kask/Tidewater Rooms/01 Tidewater.flac");
    assert_eq!(moved.to, to);
    // A move on one file system needs no space; only the rewritten sheet does.
    let sheet = item(&plan, "image.cue");
    assert!(sheet.rewritten);
    assert_eq!(plan.bytes_needed, sheet.size);
    let report = library
        .apply_managed(&plan, &scan_options(&root), &|_, _| {}, &CancelToken::new())
        .unwrap();
    assert!(report.finished, "{report:#?}");
    assert!(!path.exists() && to.is_file());
    assert!(
        !root.join("Incoming/Okto Live").exists(),
        "emptied folders went"
    );
    assert!(
        root.join("Incoming/Mari Kask - Tidewater Rooms/notes.txt")
            .is_file()
    );
    assert_eq!(track(&library, &to), id, "the track followed its file");
    assert_eq!(library.stats(id).unwrap(), before);
    // The CUE image kept its tracks through the move.
    let image = item(&plan, "image.flac").to.clone();
    assert!(library.track_at(&image, 2).unwrap().is_some());
    // Nothing is outside any more.
    let again = library
        .plan_consolidate(&options(&root, Transfer::Move))
        .unwrap();
    assert_eq!(again.to_transfer(), 0, "{:#?}", again.items);
}

#[test]
fn consolidating_by_copying_lets_the_library_follow_the_copies() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library, path, id) = untidy_library("managed-consolidate-copy");
    let plan = library
        .plan_consolidate(&options(&root, Transfer::Copy))
        .unwrap();
    library
        .apply_managed(&plan, &scan_options(&root), &|_, _| {}, &CancelToken::new())
        .unwrap();
    let to = root.join("Library/Mari Kask/Tidewater Rooms/01 Tidewater.flac");
    assert!(path.is_file() && to.is_file());
    assert_eq!(track(&library, &to), id);
    assert_eq!(library.stats(id).unwrap().rating, Rating::from_stars(4));
}

/// Imports by moving and stops after `after` files, as a crash between files
/// would; then puts the journal and the disk in the state a crash in the middle
/// of the next file leaves: its copy in place but not yet recorded, and a
/// half-written temporary file of the one after.
fn cut_short(name: &str, after: usize) -> (PathBuf, PathBuf, Library, ManagedPlan) {
    let (base, root, library) = library_at(name);
    let from = downloads(&base);
    let plan = plan_import(&library, &root, &from, Transfer::Move);
    let cancel = CancelToken::new();
    let done = AtomicUsize::new(0);
    let report = library
        .apply_managed(
            &plan,
            &scan_options(&root),
            &|n, _| {
                done.store(n, Ordering::SeqCst);
                if n >= after {
                    cancel.cancel();
                }
            },
            &cancel,
        )
        .unwrap();
    assert!(!report.finished);
    assert_eq!(done.load(Ordering::SeqCst), after);
    let waiting = library.interrupted_managed_run().unwrap().unwrap();
    assert_eq!(waiting.done, after);
    assert_eq!(waiting.waiting, plan.items.len() - after);
    // A new run waits until this one is settled.
    assert!(matches!(
        library.apply_managed(&plan, &scan_options(&root), &|_, _| {}, &CancelToken::new()),
        Err(ManagedError::Interrupted(_))
    ));
    // The next file: copied into place, its identity in the journal, but the
    // program died before it said so (and before the source went).
    let next = &plan.items[after];
    fs::create_dir_all(next.to.parent().unwrap()).unwrap();
    fs::copy(&next.from, &next.to).unwrap();
    let meta = fs::metadata(&next.to).unwrap();
    let run = waiting.run.get();
    let position = i64::try_from(after).unwrap();
    {
        use std::os::unix::fs::MetadataExt;
        let (dev, ino) = (meta.dev().cast_signed(), meta.ino().cast_signed());
        library
            .database()
            .writer()
            .unwrap()
            .write(move |c| {
                c.execute(
                    "UPDATE managed_ops SET dev = ?3, ino = ?4 WHERE run_id = ?1 AND position = ?2",
                    rusqlite::params![run, position, dev, ino],
                )
            })
            .unwrap();
    }
    // The one after: half a copy under its temporary name.
    let after_next = &plan.items[after + 1];
    let mut part = std::ffi::OsString::from(".");
    part.push(after_next.to.file_name().unwrap());
    part.push(".helikodu-part");
    write(after_next.to.with_file_name(part), b"half");
    (base.join("Downloads"), root, library, plan)
}

#[test]
fn a_run_cut_short_goes_on_from_the_disk() {
    if !have_ffmpeg() {
        return;
    }
    let (from, root, library, plan) = cut_short("managed-resume", 1);
    let waiting = library.interrupted_managed_run().unwrap().unwrap();
    let report = library
        .resume_managed_run(
            waiting.run,
            &scan_options(&root),
            &|_, _| {},
            &CancelToken::new(),
        )
        .unwrap();
    assert!(report.finished, "{report:#?}");
    assert!(
        report
            .outcomes
            .iter()
            .all(|o| o.result == ItemResult::Moved),
        "{report:#?}"
    );
    for item in &plan.items {
        assert!(item.to.is_file(), "{} is in place", item.to.display());
        assert!(!item.from.exists(), "{} went", item.from.display());
    }
    assert!(
        files(&root)
            .iter()
            .all(|p| !p.to_string_lossy().ends_with(".helikodu-part"))
    );
    // Only the file that is not an album's stays behind.
    assert_eq!(
        files(&from),
        [from.join("Mari Kask - Tidewater Rooms/notes.txt")]
    );
    track(&library, &item(&plan, "01 Tidewater.flac").to);
    assert!(library.interrupted_managed_run().unwrap().is_none());
}

#[test]
fn a_run_cut_short_rolls_back_without_losing_a_file() {
    if !have_ffmpeg() {
        return;
    }
    let (from, root, library, plan) = cut_short("managed-rollback", 2);
    let before: Vec<PathBuf> = plan.items.iter().map(|i| i.from.clone()).collect();
    let waiting = library.interrupted_managed_run().unwrap().unwrap();
    let report = library
        .roll_back_managed_run(waiting.run, &|_, _| {})
        .unwrap();
    assert!(report.finished, "{report:#?}");
    for path in &before {
        assert!(path.is_file(), "{} is back", path.display());
    }
    assert!(
        files(&root.join("Library")).is_empty(),
        "{:#?}",
        files(&root)
    );
    assert!(!root.join("Library/Mari Kask").exists());
    assert!(from.join("Okto Live/image.cue").is_file());
    assert!(library.interrupted_managed_run().unwrap().is_none());
    assert!(
        library
            .track_at(&item(&plan, "01 Tidewater.flac").to, 0)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_read_only_source_is_copied_and_kept() {
    use std::os::unix::fs::PermissionsExt;
    if !have_ffmpeg() || rustix_is_root() {
        return;
    }
    let (base, root, library) = library_at("managed-read-only");
    let from = downloads(&base);
    let live = from.join("Okto Live");
    fs::set_permissions(&live, fs::Permissions::from_mode(0o555)).unwrap();
    let plan = plan_import(&library, &root, &live, Transfer::Move);
    let result =
        library.apply_managed(&plan, &scan_options(&root), &|_, _| {}, &CancelToken::new());
    fs::set_permissions(&live, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        plan.items
            .iter()
            .all(|i| i.action == ItemAction::CopyKeepingSource),
        "{:#?}",
        plan.items
    );
    let report = result.unwrap();
    assert!(
        report
            .outcomes
            .iter()
            .all(|o| o.result == ItemResult::Copied),
        "{report:#?}"
    );
    assert!(live.join("image.flac").is_file());
    assert!(item(&plan, "image.flac").to.is_file());
}

fn rustix_is_root() -> bool {
    // Root may write anywhere, so a read-only folder means nothing to it.
    std::env::var_os("USER").is_some_and(|u| u == "root")
}

#[test]
fn a_consolidation_rolled_back_puts_files_and_library_rows_back() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library, path, id) = untidy_library("managed-consolidate-rollback");
    let plan = library
        .plan_consolidate(&options(&root, Transfer::Move))
        .unwrap();
    let cancel = CancelToken::new();
    let report = library
        .apply_managed(
            &plan,
            &scan_options(&root),
            &|n, _| {
                if n >= 2 {
                    cancel.cancel();
                }
            },
            &cancel,
        )
        .unwrap();
    assert!(!report.finished);
    let to = item(&plan, "01 Tidewater.flac").to.clone();
    assert_eq!(
        track(&library, &to),
        id,
        "the moved file took its row along"
    );
    let waiting = library.interrupted_managed_run().unwrap().unwrap();
    assert_eq!(waiting.operation, ManagedOperation::Consolidate);
    let back = library
        .roll_back_managed_run(waiting.run, &|_, _| {})
        .unwrap();
    assert!(back.finished, "{back:#?}");
    assert!(path.is_file() && !to.exists());
    assert_eq!(track(&library, &path), id, "and brought it back");
    assert_eq!(library.stats(id).unwrap().play_count, 1);
    assert!(files(&root.join("Library")).is_empty());
}
