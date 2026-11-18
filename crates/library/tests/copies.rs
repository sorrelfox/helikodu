// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Copies: byte-identical files recognised by the scanner, their shared identity and
//! main copy, the settings that skip or hide them, the duplicate finder, and moving
//! them to the trash safely.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[path = "support/scanning.rs"]
#[allow(dead_code)]
mod support;

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use helikodu_library::{
    AlbumQuery, ArtistQuery, ArtistRole, CancelToken, CopyGroup, DatabaseConfig, FileId,
    GenreQuery, KeepRule, Library, Listen, ListenEnd, MatchBy, Page, PlayRules, QueryRequest,
    Rating, ScanOptions, ScanSummary, SearchRequest, TrackFilter, TrackQuery, Trash, TrashRefusal,
    backup_path, parse_query,
};
use helikodu_model::library::TrackId;
use helikodu_model::settings::{DuplicateScan, DuplicateSettings};
use support::{flac, folder, have_ffmpeg, worker_program, write};

fn options(roots: &[&Path]) -> ScanOptions {
    let mut options = ScanOptions::new(roots.iter().copied());
    options.workers.program = Some(worker_program());
    options.workers.count = 2;
    options
}

fn scan(library: &Library, roots: &[&Path]) -> ScanSummary {
    library
        .scan(&options(roots), None, &CancelToken::new())
        .unwrap()
}

fn settings(library: &Library, roots: &[&Path], on_scan: DuplicateScan, show_one: bool) {
    library
        .apply_duplicate_settings(
            DuplicateSettings { on_scan, show_one },
            &options(roots),
            None,
            &CancelToken::new(),
        )
        .unwrap();
}

fn sql_count(library: &Library, sql: &str) -> i64 {
    library
        .database()
        .read(|c| c.query_row(sql, [], |row| row.get(0)))
        .unwrap()
}

fn track(library: &Library, path: &Path) -> TrackId {
    library
        .track_at(path, 0)
        .unwrap()
        .unwrap_or_else(|| panic!("no track at {}", path.display()))
}

fn file_of(group: &CopyGroup, path: &Path) -> FileId {
    group
        .files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{} is not in the group", path.display()))
        .file
}

fn all_tracks(library: &Library) -> usize {
    library
        .tracks(&TrackQuery {
            filter: TrackFilter::All,
            page: Page::first(1000),
            include_masked: false,
            sort: Vec::new(),
        })
        .unwrap()
        .total
}

