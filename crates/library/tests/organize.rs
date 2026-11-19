// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The file organizer: the preview, runs, the undo and redo matrix, collisions,
//! partial failures, moves by copying, recovery after a crash, renaming one file,
//! and missing files found and relinked. Every test works in its own folder under
//! the target folder.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[path = "support/scanning.rs"]
#[allow(dead_code)]
mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use helikodu_library::raw::rusqlite;
use helikodu_library::{
    CancelToken, Change, CharacterSet, DatabaseConfig, Destination, FileRunSummary, Library,
    MoveProblem, MoveRequest, OrganizeError, OrganizeOptions, PlannedMove, ScanOptions,
};
use helikodu_model::library::TrackId;
use support::{flac, folder, have_ffmpeg, worker_program, write};

const PATTERN: &str = "{albumartist}/{year} {album}/[{disc}-]{track} {title}";

fn scan(library: &Library, root: &Path) {
    let mut options = ScanOptions::new([root]);
    options.workers.program = Some(worker_program());
    options.workers.count = 2;
    library.scan(&options, None, &CancelToken::new()).unwrap();
}

fn options(pattern: &str) -> OrganizeOptions {
    OrganizeOptions {
        pattern: pattern.into(),
        into: Destination::OwnLibraryFolder,
        characters: CharacterSet::Windows,
        unknown: "Unknown".into(),
    }
}

/// A library folder with two albums in untidy places, a cover beside one, and
/// the library that scanned it.
fn untidy(name: &str) -> (PathBuf, Library) {
    let root = folder(name).join("Music");
    let album = [
        ("artist", "Mari Kask"),
        ("album", "Tidewater Rooms"),
        ("date", "2021"),
    ];
    for (n, title) in [(1, "Tidewater"), (2, "Glass Harbour")] {
        let track = n.to_string();
        let mut tags = album.to_vec();
        tags.push(("title", title));
        tags.push(("track", &track));
        flac(
            root.join(format!(
                "Incoming/Mari Kask - Tidewater Rooms/0{n} {title}.flac"
            )),
            440 + n * 10,
            1,
            &tags,
        );
    }
    write(
        root.join("Incoming/Mari Kask - Tidewater Rooms/cover.jpg"),
        b"not really a jpeg",
    );
    flac(
        root.join("Okto/Night Ferry/02 - night ferry.flac"),
        600,
        1,
        &[
            ("artist", "Okto"),
            ("album", "Night Ferry"),
            ("date", "2020"),
            ("title", "Night Ferry"),
            ("track", "2"),
            ("disc", "1"),
            ("disctotal", "1"),
        ],
    );
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    scan(&library, &root);
    (root, library)
}

fn track(library: &Library, path: &Path) -> TrackId {
    library
        .track_at(path, 0)
        .unwrap()
        .unwrap_or_else(|| panic!("no track at {}", path.display()))
}

fn plan(library: &Library, pattern: &str) -> Vec<PlannedMove> {
    library.plan_organize(None, &options(pattern)).unwrap()
}

fn requests(planned: &[PlannedMove]) -> Vec<MoveRequest> {
    planned
        .iter()
        .filter(|p| p.change.moves())
        .map(|p| MoveRequest {
            file: p.file,
            from: p.from.clone(),
            to: p.to.clone(),
        })
        .collect()
}

fn run(library: &Library, planned: &[PlannedMove]) -> helikodu_library::RunReport {
    library
        .organize(
            &requests(planned),
            Some(PATTERN),
            true,
            &|_, _| {},
            &CancelToken::new(),
        )
        .unwrap()
}

fn last(library: &Library) -> FileRunSummary {
    library.last_file_run().unwrap().expect("a run")
}

/// Every file under `root`, relative, with its bytes' length.
fn tree(root: &Path) -> BTreeMap<PathBuf, u64> {
    let mut out = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    entry.metadata().unwrap().len(),
                );
            }
        }
    }
    out
}

/// The folders under `root`, relative.
fn folders(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                out.push(entry.path().strip_prefix(root).unwrap().to_path_buf());
                pending.push(entry.path());
            }
        }
    }
    out.sort();
    out
}

