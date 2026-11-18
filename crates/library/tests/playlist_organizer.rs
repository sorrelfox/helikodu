// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Folders, smart playlists, undo and redo, the lock, tidying, pins and the playlist
//! files found in the library folders, through the public API.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use helikodu_library::{
    AudioProperties, DatabaseConfig, ExcludeRule, FileStamp, ItemContent, Library, Page, PinTarget,
    PlaylistError, PlaylistId, PlaylistType, RootId, TrackRecord, TrackTags, parse_query,
    print_query,
};
use helikodu_model::library::TrackId;

fn folder(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("library-playlist-organizer")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("music")).unwrap();
    dir
}

fn library(dir: &Path) -> (Library, RootId) {
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let root = library.add_root(dir.join("music")).unwrap();
    (library, root)
}

fn add(library: &Library, root: RootId, path: PathBuf, title: &str, year: i32) -> TrackId {
    let mut record = TrackRecord::new(
        root,
        path,
        FileStamp {
            size: title.len() as u64,
            mtime_ns: Some(1),
            inode: None,
            quick_hash: title.as_bytes().to_vec(),
        },
    );
    record.audio = AudioProperties {
        duration: Some(Duration::from_secs(60)),
        sample_rate: Some(44_100),
        channels: Some(2),
        ..AudioProperties::default()
    };
    record.tags = TrackTags {
        title: Some(title.to_owned()),
        artists: vec!["Band".to_owned()],
        album: Some("Album".to_owned()),
        year: Some(year),
        ..TrackTags::default()
    };
    library.upsert_track(record).unwrap().track
}

fn tracks(library: &Library, playlist: PlaylistId) -> Vec<Option<TrackId>> {
    library
        .playlist_items(playlist, Page::first(1000))
        .unwrap()
        .items
        .into_iter()
        .map(|item| match item.content {
            ItemContent::Track(track) => Some(track.id),
            ItemContent::Dead(_) => None,
        })
        .collect()
}

/// Five tracks, years 2001 to 2005.
fn five(library: &Library, root: RootId, dir: &Path) -> Vec<TrackId> {
    (1..=5)
        .map(|i| {
            add(
                library,
                root,
                dir.join(format!("music/{i}.flac")),
                &format!("Song {i}"),
                2000 + i,
            )
        })
        .collect()
}

#[test]
fn folders_hold_playlists_and_give_them_back() {
    let dir = folder("folders");
    let (library, _) = library(&dir);
    let morning = library.create_playlist("Morning").unwrap();
    let workouts = library.create_playlist_folder("Workouts", None).unwrap();
    let inner = library
        .create_playlist_folder("Running", Some(workouts))
        .unwrap();
    let evening = library.create_playlist("Evening").unwrap();
    library.move_playlist(evening, Some(inner), None).unwrap();
    library
        .move_playlist(morning, Some(workouts), Some(0))
        .unwrap();

    let tree = library.playlist_tree().unwrap();
    let shape: Vec<(String, usize, PlaylistType)> = tree
        .iter()
        .map(|n| (n.summary.name.clone(), n.depth, n.summary.kind))
        .collect();
    assert_eq!(
        shape,
        [
            ("Favourites".into(), 0, PlaylistType::Static),
            ("Workouts".into(), 0, PlaylistType::Folder),
            ("Morning".into(), 1, PlaylistType::Static),
            ("Running".into(), 1, PlaylistType::Folder),
            ("Evening".into(), 2, PlaylistType::Static),
        ]
    );
    assert_eq!(tree[1].summary.items, 2);

    // A folder cannot go into itself or below itself, nor anything into a playlist.
    assert!(matches!(
        library.move_playlist(workouts, Some(inner), None),
        Err(PlaylistError::BadFolder(_))
    ));
    assert!(matches!(
        library.move_playlist(evening, Some(morning), None),
        Err(PlaylistError::BadFolder(_))
    ));

    // Deleting a folder keeps what was in it, one level up.
    library.delete_playlist(workouts).unwrap();
    let tree = library.playlist_tree().unwrap();
    let names: Vec<(&str, usize)> = tree
        .iter()
        .map(|n| (n.summary.name.as_str(), n.depth))
        .collect();
    assert_eq!(
        names,
        [
            ("Favourites", 0),
            ("Morning", 0),
            ("Running", 0),
            ("Evening", 1)
        ]
    );
}