/// Two albums of two tracks in `a`, and the same files copied into `b`.
fn two_trees(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let base = folder(name);
    let a = base.join("a");
    let b = base.join("b");
    for (n, hz) in [(1, 440), (2, 550)] {
        let title = format!("Song {n}");
        let track = n.to_string();
        flac(
            a.join(format!("Album/{n:02}.flac")),
            hz,
            1,
            &[
                ("title", title.as_str()),
                ("artist", "Band"),
                ("album", "Album"),
                ("track", track.as_str()),
                ("genre", "Rock"),
            ],
        );
    }
    copy_tree(&a, &b);
    (base, a, b)
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::create_dir_all(to).unwrap();
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn memory_library() -> Library {
    Library::open(&DatabaseConfig::in_memory()).unwrap()
}

#[test]
fn copies_in_two_folders_share_one_identity_and_play_the_main_copy() {
    if !have_ffmpeg() {
        return;
    }
    let (_base, a, b) = two_trees("two-folders");
    let library = memory_library();
    let summary = scan(&library, &[&a, &b]);
    assert_eq!(summary.copies, 4, "{summary:?}");
    assert_eq!(sql_count(&library, "SELECT count(*) FROM copy_groups"), 2);
    assert_eq!(all_tracks(&library), 4);

    let main = a.join("Album/01.flac");
    let copy = b.join("Album/01.flac");
    let group = library.copies_of(track(&library, &copy)).unwrap().unwrap();
    assert_eq!(group.main, file_of(&group, &main));
    assert_eq!(group.files.len(), 2);
    assert_eq!(group.files[0].path, main);
    assert!(
        group
            .files
            .iter()
            .all(|f| f.same_file_as.is_none() && !f.missing)
    );
    assert_eq!(group.files[0].format.as_deref(), Some("flac"));
    assert_eq!(group.removable(), vec![file_of(&group, &copy)]);
    assert_eq!(group.files[1].track, Some(track(&library, &copy)));

    // One identity, and playing the copy plays the main copy's file.
    let (main_track, copy_track) = (track(&library, &main), track(&library, &copy));
    assert_eq!(
        library.identity_of(main_track).unwrap(),
        library.identity_of(copy_track).unwrap()
    );
    assert_eq!(
        library.track_source(copy_track).unwrap().unwrap().path,
        main
    );
    // Every track row knows it has one other copy.
    let page = library
        .tracks(&TrackQuery {
            filter: TrackFilter::All,
            page: Page::first(10),
            include_masked: false,
            sort: Vec::new(),
        })
        .unwrap();
    assert!(page.items.iter().all(|t| t.copies == 1), "{page:?}");

    // Statistics recorded on the copy are the main copy's.
    library
        .record_listen(
            copy_track,
            Listen {
                started_at: SystemTime::now(),
                played: Duration::from_secs(1),
                position: None,
                end: ListenEnd::Finished,
                context: None,
            },
            PlayRules::default(),
        )
        .unwrap();
    assert_eq!(library.stats(main_track).unwrap().play_count, 1);

    // A rescan changes nothing.
    let again = scan(&library, &[&a, &b]);
    assert_eq!((again.read, again.copies), (0, 0), "{again:?}");
    assert_eq!(sql_count(&library, "SELECT count(*) FROM copy_groups"), 2);
}

#[test]
fn show_one_hides_the_copies_from_every_query() {
    if !have_ffmpeg() {
        return;
    }
    let (_base, a, b) = two_trees("show-one");
    let library = memory_library();
    scan(&library, &[&a, &b]);
    let albums = || {
        library
            .albums(&AlbumQuery {
                page: Page::first(100),
                ..AlbumQuery::default()
            })
            .unwrap()
    };
    // The copies form an album of their own folder.
    assert_eq!(albums().total, 2);
    assert_eq!(
        library.search(&SearchRequest::new("song")).unwrap().total,
        4
    );

    settings(&library, &[&a, &b], DuplicateScan::Add, true);
    assert_eq!(all_tracks(&library), 2);
    let shown = albums();
    assert_eq!(shown.total, 1, "an album of hidden copies disappears");
    assert_eq!(shown.items[0].track_count, 2);
    let artists = library
        .artists(&ArtistQuery::new(ArtistRole::Artist))
        .unwrap();
    assert_eq!(artists.total, 1);
    assert_eq!(artists.items[0].track_count, 2);
    assert_eq!(artists.items[0].album_count, 1);
    let genres = library.genres(&GenreQuery::default()).unwrap();
    assert_eq!(genres.items[0].track_count, 2);
    assert_eq!(
        library.search(&SearchRequest::new("song")).unwrap().total,
        2
    );
    assert_eq!(library.search(&SearchRequest::new("so")).unwrap().total, 2);
    let tree = parse_query("artist:band").unwrap();
    assert_eq!(
        library
            .query_count(&QueryRequest::new(tree.clone()))
            .unwrap(),
        2
    );
    let totals = library.totals().unwrap();
    assert_eq!((totals.tracks, totals.albums, totals.files), (2, 1, 2));
    assert_eq!((totals.artists, totals.genres), (1, 1));
    // The copies still know of each other.
    let main = a.join("Album/01.flac");
    assert_eq!(
        library
            .copies_of(track(&library, &main))
            .unwrap()
            .unwrap()
            .files
            .len(),
        2
    );

    settings(&library, &[&a, &b], DuplicateScan::Add, false);
    assert_eq!(all_tracks(&library), 4);
    assert_eq!(albums().total, 2);
    assert_eq!(library.query_count(&QueryRequest::new(tree)).unwrap(), 4);
    assert_eq!(library.totals().unwrap().tracks, 4);
}

#[test]
fn skipped_copies_have_no_tracks_and_switching_applies_at_once() {
    if !have_ffmpeg() {
        return;
    }
    let (_base, a, b) = two_trees("skip");
    let library = memory_library();
    settings(&library, &[&a, &b], DuplicateScan::Skip, false);
    let first = scan(&library, &[&a, &b]);
    // The copies are recognised before they are read.
    assert_eq!((first.read, first.copies), (2, 4), "{first:?}");
    assert_eq!(all_tracks(&library), 2);
    assert_eq!(sql_count(&library, "SELECT count(*) FROM files"), 4);
    assert_eq!(
        library.search(&SearchRequest::new("song")).unwrap().total,
        2
    );
    assert_eq!(library.totals().unwrap().tracks, 2);
    let main = a.join("Album/01.flac");
    let copy = b.join("Album/01.flac");
    assert!(library.track_at(&copy, 0).unwrap().is_none());
    let main_track = track(&library, &main);
    let group = library.copies_of(main_track).unwrap().unwrap();
    assert_eq!(group.files.len(), 2);
    assert_eq!(group.files[1].path, copy);
    assert_eq!(group.files[1].track, None);
    let summary = library
        .tracks(&TrackQuery::default())
        .unwrap()
        .items
        .into_iter()
        .find(|t| t.id == main_track)
        .unwrap();
    assert_eq!(summary.copies, 1);

    // Unchanged copies are not read again.
    let again = scan(&library, &[&a, &b]);
    assert_eq!(again.read, 0, "{again:?}");

    // Copies get tracks again at once, sharing the identity.
    settings(&library, &[&a, &b], DuplicateScan::Add, false);
    assert_eq!(all_tracks(&library), 4);
    let copy_track = track(&library, &copy);
    assert_eq!(
        library.identity_of(copy_track).unwrap(),
        library.identity_of(main_track).unwrap()
    );
    // And lose them again.
    settings(&library, &[&a, &b], DuplicateScan::Skip, false);
    assert_eq!(all_tracks(&library), 2);
    assert!(library.track_at(&copy, 0).unwrap().is_none());
    assert_eq!(
        track(&library, &main),
        main_track,
        "the main copy's tracks stay"
    );
}

#[test]
fn the_main_copy_is_chosen_and_taken_over_when_missing() {
    if !have_ffmpeg() {
        return;
    }
    for mode in [DuplicateScan::Add, DuplicateScan::Skip] {
        let (base, a, b) = two_trees(&format!("main-{mode:?}"));
        let library = memory_library();
        settings(&library, &[&a, &b], mode, false);
        scan(&library, &[&a, &b]);
        let main = a.join("Album/01.flac");
        let copy = b.join("Album/01.flac");
        let group = library.copies_of(track(&library, &main)).unwrap().unwrap();
        assert_eq!(group.main, file_of(&group, &main));
        let main_track = track(&library, &main);
        let identity = library.identity_of(main_track).unwrap();

        // The owner picks the other copy.
        library.set_main_copy(file_of(&group, &copy)).unwrap();
        let copy_track = track(&library, &copy);
        let group = library.copies_of(copy_track).unwrap().unwrap();
        assert_eq!(group.main, file_of(&group, &copy));
        assert_eq!(
            library.track_source(copy_track).unwrap().unwrap().path,
            copy
        );
        assert_eq!(library.identity_of(copy_track).unwrap(), identity);
        if mode == DuplicateScan::Skip {
            assert_eq!(
                copy_track, main_track,
                "the track moved to the new main copy"
            );
            assert!(library.track_at(&main, 0).unwrap().is_none());
        } else {
            assert_eq!(
                library.track_source(main_track).unwrap().unwrap().path,
                copy
            );
        }

        // The main copy goes missing: the other takes over.
        let away = base.join("away.flac");
        fs::rename(&copy, &away).unwrap();
        scan(&library, &[&a, &b]);
        let now = track(&library, &main);
        let group = library.copies_of(now).unwrap().unwrap();
        assert_eq!(group.main, file_of(&group, &main));
        assert_eq!(library.identity_of(now).unwrap(), identity);
        assert_eq!(library.track_source(now).unwrap().unwrap().path, main);
        assert_eq!(
            all_tracks(&library),
            if mode == DuplicateScan::Skip { 2 } else { 4 }
        );

        // It comes back and does not take the role back.
        fs::rename(&away, &copy).unwrap();
        scan(&library, &[&a, &b]);
        let group = library.copies_of(track(&library, &main)).unwrap().unwrap();
        assert_eq!(group.main, file_of(&group, &main), "{mode:?}");
        assert_eq!(group.files.len(), 2);
    }
}

#[test]
fn one_file_reached_by_two_paths_is_never_trashed() {
    if !have_ffmpeg() {
        return;
    }
    let base = folder("same-file");
    let a = base.join("a");
    let linked = base.join("linked");
    let mirror = base.join("mirror");
    let main = a.join("x.flac");
    flac(&main, 440, 1, &[("title", "X"), ("artist", "Y")]);
    // A hard link in another library folder, and a library folder that is a
    // symbolic link to the first.
    fs::create_dir_all(&linked).unwrap();
    fs::hard_link(&main, linked.join("x.flac")).unwrap();
    symlink(&a, &mirror).unwrap();
    let library = memory_library();
    scan(&library, &[&a, &linked, &mirror]);
    let group = library.copies_of(track(&library, &main)).unwrap().unwrap();
    assert_eq!(group.files.len(), 3, "{group:?}");
    assert!(group.files.iter().all(|f| f.same_inode()), "{group:?}");
    assert!(group.removable().is_empty());
    // Two paths to one file are not copies.
    assert_eq!(
        library.tracks(&TrackQuery::default()).unwrap().items[0].copies,
        0
    );

    let trash = Trash::with_home_trash(base.join("home/.local/share/Trash"));
    let others: Vec<FileId> = group.files.iter().skip(1).map(|f| f.file).collect();
    let outcomes = library.trash_copies(&others, &trash).unwrap();
    for outcome in &outcomes {
        assert_eq!(outcome.result, Err(TrashRefusal::SameFile), "{outcome:?}");
    }
    let outcomes = library.trash_duplicates(&others, &trash).unwrap();
    for outcome in &outcomes {
        assert_eq!(outcome.result, Err(TrashRefusal::SameFile), "{outcome:?}");
    }
    assert!(main.is_file() && linked.join("x.flac").is_file());
    assert!(mirror.join("x.flac").is_file());

    let found = library
        .find_duplicates(MatchBy::SameFile, KeepRule::Oldest)
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].entries.len(), 3);
    assert!(found[0].entries.iter().all(|e| !e.removable && !e.marked));
    let identical = library
        .find_duplicates(MatchBy::IdenticalFiles, KeepRule::Oldest)
        .unwrap();
    assert!(
        identical[0]
            .entries
            .iter()
            .all(|e| !e.removable && !e.marked)
    );
}