#[test]
fn the_preview_names_each_file_and_says_what_happens() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library) = untidy("organize-preview");
    let planned = plan(&library, PATTERN);
    let by_from: BTreeMap<PathBuf, &PlannedMove> =
        planned.iter().map(|p| (p.from.clone(), p)).collect();
    let first = by_from[&root.join("Incoming/Mari Kask - Tidewater Rooms/01 Tidewater.flac")];
    assert_eq!(
        first.to,
        root.join("Mari Kask/2021 Tidewater Rooms/01 Tidewater.flac")
    );
    assert_eq!(first.change, Change::Move);
    let ferry = by_from[&root.join("Okto/Night Ferry/02 - night ferry.flac")];
    assert_eq!(
        ferry.to,
        root.join("Okto/2020 Night Ferry/02 Night Ferry.flac"),
        "one disc of one: no disc number"
    );
    assert_eq!(ferry.change, Change::MoveAndRename);
    // Nothing changed on the disk.
    assert!(
        root.join("Incoming/Mari Kask - Tidewater Rooms/cover.jpg")
            .exists()
    );
    assert!(!root.join("Mari Kask").exists());

    // A file already at its place, a name taken on the disk, and two files that
    // would get one name.
    let renamed = plan(&library, "{album}/{filename}");
    let taken_on_disk = root.join("Tidewater Rooms/01 Tidewater.flac");
    write(&taken_on_disk, b"someone else's file");
    let again = plan(&library, "{album}/{filename}");
    let tide = again
        .iter()
        .find(|p| p.to == taken_on_disk)
        .expect("the taken name is planned");
    assert_eq!(tide.change, Change::NameTaken);
    assert_eq!(renamed.len(), 3);
    let same = plan(&library, "{album}");
    let changes: Vec<&Change> = same
        .iter()
        .filter(|p| p.to.starts_with(root.join("Tidewater Rooms.flac")))
        .map(|p| &p.change)
        .collect();
    assert_eq!(changes, [&Change::MoveAndRename, &Change::NameTaken]);
    let in_place = plan(&library, "Okto/Night Ferry/02 - night ferry");
    assert!(
        in_place
            .iter()
            .any(|p| p.change == Change::InPlace && p.from == ferry.from)
    );
    // A folder outside the library is refused.
    let outside = library.plan_organize(
        None,
        &OrganizeOptions {
            into: Destination::Folder(folder("organize-preview-outside")),
            ..options(PATTERN)
        },
    );
    assert!(matches!(outside, Err(OrganizeError::OutsideLibrary(_))));
    // A pattern that cannot be read.
    assert!(matches!(
        library.plan_organize(None, &options("{album}/{nope}")),
        Err(OrganizeError::Pattern(_))
    ));
}