#[test]
fn smart_playlists_follow_the_library() {
    let dir = folder("smart");
    let (library, root) = library(&dir);
    let t = five(&library, root, &dir);
    let query = parse_query("year>=2003 sort:year desc limit:2").unwrap();
    let smart = library
        .create_smart_playlist("Recent", &query, None)
        .unwrap();
    assert_eq!(
        print_query(&library.smart_playlist_query(smart).unwrap()),
        "year>=2003 sort:year desc limit:2"
    );
    let page = library
        .smart_playlist_tracks(smart, Page::first(10))
        .unwrap();
    let ids: Vec<TrackId> = page.items.iter().map(|s| s.id).collect();
    assert_eq!(ids, [t[4], t[3]]);
    // All at once, in one run of the query, the same tracks in the same order.
    assert_eq!(
        library.all_smart_playlist_tracks(smart).unwrap(),
        page.items
    );
    let summary = library.playlist(smart).unwrap().unwrap();
    assert_eq!(summary.kind, PlaylistType::Smart);
    assert_eq!(summary.items, 2);
    assert_eq!(summary.duration, Duration::from_secs(120));

    // A new track that fits comes in at once.
    let new = add(&library, root, dir.join("music/6.flac"), "Song 6", 2010);
    let page = library
        .smart_playlist_tracks(smart, Page::first(10))
        .unwrap();
    assert_eq!(page.items[0].id, new);

    // Its items cannot be edited; its query can, and the change can be undone.
    assert!(matches!(
        library.add_to_playlist(smart, vec![t[0]]),
        Err(PlaylistError::NotStatic(_))
    ));
    library
        .set_smart_playlist_query(smart, &parse_query("year<2002").unwrap())
        .unwrap();
    assert_eq!(library.playlist(smart).unwrap().unwrap().items, 1);
    assert!(library.undo_playlist(smart).unwrap());
    assert_eq!(library.playlist(smart).unwrap().unwrap().items, 2);

    // A copy is a static playlist of the tracks it selects now.
    let copy = library.duplicate_playlist(smart, "Copy").unwrap();
    assert_eq!(tracks(&library, copy), [Some(new), Some(t[4])]);
}

#[test]
fn edits_undo_and_redo_and_the_lock_refuses_them() {
    let dir = folder("undo");
    let (library, root) = library(&dir);
    let t = five(&library, root, &dir);
    let list = library.create_playlist("List").unwrap();
    assert!(!library.undo_playlist(list).unwrap());
    library
        .add_to_playlist(list, vec![t[0], t[1], t[2]])
        .unwrap();
    library.move_playlist_items(list, vec![2], 0).unwrap();
    library.remove_from_playlist(list, vec![1]).unwrap();
    library.rename_playlist(list, "Renamed").unwrap();
    assert_eq!(tracks(&library, list), [Some(t[2]), Some(t[1])]);

    assert!(library.undo_playlist(list).unwrap());
    assert_eq!(library.playlist(list).unwrap().unwrap().name, "List");
    assert!(library.undo_playlist(list).unwrap());
    assert_eq!(tracks(&library, list), [Some(t[2]), Some(t[0]), Some(t[1])]);
    assert!(library.undo_playlist(list).unwrap());
    assert_eq!(tracks(&library, list), [Some(t[0]), Some(t[1]), Some(t[2])]);
    let summary = library.playlist(list).unwrap().unwrap();
    assert!(summary.can_undo && summary.can_redo);
    assert!(library.redo_playlist(list).unwrap());
    assert_eq!(tracks(&library, list), [Some(t[2]), Some(t[0]), Some(t[1])]);

    // A new edit drops what could be redone.
    library.add_to_playlist(list, vec![t[3]]).unwrap();
    assert!(!library.playlist(list).unwrap().unwrap().can_redo);
    assert!(!library.redo_playlist(list).unwrap());

    library.set_playlist_locked(list, true).unwrap();
    assert!(library.playlist(list).unwrap().unwrap().locked);
    for result in [
        library.add_to_playlist(list, vec![t[4]]).map(|_| ()),
        library.remove_from_playlist(list, vec![0]).map(|_| ()),
        library.move_playlist_items(list, vec![0], 2).map(|_| ()),
        library.shuffle_playlist(list, 7),
        library.undo_playlist(list).map(|_| ()),
    ] {
        assert!(
            matches!(result, Err(PlaylistError::Locked(_))),
            "{result:?}"
        );
    }
    library.set_playlist_locked(list, false).unwrap();
    assert!(library.undo_playlist(list).unwrap());
    assert_eq!(tracks(&library, list).len(), 3);
}