#[test]
fn sections_of_identical_cue_images_are_copies() {
    if !have_ffmpeg() {
        return;
    }
    let base = folder("cue-copies");
    let a = base.join("a");
    let b = base.join("b");
    flac(a.join("album.flac"), 440, 9, &[]);
    write(
        a.join("album.cue"),
        "PERFORMER \"P\"\r\nTITLE \"Live\"\r\nFILE \"album.flac\" WAVE\r\n\
         TRACK 01 AUDIO\r\n  TITLE \"One\"\r\n  INDEX 01 00:00:00\r\n\
         TRACK 02 AUDIO\r\n  TITLE \"Two\"\r\n  INDEX 01 00:03:00\r\n\
         TRACK 03 AUDIO\r\n  TITLE \"Three\"\r\n  INDEX 01 00:06:00\r\n",
    );
    copy_tree(&a, &b);
    let library = memory_library();
    scan(&library, &[&a, &b]);
    assert_eq!(all_tracks(&library), 6);
    for sub in 1..=3 {
        let main = library
            .track_at(&a.join("album.flac"), sub)
            .unwrap()
            .unwrap();
        let copy = library
            .track_at(&b.join("album.flac"), sub)
            .unwrap()
            .unwrap();
        assert_eq!(
            library.identity_of(main).unwrap(),
            library.identity_of(copy).unwrap()
        );
        let group = library.copies_of(copy).unwrap().unwrap();
        assert_eq!(group.files[1].track, Some(copy));
        assert_eq!(group.files[0].track, Some(main));
    }
    assert_eq!(
        sql_count(&library, "SELECT count(DISTINCT identity_id) FROM tracks"),
        3
    );
    settings(&library, &[&a, &b], DuplicateScan::Add, true);
    assert_eq!(all_tracks(&library), 3);
    settings(&library, &[&a, &b], DuplicateScan::Skip, true);
    assert_eq!(all_tracks(&library), 3);
    assert_eq!(sql_count(&library, "SELECT count(*) FROM tracks"), 3);
}