#[test]
fn a_run_undo_and_redo_restore_the_disk_and_the_library_each_time() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library) = untidy("organize-matrix");
    let before = tree(&root);
    let folders_before = folders(&root);
    let old = root.join("Incoming/Mari Kask - Tidewater Rooms/01 Tidewater.flac");
    let new = root.join("Mari Kask/2021 Tidewater Rooms/01 Tidewater.flac");
    let id = track(&library, &old);
    let identity = library.identity_of(id).unwrap();

    let planned = plan(&library, PATTERN);
    let report = run(&library, &planned);
    assert_eq!(report.moved_files(), 3, "{report:?}");
    assert_eq!(report.moved(), 4, "the cover moved along: {report:?}");
    let after = tree(&root);
    assert!(after.contains_key(Path::new("Mari Kask/2021 Tidewater Rooms/cover.jpg")));
    assert!(
        !root.join("Incoming").exists(),
        "the emptied folders are gone"
    );
    assert!(
        !root.join("Okto/Night Ferry").exists(),
        "the emptied album folder is gone"
    );
    assert!(root.join("Okto").exists(), "a folder still in use stays");
    assert_eq!(track(&library, &new), id, "the track keeps its id");
    assert_eq!(library.identity_of(id).unwrap(), identity);
    assert_eq!(library.track_at(&old, 0).unwrap(), None);
    let summary = last(&library);
    assert!(summary.can_undo() && !summary.can_redo());
    assert_eq!(summary.pattern.as_deref(), Some(PATTERN));

    for round in 0..2 {
        let undone = library.undo_file_run(summary.run, &|_, _| {}).unwrap();
        assert_eq!(undone.moved(), 4, "round {round}: {undone:?}");
        assert_eq!(tree(&root), before, "round {round}: the files are back");
        assert_eq!(
            folders(&root),
            folders_before,
            "round {round}: the folders too"
        );
        assert_eq!(track(&library, &old), id);
        assert_eq!(library.track_at(&new, 0).unwrap(), None);
        let state = last(&library);
        assert!(!state.can_undo() && state.can_redo());
        assert!(matches!(
            library.undo_file_run(summary.run, &|_, _| {}),
            Err(OrganizeError::NothingToDo)
        ));

        let redone = library.redo_file_run(summary.run, &|_, _| {}).unwrap();
        assert_eq!(redone.moved(), 4, "round {round}: {redone:?}");
        assert_eq!(tree(&root), after, "round {round}: moved again");
        assert!(!root.join("Incoming").exists());
        assert_eq!(track(&library, &new), id);
        let state = last(&library);
        assert!(state.can_undo() && !state.can_redo());
    }

    // A newer run makes the older one final.
    let back = plan(&library, "{album}/{filename}");
    let newer = run(&library, &back);
    assert!(newer.run.is_some());
    assert!(matches!(
        library.undo_file_run(summary.run, &|_, _| {}),
        Err(OrganizeError::NothingToDo)
    ));
}

#[test]
fn a_destination_taken_after_the_preview_leaves_that_file_alone() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library) = untidy("organize-taken");
    let planned = plan(&library, PATTERN);
    let blocker = root.join("Okto/2020 Night Ferry/02 Night Ferry.flac");
    write(&blocker, b"appeared meanwhile");
    let report = run(&library, &planned);
    let problems: Vec<_> = report.problems().collect();
    assert_eq!(problems.len(), 1, "{report:?}");
    assert_eq!(problems[0].problem, Some(MoveProblem::NameTaken));
    assert_eq!(fs::read(&blocker).unwrap(), b"appeared meanwhile");
    assert!(root.join("Okto/Night Ferry/02 - night ferry.flac").exists());
    assert_eq!(last(&library).failed, 1);
    // The undo moves back only what moved.
    let undone = library
        .undo_file_run(last(&library).run, &|_, _| {})
        .unwrap();
    assert_eq!(undone.moved(), 3);
    assert_eq!(fs::read(&blocker).unwrap(), b"appeared meanwhile");
}

#[test]
fn an_undo_blocked_by_a_new_file_can_be_finished_later() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library) = untidy("organize-undo-blocked");
    let before = tree(&root);
    let planned = plan(&library, PATTERN);
    run(&library, &planned);
    let old = root.join("Okto/Night Ferry/02 - night ferry.flac");
    write(&old, b"a new file where the old one was");
    let summary = last(&library);
    let undone = library.undo_file_run(summary.run, &|_, _| {}).unwrap();
    let problems: Vec<_> = undone.problems().collect();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].problem, Some(MoveProblem::NameTaken));
    assert_eq!(fs::read(&old).unwrap(), b"a new file where the old one was");
    let state = last(&library);
    assert_eq!((state.moved, state.undone), (1, 3));
    fs::remove_file(&old).unwrap();
    let rest = library.undo_file_run(summary.run, &|_, _| {}).unwrap();
    assert_eq!(rest.moved(), 1);
    assert_eq!(tree(&root), before);
    assert!(!last(&library).can_undo());
}

#[test]
fn a_file_moved_away_meanwhile_is_reported_and_the_rest_moves() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library) = untidy("organize-gone");
    let planned = plan(&library, PATTERN);
    let ferry = root.join("Okto/Night Ferry/02 - night ferry.flac");
    fs::rename(&ferry, root.join("elsewhere.flac")).unwrap();
    let report = run(&library, &planned);
    let problems: Vec<_> = report.problems().collect();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].problem, Some(MoveProblem::Gone));
    assert_eq!(report.moved_files(), 2);
}