#[test]
fn duplicates_and_missing_items_go_and_shuffling_keeps_the_items() {
    let dir = folder("tidy");
    let (library, root) = library(&dir);
    let t = five(&library, root, &dir);
    let list = library.create_playlist("Tidy").unwrap();
    library
        .add_to_playlist(list, vec![t[0], t[1], t[0], t[2], t[1], t[3]])
        .unwrap();
    library
        .mark_file_missing(dir.join("music/4.flac"), SystemTime::now())
        .unwrap();
    library.remove_file(dir.join("music/3.flac")).unwrap();
    let counts = library.playlist_cleanup_counts(list).unwrap();
    assert_eq!((counts.duplicates, counts.missing), (2, 2));

    assert_eq!(library.remove_playlist_duplicates(list).unwrap(), 2);
    assert_eq!(
        tracks(&library, list),
        [Some(t[0]), Some(t[1]), None, Some(t[3])]
    );
    assert_eq!(library.remove_missing_from_playlist(list).unwrap(), 2);
    assert_eq!(tracks(&library, list), [Some(t[0]), Some(t[1])]);
    let counts = library.playlist_cleanup_counts(list).unwrap();
    assert_eq!((counts.duplicates, counts.missing), (0, 0));

    library.add_to_playlist(list, vec![t[4]]).unwrap();
    library.shuffle_playlist(list, 42).unwrap();
    let mut after: Vec<_> = tracks(&library, list).into_iter().flatten().collect();
    after.sort();
    assert_eq!(after, [t[0], t[1], t[4]]);
    library.undo_playlist(list).unwrap();
    assert_eq!(tracks(&library, list), [Some(t[0]), Some(t[1]), Some(t[4])]);
}

#[test]
fn pins_keep_their_order_and_leave_with_what_they_pin() {
    let dir = folder("pins");
    let (library, root) = library(&dir);
    five(&library, root, &dir);
    let list = library.create_playlist("Pinned list").unwrap();
    let album = library
        .albums(&helikodu_library::AlbumQuery::default())
        .unwrap()
        .items[0]
        .id;
    library.pin(PinTarget::Playlist(list)).unwrap();
    library.pin(PinTarget::Album(album)).unwrap();
    library.pin(PinTarget::Playlist(list)).unwrap();
    let pins = library.pins().unwrap();
    let names: Vec<&str> = pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Pinned list", "Album"]);
    assert_eq!(pins[0].playlist, Some(PlaylistType::Static));

    assert!(library.unpin(PinTarget::Album(album)).unwrap());
    assert!(!library.unpin(PinTarget::Album(album)).unwrap());
    library.delete_playlist(list).unwrap();
    assert!(library.pins().unwrap().is_empty());
}

#[test]
fn playlist_files_in_the_library_are_picked_up_and_followed() {
    let dir = folder("found");
    let (library, root) = library(&dir);
    let t = five(&library, root, &dir);
    fs::create_dir_all(dir.join("music/lists")).unwrap();
    fs::create_dir_all(dir.join("music/.hidden")).unwrap();
    fs::create_dir_all(dir.join("music/Skip")).unwrap();
    let file = dir.join("music/lists/Mix.m3u8");
    fs::write(&file, "../1.flac\n../2.flac\n").unwrap();
    fs::write(dir.join("music/.hidden/Hidden.m3u"), "../1.flac\n").unwrap();
    fs::write(dir.join("music/Skip/Skipped.pls"), "[playlist]\n").unwrap();

    let skip = [ExcludeRule::Name("Skip".into())];
    let report = library.import_found_playlists(&skip).unwrap();
    assert_eq!((report.imported, report.refreshed), (1, 0));
    let tree = library.playlist_tree().unwrap();
    // Favourites, then the file.
    assert_eq!(tree.len(), 2);
    assert!(tree[0].summary.builtin);
    let list = tree[1].summary.id;
    assert_eq!(tree[1].summary.name, "Mix");
    assert_eq!(tree[1].summary.source.as_deref(), Some(file.as_path()));
    assert_eq!(tracks(&library, list), [Some(t[0]), Some(t[1])]);

    // Unchanged: nothing happens.
    let report = library.import_found_playlists(&skip).unwrap();
    assert_eq!((report.imported, report.refreshed), (0, 0));

    // Changed: the items follow the file, and that can be undone.
    fs::write(&file, "../3.flac\n").unwrap();
    let later = SystemTime::now() + Duration::from_secs(5);
    fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let report = library.import_found_playlists(&skip).unwrap();
    assert_eq!((report.imported, report.refreshed), (0, 1));
    assert_eq!(tracks(&library, list), [Some(t[2])]);
    assert!(library.undo_playlist(list).unwrap());
    assert_eq!(tracks(&library, list), [Some(t[0]), Some(t[1])]);
}