/// Gives the track of `copy` an identity of its own with statistics, as a library
/// from before copies were recognised has it, and forgets the groups.
fn split_as_before(library: &Library, copy: TrackId) -> i64 {
    let id = i64::try_from(copy.get()).unwrap();
    library
        .database()
        .writer()
        .unwrap()
        .write(move |c| {
            let identity: i64 = c.query_row(
                "INSERT INTO identities (created_at) VALUES (1) RETURNING id",
                [],
                |row| row.get(0),
            )?;
            c.execute(
                "UPDATE tracks SET identity_id = ?2, is_copy = 0, copies = 0 WHERE id = ?1",
                [id, identity],
            )?;
            c.execute_batch(
                "UPDATE files SET copy_group = NULL, content_hash = NULL;
                 DELETE FROM copy_groups;
                 UPDATE tracks SET is_copy = 0, copies = 0;",
            )?;
            Ok(identity)
        })
        .unwrap()
}

fn listen(library: &Library, track: TrackId, at: u64) {
    library
        .record_listen(
            track,
            Listen {
                started_at: SystemTime::UNIX_EPOCH + Duration::from_secs(at),
                played: Duration::from_secs(1),
                position: None,
                end: ListenEnd::Finished,
                context: Some("test".into()),
            },
            PlayRules::default(),
        )
        .unwrap();
}