#[test]
fn a_stopped_run_moves_nothing_more_and_can_be_undone() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library) = untidy("organize-stop");
    let before = tree(&root);
    let planned = plan(&library, PATTERN);
    let cancel = CancelToken::new();
    let stopper = cancel.clone();
    let report = library
        .organize(
            &requests(&planned),
            Some(PATTERN),
            false,
            &move |done, _| {
                if done == 1 {
                    stopper.cancel();
                }
            },
            &cancel,
        )
        .unwrap();
    assert_eq!(report.moved(), 1);
    assert!(
        report
            .problems()
            .all(|p| p.problem == Some(MoveProblem::Stopped))
    );
    library
        .undo_file_run(last(&library).run, &|_, _| {})
        .unwrap();
    assert_eq!(tree(&root), before);
}

#[test]
fn a_move_by_copying_is_undone_like_a_rename() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library) = untidy("organize-copy");
    let before = tree(&root);
    let planned = plan(&library, PATTERN);
    let old = root.join("Incoming/Mari Kask - Tidewater Rooms/02 Glass Harbour.flac");
    let id = track(&library, &old);
    let report = library
        .organize_by_copying(&requests(&planned), Some(PATTERN))
        .unwrap();
    assert_eq!(report.moved_files(), 3, "{report:?}");
    assert_eq!(
        track(
            &library,
            &root.join("Mari Kask/2021 Tidewater Rooms/02 Glass Harbour.flac")
        ),
        id
    );
    assert!(!old.exists());
    // No temporary copy is left anywhere.
    assert!(
        tree(&root)
            .keys()
            .all(|p| !p.to_string_lossy().ends_with(".helikodu-part"))
    );
    library
        .undo_file_run(last(&library).run, &|_, _| {})
        .unwrap();
    let mut without_cover = before.clone();
    without_cover.remove(Path::new("Incoming/Mari Kask - Tidewater Rooms/cover.jpg"));
    let mut now = tree(&root);
    now.remove(Path::new("Incoming/Mari Kask - Tidewater Rooms/cover.jpg"));
    assert_eq!(now, without_cover);
    assert_eq!(track(&library, &old), id);
}

#[test]
fn a_run_cut_short_by_a_crash_is_settled_from_the_disk() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library) = untidy("organize-crash");
    let moved_from = root.join("Okto/Night Ferry/02 - night ferry.flac");
    let moved_to = root.join("Okto/Ferry.flac");
    let stayed_from = root.join("Incoming/Mari Kask - Tidewater Rooms/01 Tidewater.flac");
    let stayed_to = root.join("Tidewater.flac");
    let id = track(&library, &moved_from);
    let file_of = |path: &Path| {
        let p = path.to_path_buf();
        library
            .database()
            .read(move |c| {
                c.query_row(
                    "SELECT f.id FROM files f JOIN dirs d ON d.id = f.dir_id
                     WHERE d.path || '/' || f.name_text = ?1",
                    [p.to_string_lossy().into_owned()],
                    |row| row.get::<_, i64>(0),
                )
            })
            .unwrap()
    };
    let (moved_file, stayed_file) = (file_of(&moved_from), file_of(&stayed_from));
    // The journal as a run leaves it when the program dies after the first
    // file moved on the disk, before the library heard of it.
    let rows = [
        (moved_file, moved_from.clone(), moved_to.clone()),
        (stayed_file, stayed_from.clone(), stayed_to.clone()),
    ];
    library
        .database()
        .writer()
        .unwrap()
        .write(move |c| {
            c.execute(
                "INSERT INTO file_runs (id, started_at, kind, state) VALUES (7, 1, 0, 0)",
                [],
            )?;
            for (i, (file, from, to)) in rows.iter().enumerate() {
                c.execute(
                    "INSERT INTO file_moves (run_id, position, file_id, from_path, to_path)
                     VALUES (7, ?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        i64::try_from(i).unwrap(),
                        file,
                        from.to_string_lossy().as_bytes(),
                        to.to_string_lossy().as_bytes()
                    ],
                )?;
            }
            Ok(())
        })
        .unwrap();
    fs::rename(&moved_from, &moved_to).unwrap();
    // A half-written copy of the second file.
    write(root.join(".Tidewater.flac.helikodu-part"), b"half a copy");

    assert_eq!(library.recover_file_runs().unwrap(), 2);
    assert_eq!(
        track(&library, &moved_to),
        id,
        "the library followed the file"
    );
    assert!(stayed_from.exists());
    assert!(!root.join(".Tidewater.flac.helikodu-part").exists());
    let summary = last(&library);
    assert_eq!((summary.moved, summary.failed), (1, 1));
    library.undo_file_run(summary.run, &|_, _| {}).unwrap();
    assert!(moved_from.exists());
    assert_eq!(track(&library, &moved_from), id);
}