#[test]
fn identities_of_an_existing_library_are_merged_after_a_backup() {
    if !have_ffmpeg() {
        return;
    }
    let base = folder("merge");
    let a = base.join("a");
    let b = base.join("b");
    flac(a.join("x.flac"), 440, 1, &[("title", "X")]);
    copy_tree(&a, &b);
    let db = base.join("library.db");
    let library = Library::open(&DatabaseConfig::file(&db)).unwrap();
    scan(&library, &[&a, &b]);
    let main = track(&library, &a.join("x.flac"));
    let copy = track(&library, &b.join("x.flac"));
    // No data was combined yet, so no backup was needed.
    assert!(!backup_path(&db).exists());

    split_as_before(&library, copy);
    assert_ne!(
        library.identity_of(main).unwrap(),
        library.identity_of(copy).unwrap()
    );
    listen(&library, main, 1_000);
    listen(&library, main, 2_000);
    listen(&library, copy, 3_000);
    library
        .set_rating(copy, Some(Rating::from_stars(4).unwrap()))
        .unwrap();
    library
        .set_resume_position(copy, Some(Duration::from_millis(500)))
        .ok();
    let playlist = library.create_playlist("Mix").unwrap();
    library.add_to_playlist(playlist, vec![copy]).unwrap();

    library
        .confirm_copies(&options(&[&a, &b]), &CancelToken::new())
        .unwrap();
    let identity = library.identity_of(main).unwrap();
    assert_eq!(library.identity_of(copy).unwrap(), identity);
    let stats = library.stats(main).unwrap();
    assert_eq!(stats.play_count, 3);
    assert_eq!(
        stats.first_played,
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000))
    );
    assert_eq!(
        stats.last_played,
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(3_000))
    );
    assert_eq!(
        stats.rating,
        Rating::from_stars(4),
        "the only rating is kept"
    );
    let history = library
        .history(&helikodu_library::HistoryQuery::default())
        .unwrap();
    assert_eq!(history.len(), 3);
    assert!(history.iter().all(|h| h.identity == identity));
    let items = library.playlist_items(playlist, Page::first(10)).unwrap();
    match &items.items[0].content {
        helikodu_library::ItemContent::Track(t) => assert_eq!(t.id, main),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        sql_count(&library, "SELECT count(*) FROM identities"),
        1,
        "the merged identity is gone"
    );

    // The backup was written once, before that merge.
    let backup = backup_path(&db);
    assert!(backup.is_file());
    let written = fs::metadata(&backup).unwrap().modified().unwrap();
    let backed_up = Library::open_read_only(&DatabaseConfig::file(&backup)).unwrap();
    assert_eq!(
        backed_up
            .database()
            .read(|c| c.query_row("SELECT count(*) FROM history", [], |r| r.get::<_, i64>(0)))
            .unwrap(),
        3
    );
    drop(backed_up);
    split_as_before(&library, copy);
    listen(&library, copy, 4_000);
    library
        .confirm_copies(&options(&[&a, &b]), &CancelToken::new())
        .unwrap();
    assert_eq!(library.stats(main).unwrap().play_count, 4);
    assert_eq!(fs::metadata(&backup).unwrap().modified().unwrap(), written);
}

#[test]
fn copies_go_to_the_trash_only_when_safe() {
    if !have_ffmpeg() {
        return;
    }
    let base = folder("trash");
    let a = base.join("a");
    let b = base.join("b");
    let c = base.join("c");
    flac(a.join("x.flac"), 440, 1, &[("title", "X")]);
    copy_tree(&a, &b);
    copy_tree(&a, &c);
    let library = memory_library();
    scan(&library, &[&a, &b, &c]);
    let main = a.join("x.flac");
    let group = library.copies_of(track(&library, &main)).unwrap().unwrap();
    assert_eq!(group.files.len(), 3);
    let (fa, fb, fc) = (
        file_of(&group, &main),
        file_of(&group, &b.join("x.flac")),
        file_of(&group, &c.join("x.flac")),
    );
    // `c` changes after the scan.
    let mut bytes = fs::read(c.join("x.flac")).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::thread::sleep(Duration::from_millis(20));
    fs::write(c.join("x.flac"), &bytes).unwrap();

    let home = base.join("home");
    let trash = Trash::with_home_trash(home.join(".local/share/Trash"));
    let outcomes = library.trash_copies(&[fa, fb, fc], &trash).unwrap();
    assert_eq!(outcomes[0].result, Err(TrashRefusal::MainCopy));
    assert_eq!(outcomes[2].result, Err(TrashRefusal::Changed));
    let trashed = outcomes[1].result.as_ref().unwrap();
    assert!(main.is_file() && c.join("x.flac").is_file());
    assert!(!b.join("x.flac").exists());
    assert_eq!(trashed.path, home.join(".local/share/Trash/files/x.flac"));
    assert!(trashed.path.is_file());
    let info = fs::read_to_string(&trashed.info).unwrap();
    let lines: Vec<&str> = info.lines().collect();
    assert_eq!(lines[0], "[Trash Info]");
    assert_eq!(lines[1], format!("Path={}", b.join("x.flac").display()));
    let date = lines[2].strip_prefix("DeletionDate=").unwrap();
    assert_eq!(date.len(), 19);
    assert_eq!(&date[4..5], "-");
    assert_eq!(&date[10..11], "T");
    // The library forgot the file; the group has the other two.
    let group = library.copies_of(track(&library, &main)).unwrap().unwrap();
    assert_eq!(group.files.len(), 2);
    assert!(library.track_at(&b.join("x.flac"), 0).unwrap().is_none());

    // A second copy of the same name gets a number in the trash.
    copy_tree(&a, &b);
    scan(&library, &[&a, &b, &c]);
    let group = library.copies_of(track(&library, &main)).unwrap().unwrap();
    let outcomes = library
        .trash_copies(&[file_of(&group, &b.join("x.flac"))], &trash)
        .unwrap();
    let trashed = outcomes[0].result.as_ref().unwrap();
    assert_eq!(trashed.path, home.join(".local/share/Trash/files/x.2.flac"));
    assert!(
        home.join(".local/share/Trash/info/x.2.flac.trashinfo")
            .is_file()
    );
}