#[test]
fn renaming_one_file_is_a_run_that_can_be_undone() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library) = untidy("organize-rename");
    let old = root.join("Okto/Night Ferry/02 - night ferry.flac");
    let id = track(&library, &old);
    let planned = plan(&library, PATTERN);
    let file = planned.iter().find(|p| p.from == old).unwrap().file;
    let report = library
        .rename_file(file, old.clone(), "Night Ferry.flac".as_ref())
        .unwrap();
    assert_eq!(report.moved(), 1);
    let new = root.join("Okto/Night Ferry/Night Ferry.flac");
    assert_eq!(track(&library, &new), id);
    // Into a taken name: refused.
    write(root.join("Okto/Night Ferry/taken.flac"), b"x");
    let refused = library
        .rename_file(file, new.clone(), "taken.flac".as_ref())
        .unwrap();
    assert_eq!(refused.moved(), 0);
    assert!(refused.run.is_none());
    library
        .undo_file_run(last(&library).run, &|_, _| {})
        .unwrap();
    assert_eq!(track(&library, &old), id);
}

#[test]
fn saved_patterns_start_with_the_drawn_ones_and_can_change() {
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let saved = library.naming_patterns().unwrap();
    assert_eq!(saved.len(), 4);
    assert_eq!(saved[1].name, "With year and disc");
    assert_eq!(saved[1].pattern, PATTERN);
    let id = library
        .save_naming_pattern(None, "Mine", "{artist}/{title}")
        .unwrap();
    library
        .save_naming_pattern(Some(id), "Mine too", "{artist}/{album}")
        .unwrap();
    let saved = library.naming_patterns().unwrap();
    assert_eq!(saved.last().unwrap().name, "Mine too");
    library.delete_naming_pattern(id).unwrap();
    assert_eq!(library.naming_patterns().unwrap().len(), 4);
}

#[test]
fn missing_files_are_found_by_content_relinked_and_removed() {
    if !have_ffmpeg() {
        return;
    }
    let (root, library) = untidy("missing");
    let ferry = root.join("Okto/Night Ferry/02 - night ferry.flac");
    let tide = root.join("Incoming/Mari Kask - Tidewater Rooms/01 Tidewater.flac");
    let ferry_id = track(&library, &ferry);
    let tide_id = track(&library, &tide);
    // One file renamed into another folder of the library, one gone for good.
    let sorted = root.join("Sorted/Okto");
    fs::create_dir_all(&sorted).unwrap();
    fs::rename(&ferry, sorted.join("ferry, renamed.flac")).unwrap();
    fs::remove_file(&tide).unwrap();
    library
        .mark_file_missing(&ferry, SystemTime::now())
        .unwrap();
    library.mark_file_missing(&tide, SystemTime::now()).unwrap();

    let missing = library.missing_files().unwrap();
    assert_eq!(missing.len(), 2);
    assert!(missing.iter().any(|m| m.track == Some(ferry_id)));
    let files: Vec<_> = missing.iter().map(|m| m.file).collect();
    let found = library
        .find_missing(&files, Some(&root.join("Sorted")), &CancelToken::new())
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].at, sorted.join("ferry, renamed.flac"));
    // Every library folder when no folder is named.
    assert_eq!(
        library
            .find_missing(&files, None, &CancelToken::new())
            .unwrap(),
        found
    );

    let outcomes = library.relink_missing(&found).unwrap();
    assert!(outcomes.iter().all(|o| o.problem.is_none()), "{outcomes:?}");
    assert_eq!(
        track(&library, &sorted.join("ferry, renamed.flac")),
        ferry_id
    );
    assert_eq!(library.missing_files().unwrap().len(), 1);
    // Relinking again: it is no longer missing.
    assert!(library.relink_missing(&found).unwrap()[0].problem.is_some());

    let tide_file = library.missing_files().unwrap()[0].file;
    assert_eq!(library.remove_missing(&[tide_file]).unwrap(), 1);
    assert!(library.missing_files().unwrap().is_empty());
    assert!(library.track_at(&tide, 0).unwrap().is_none());
    let _ = tide_id;
}