#[test]
fn files_on_another_volume_go_to_its_trash() {
    if !have_ffmpeg() {
        return;
    }
    let base = folder("volume-trash");
    let volume = base.join("volume");
    let a = volume.join("music/a");
    let b = volume.join("music/b");
    flac(a.join("x y.flac"), 440, 1, &[("title", "X")]);
    copy_tree(&a, &b);
    let library = memory_library();
    scan(&library, &[&a, &b]);
    let home_trash = base.join("home/Trash");
    let trash = Trash::with_home_trash(&home_trash).with_volume(&volume);
    let group = library
        .copies_of(track(&library, &a.join("x y.flac")))
        .unwrap()
        .unwrap();
    let copy = file_of(&group, &b.join("x y.flac"));
    let uid = fs::metadata(&base).unwrap().uid();
    let outcomes = library.trash_copies(&[copy], &trash).unwrap();
    let trashed = outcomes[0].result.as_ref().unwrap();
    let own = volume.join(format!(".Trash-{uid}"));
    assert_eq!(trashed.trash, own);
    assert!(own.join("files/x y.flac").is_file());
    let info = fs::read_to_string(own.join("info/x y.flac.trashinfo")).unwrap();
    assert!(info.contains("\nPath=music/b/x%20y.flac\n"), "{info}");
    assert_eq!(
        fs::metadata(&own).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(!home_trash.exists(), "the home trash was not used");

    // A shared `.Trash` with the sticky bit is used first.
    copy_tree(&a, &b);
    scan(&library, &[&a, &b]);
    fs::create_dir(volume.join(".Trash")).unwrap();
    fs::set_permissions(volume.join(".Trash"), fs::Permissions::from_mode(0o1777)).unwrap();
    let group = library
        .copies_of(track(&library, &a.join("x y.flac")))
        .unwrap()
        .unwrap();
    let outcomes = library
        .trash_copies(&[file_of(&group, &b.join("x y.flac"))], &trash)
        .unwrap();
    let trashed = outcomes[0].result.as_ref().unwrap();
    assert_eq!(trashed.trash, volume.join(format!(".Trash/{uid}")));

    // No usable trash: the file stays. File modes do not bind root (the CI
    // containers run as root), so a read-only volume cannot be made there.
    if uid == 0 {
        return;
    }
    fs::remove_dir_all(volume.join(".Trash")).unwrap();
    fs::remove_dir_all(&own).unwrap();
    copy_tree(&a, &b);
    scan(&library, &[&a, &b]);
    fs::set_permissions(&volume, fs::Permissions::from_mode(0o555)).unwrap();
    let group = library
        .copies_of(track(&library, &a.join("x y.flac")))
        .unwrap()
        .unwrap();
    let outcomes = library
        .trash_copies(&[file_of(&group, &b.join("x y.flac"))], &trash)
        .unwrap();
    fs::set_permissions(&volume, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(outcomes[0].result, Err(TrashRefusal::Trash(_))),
        "{outcomes:?}"
    );
    assert!(b.join("x y.flac").is_file());
    assert_eq!(
        library
            .copies_of(track(&library, &a.join("x y.flac")))
            .unwrap()
            .unwrap()
            .files
            .len(),
        2
    );
}

#[test]
fn the_finder_matches_recordings_and_album_tracks() {
    if !have_ffmpeg() {
        return;
    }
    let base = folder("finder");
    let a = base.join("a");
    let b = base.join("b");
    let tags = [
        ("title", "Tune"),
        ("artist", "Band"),
        ("album", "Record"),
        ("track", "1"),
    ];
    flac(a.join("tune.flac"), 440, 2, &tags);
    // Another rip: other bytes, the same tags, a length within 2 s.
    flac(b.join("tune.flac"), 441, 3, &tags);
    flac(
        b.join("other.flac"),
        441,
        3,
        &[("title", "Other"), ("artist", "Band")],
    );
    let library = memory_library();
    scan(&library, &[&a, &b]);
    assert_eq!(sql_count(&library, "SELECT count(*) FROM copy_groups"), 0);
    let counts = library.duplicate_counts().unwrap();
    assert_eq!(
        (
            counts.identical_files,
            counts.same_file,
            counts.same_recording,
            counts.same_album_track
        ),
        (0, 0, 1, 1)
    );
    for match_by in [MatchBy::SameRecording, MatchBy::SameAlbumTrack] {
        let groups = library
            .find_duplicates(match_by, KeepRule::BestQuality)
            .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].entries.len(), 2);
        assert_eq!(groups[0].title.as_deref(), Some("Tune"));
        assert_eq!(groups[0].entries.iter().filter(|e| e.marked).count(), 1);
        let by_hand = library.find_duplicates(match_by, KeepRule::ByHand).unwrap();
        assert!(by_hand[0].entries.iter().all(|e| !e.marked && e.removable));
    }
    let oldest = library
        .find_duplicates(MatchBy::SameRecording, KeepRule::Oldest)
        .unwrap();
    let marked: Vec<FileId> = oldest[0]
        .entries
        .iter()
        .filter(|e| e.marked)
        .map(|e| e.file)
        .collect();
    assert_eq!(marked.len(), 1);
    let trash = Trash::with_home_trash(base.join("home/Trash"));
    let outcomes = library.trash_duplicates(&marked, &trash).unwrap();
    assert!(outcomes[0].result.is_ok(), "{outcomes:?}");
    assert_eq!(all_tracks(&library), 2);
    assert_eq!(library.duplicate_counts().unwrap().same_recording, 0);
}