#[test]
fn expressions_name_evaluate_sort_and_group_tracks() {
    use helikodu_model::expression::Template;
    if !have_ffmpeg() {
        return;
    }
    let (root, library) = untidy("organize-expressions");
    let planned = plan(
        &library,
        "{initial(albumartist)}/{albumartist}/{if(year < 2021, 'Old', 'New')} {upper(album)}/{track} {title}",
    );
    let by_from: BTreeMap<PathBuf, &PlannedMove> =
        planned.iter().map(|p| (p.from.clone(), p)).collect();
    assert_eq!(
        by_from[&root.join("Okto/Night Ferry/02 - night ferry.flac")].to,
        root.join("O/Okto/Old NIGHT FERRY/02 Night Ferry.flac")
    );
    assert_eq!(
        by_from[&root.join("Incoming/Mari Kask - Tidewater Rooms/01 Tidewater.flac")].to,
        root.join("M/Mari Kask/New TIDEWATER ROOMS/01 Tidewater.flac")
    );

    let tide = track(
        &library,
        &root.join("Incoming/Mari Kask - Tidewater Rooms/01 Tidewater.flac"),
    );
    let harbour = track(
        &library,
        &root.join("Incoming/Mari Kask - Tidewater Rooms/02 Glass Harbour.flac"),
    );
    let ferry = track(
        &library,
        &root.join("Okto/Night Ferry/02 - night ferry.flac"),
    );
    let tracks = [tide, harbour, ferry];

    let row =
        Template::parse("{artist} – {title} ({time(duration)}, {plays} plays, {kind})").unwrap();
    let rows = library.evaluate_expression(&row, &tracks).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].0, tide);
    assert_eq!(rows[0].1, "Mari Kask – Tidewater (0:01, 0 plays, music)");
    let added = Template::parse("{year(added)}").unwrap();
    let years = library.evaluate_expression(&added, &[ferry]).unwrap();
    assert_eq!(years[0].1.len(), 4, "{years:?}");

    let by_title = Template::parse("{title}").unwrap();
    assert_eq!(
        library
            .sort_by_expression(&by_title, &tracks, false)
            .unwrap(),
        [harbour, ferry, tide]
    );
    assert_eq!(
        library
            .sort_by_expression(&by_title, &tracks, true)
            .unwrap(),
        [tide, ferry, harbour]
    );
    let by_artist = Template::parse("{albumartist}").unwrap();
    assert_eq!(
        library.group_by_expression(&by_artist, &tracks).unwrap(),
        [
            ("Mari Kask".to_owned(), vec![tide, harbour]),
            ("Okto".to_owned(), vec![ferry])
        ]
    );
    let all = library.expression_values(&[ferry], None).unwrap();
    let values = &all[0].1;
    use helikodu_library::FieldValues;
    use helikodu_model::query::Field;
    assert_eq!(values.value(&Field::Album).as_deref(), Some("Night Ferry"));
    assert!(
        values
            .value(&Field::Path)
            .unwrap()
            .ends_with("02 - night ferry.flac")
    );
    assert_eq!(values.value(&Field::Plays).as_deref(), Some("0"));
}
