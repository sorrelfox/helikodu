// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Static playlists through the public API: editing, identities across moves, and
//! importing and exporting playlist files with a library in memory and the files in a
//! temporary folder.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use helikodu_library::{
    AudioProperties, DatabaseConfig, DeadReference, ExportOptions, FileStamp, ImportOptions,
    ItemContent, Library, Page, PlaylistError, PlaylistId, PlaylistItem, RootId, TrackChange,
    TrackRecord, TrackTags,
};
use helikodu_meta::playlists::{PathStyle, PlaylistKind, PlaylistWarningKind, WriteWarningKind};
use helikodu_model::library::TrackId;

/// A fresh, empty folder named `name` for one test.
fn folder(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("library-playlists")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// An empty library in memory whose one library folder is `dir/music`.
fn library(dir: &Path) -> (Library, RootId) {
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let root = library.add_root(dir.join("music")).unwrap();
    (library, root)
}

/// A track record at `path`, identified by `content`, with a title, an artist and a
/// length in seconds.
fn record(
    root: RootId,
    path: impl Into<PathBuf>,
    content: &str,
    tags: (&str, &str, u64),
) -> TrackRecord {
    let mut record = TrackRecord::new(
        root,
        path,
        FileStamp {
            size: content.len() as u64,
            mtime_ns: Some(1),
            inode: None,
            quick_hash: content.as_bytes().to_vec(),
        },
    );
    record.audio = AudioProperties {
        duration: Some(Duration::from_secs(tags.2)),
        sample_rate: Some(44_100),
        channels: Some(2),
        ..AudioProperties::default()
    };
    record.tags = TrackTags {
        title: Some(tags.0.to_owned()),
        artists: vec![tags.1.to_owned()],
        album: Some("Album".to_owned()),
        ..TrackTags::default()
    };
    record
}

fn add(library: &Library, record: TrackRecord) -> TrackId {
    library.upsert_track(record).unwrap().track
}

/// The track of each item, or `None` for a dead one.
fn tracks(library: &Library, playlist: PlaylistId) -> Vec<Option<TrackId>> {
    items(library, playlist)
        .into_iter()
        .map(|item| match item.content {
            ItemContent::Track(track) => Some(track.id),
            ItemContent::Dead(_) => None,
        })
        .collect()
}

fn items(library: &Library, playlist: PlaylistId) -> Vec<PlaylistItem> {
    let page = library.playlist_items(playlist, Page::first(1000)).unwrap();
    for (position, item) in page.items.iter().enumerate() {
        assert_eq!(item.position, position);
    }
    page.items
}

/// `text` in cp1251: ASCII and the Cyrillic letters А–я.
fn cp1251(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| match u32::from(c) {
            code @ 0..=0x7f => code as u8,
            code @ 0x410..=0x44f => (code - 0x410 + 0xc0) as u8,
            _ => panic!("{c} is not in the test's cp1251 table"),
        })
        .collect()
}

fn bytes_path(dir: &Path, name: &[u8]) -> PathBuf {
    dir.join(OsStr::from_bytes(name))
}

#[test]
fn create_rename_duplicate_delete() {
    let dir = folder("crud");
    let (library, root) = library(&dir);
    let a = add(
        &library,
        record(root, dir.join("music/a.flac"), "a", ("A", "X", 100)),
    );
    let first = library.create_playlist("  Morning ").unwrap();
    let second = library.create_playlist("Evening").unwrap();
    assert!(matches!(
        library.create_playlist(" \t"),
        Err(PlaylistError::EmptyName)
    ));
    library.add_to_playlist(first, vec![a, a]).unwrap();
    library.rename_playlist(first, "Dawn").unwrap();
    let copy = library.duplicate_playlist(first, "Dawn (copy)").unwrap();
    assert_eq!(tracks(&library, copy), [Some(a), Some(a)]);

    let listed = library.playlists(Page::default()).unwrap();
    // The built-in Favourites comes first.
    assert_eq!(listed.total, 4);
    let names: Vec<_> = listed.items.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Favourites", "Dawn", "Evening", "Dawn (copy)"]);
    assert_eq!(listed.items[1].items, 2);
    assert_eq!(listed.items[1].duration, Duration::from_secs(200));
    assert!(listed.items[1].modified >= listed.items[1].created);
    assert_eq!(
        library.playlists(Page::new(2, 1)).unwrap().items[0].id,
        second
    );

    library.delete_playlist(first).unwrap();
    assert!(library.playlist(first).unwrap().is_none());
    assert_eq!(tracks(&library, copy), [Some(a), Some(a)]);
    assert!(matches!(
        library.delete_playlist(first),
        Err(PlaylistError::UnknownPlaylist(id)) if id == first
    ));
    assert!(matches!(
        library.rename_playlist(first, "x"),
        Err(PlaylistError::UnknownPlaylist(_))
    ));
    assert!(matches!(
        library.playlist_items(first, Page::default()),
        Err(PlaylistError::UnknownPlaylist(_))
    ));
}

#[test]
fn add_insert_move_remove() {
    let dir = folder("edit");
    let (library, root) = library(&dir);
    let t: Vec<TrackId> = (0..6)
        .map(|i| {
            add(
                &library,
                record(
                    root,
                    dir.join(format!("music/{i}.flac")),
                    &format!("c{i}"),
                    ("T", "X", 60),
                ),
            )
        })
        .collect();
    let list = library.create_playlist("Edit").unwrap();
    assert_eq!(
        library
            .add_to_playlist(list, vec![t[0], t[1], t[2]])
            .unwrap(),
        0
    );
    assert_eq!(library.add_to_playlist(list, vec![t[3]]).unwrap(), 3);
    library
        .insert_into_playlist(list, 1, vec![t[4], t[5]])
        .unwrap();
    let ids = |v: &[usize]| v.iter().map(|&i| Some(t[i])).collect::<Vec<_>>();
    assert_eq!(tracks(&library, list), ids(&[0, 4, 5, 1, 2, 3]));
    let before: Vec<_> = items(&library, list).iter().map(|i| i.id).collect();

    // A selection dragged down, before the item at position 5.
    assert_eq!(library.move_playlist_items(list, vec![0, 2], 5).unwrap(), 3);
    assert_eq!(tracks(&library, list), ids(&[4, 1, 2, 0, 5, 3]));
    // Item ids follow their items.
    let after: Vec<_> = items(&library, list).iter().map(|i| i.id).collect();
    assert_eq!(after[3], before[0]);
    // Dragged to the end, and to the top.
    assert_eq!(library.move_playlist_items(list, vec![1], 6).unwrap(), 5);
    assert_eq!(tracks(&library, list), ids(&[4, 2, 0, 5, 3, 1]));
    assert_eq!(library.move_playlist_items(list, vec![4, 5], 0).unwrap(), 0);
    assert_eq!(tracks(&library, list), ids(&[3, 1, 4, 2, 0, 5]));
    // A move onto itself changes nothing.
    assert_eq!(library.move_playlist_items(list, vec![2], 2).unwrap(), 2);
    assert_eq!(tracks(&library, list), ids(&[3, 1, 4, 2, 0, 5]));

    assert_eq!(
        library.remove_from_playlist(list, vec![4, 1, 1]).unwrap(),
        2
    );
    assert_eq!(tracks(&library, list), ids(&[3, 4, 2, 5]));

    // Refused edits change nothing: they roll back whole.
    assert!(matches!(
        library.remove_from_playlist(list, vec![0, 4]),
        Err(PlaylistError::BadPosition {
            position: 4,
            len: 4
        })
    ));
    assert!(matches!(
        library.insert_into_playlist(list, 5, vec![t[0]]),
        Err(PlaylistError::BadPosition {
            position: 5,
            len: 4
        })
    ));
    assert!(matches!(
        library.move_playlist_items(list, vec![0], 9),
        Err(PlaylistError::BadPosition { .. })
    ));
    let unknown = TrackId::new(999_999);
    assert!(matches!(
        library.add_to_playlist(list, vec![t[0], unknown]),
        Err(PlaylistError::UnknownTrack(id)) if id == unknown
    ));
    assert_eq!(tracks(&library, list), ids(&[3, 4, 2, 5]));

    let page = library.playlist_items(list, Page::new(1, 2)).unwrap();
    assert_eq!(page.total, 4);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].position, 1);
}

#[test]
fn items_follow_their_tracks() {
    let dir = folder("moves");
    let (library, root) = library(&dir);
    let old = dir.join("music/Old/01.flac");
    let a = add(&library, record(root, &old, "song", ("Song", "Band", 180)));
    let b = add(
        &library,
        record(root, dir.join("music/b.flac"), "b", ("B", "Band", 90)),
    );
    let list = library.create_playlist("Moves").unwrap();
    library.add_to_playlist(list, vec![a, b]).unwrap();

    // A watched rename.
    let new = dir.join("music/New/01 Song.flac");
    library.move_file(root, &old, &new).unwrap();
    assert_eq!(tracks(&library, list), [Some(a), Some(b)]);
    let out = dir.join("after-move.m3u8");
    library
        .export_playlist(list, &out, ExportOptions::default())
        .unwrap();
    let text = fs::read_to_string(&out).unwrap();
    assert!(text.contains("music/New/01 Song.flac\n"), "{text}");

    // A file that disappeared and turned up elsewhere.
    library.mark_file_missing(&new, SystemTime::now()).unwrap();
    let moved = dir.join("music/Elsewhere/01.flac");
    let write = library
        .upsert_track(record(root, &moved, "song", ("Song", "Band", 180)))
        .unwrap();
    assert_eq!(write.change, TrackChange::Moved);
    assert_eq!(tracks(&library, list), [Some(a), Some(b)]);

    // A track that left the library: its item is dead but remembers it…
    library.remove_file(&moved).unwrap();
    let dead = library.dead_playlist_items(list).unwrap();
    assert_eq!(dead.len(), 1);
    let ItemContent::Dead(item) = &dead[0].content else {
        panic!("not dead")
    };
    assert!(item.identity.is_some());
    assert_eq!(item.title.as_deref(), Some("Song"));
    assert_eq!(library.playlist(list).unwrap().unwrap().dead, 1);
    // …and comes back to life when the track returns.
    let back = library
        .upsert_track(record(
            root,
            dir.join("music/Back/01.flac"),
            "song",
            ("Song", "Band", 180),
        ))
        .unwrap();
    assert_eq!(back.change, TrackChange::Returned);
    assert_eq!(tracks(&library, list), [Some(back.track), Some(b)]);
    assert_eq!(library.playlist(list).unwrap().unwrap().dead, 0);
}

#[test]
fn import_matches_by_path_name_and_tags() {
    let dir = folder("import");
    let (library, root) = library(&dir);
    let music = dir.join("music");
    let by_abs = add(
        &library,
        record(root, music.join("A/01.flac"), "1", ("One", "Alpha", 200)),
    );
    let by_rel = add(
        &library,
        record(root, music.join("A/02.flac"), "2", ("Two", "Alpha", 210)),
    );
    let by_win = add(
        &library,
        record(
            root,
            music.join("Beta/Album/03 Three.mp3"),
            "3",
            ("Three", "Beta", 150),
        ),
    );
    let by_url = add(
        &library,
        record(
            root,
            music.join("C/04 Four.flac"),
            "4",
            ("Four", "Gamma", 240),
        ),
    );
    let by_tags = add(
        &library,
        record(root, music.join("D/x.flac"), "5", ("Fünf", "Delta", 300)),
    );
    // Same file name in two folders: the folder decides.
    let near = add(
        &library,
        record(root, music.join("E/CD1/01.flac"), "6", ("Six", "Eps", 60)),
    );
    let _far = add(
        &library,
        record(root, music.join("F/CD2/01.flac"), "7", ("Sev", "Eps", 60)),
    );

    let m3u = format!(
        "\u{feff}#EXTM3U\r\n\
         #PLAYLIST:Imported mix\r\n\
         {abs}\r\n\
         music\\A\\02.flac\r\n\
         #EXTINF:150,Beta - Three\r\n\
         C:\\Users\\me\\Music\\Beta\\Album\\03 Three.mp3\r\n\
         file://{url}\r\n\
         #EXTINF:299,delta - FÜNF\r\n\
         /gone/renamed.flac\r\n\
         D:\\Old\\CD1\\01.flac\r\n\
         #EXTINF:-1,Live radio\r\n\
         http://radio.invalid/stream\r\n\
         nested.m3u\r\n\
         #EXTINF:100,Nobody - Nothing\r\n\
         missing/nothing.mp3\r\n",
        abs = music.join("A/01.flac").display(),
        url = music.join("C/04%20Four.flac").display(),
    );
    let path = dir.join("mix.m3u");
    fs::write(&path, m3u).unwrap();
    let report = library
        .import_playlist(&path, &ImportOptions::default())
        .unwrap();
    assert_eq!(report.name, "Imported mix");
    assert_eq!(report.kind, PlaylistKind::M3u);
    assert_eq!(report.entries, 8);
    assert_eq!((report.by_path, report.by_name, report.by_tags), (3, 2, 1));
    assert_eq!((report.dead, report.unsupported), (2, 1));
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w.kind, PlaylistWarningKind::NestedPlaylist(_)))
    );
    assert_eq!(
        tracks(&library, report.playlist),
        [
            Some(by_abs),
            Some(by_rel),
            Some(by_win),
            Some(by_url),
            Some(by_tags),
            Some(near),
            None,
            None
        ]
    );
    let dead = library.dead_playlist_items(report.playlist).unwrap();
    let references: Vec<_> = dead
        .iter()
        .map(|item| match &item.content {
            ItemContent::Dead(dead) => dead.reference.clone(),
            ItemContent::Track(_) => panic!("alive"),
        })
        .collect();
    assert_eq!(
        references,
        [
            DeadReference::Url("http://radio.invalid/stream".into()),
            DeadReference::Path(dir.join("missing/nothing.mp3")),
        ]
    );
    let ItemContent::Dead(nothing) = &dead[1].content else {
        unreachable!()
    };
    assert_eq!(nothing.artist.as_deref(), Some("Nobody"));
    assert_eq!(nothing.duration, Some(Duration::from_secs(100)));

    // The name can be chosen; without a title the file's name is used.
    let plain = dir.join("Plain list.m3u8");
    fs::write(&plain, "music/A/01.flac\n").unwrap();
    let named = library
        .import_playlist(&plain, &ImportOptions::default())
        .unwrap();
    assert_eq!(named.name, "Plain list");
    let chosen = library
        .import_playlist(
            &plain,
            &ImportOptions {
                name: Some("Chosen".into()),
                ..ImportOptions::default()
            },
        )
        .unwrap();
    assert_eq!(
        library.playlist(chosen.playlist).unwrap().unwrap().name,
        "Chosen"
    );
}

#[test]
fn non_utf8_names_survive_export_and_import() {
    let dir = folder("bytes");
    let (library, root) = library(&dir);
    let latin1 = bytes_path(&dir, b"music/Bj\xf6rk/J\xf3ga.flac");
    let utf8 = dir.join("music/Sigur Rós/Svefn-g-englar.flac");
    let a = add(
        &library,
        record(root, &latin1, "joga", ("Jóga", "Björk", 305)),
    );
    let b = add(
        &library,
        record(root, &utf8, "svefn", ("Svefn-g-englar", "Sigur Rós", 600)),
    );
    let list = library.create_playlist("Iceland").unwrap();
    library.add_to_playlist(list, vec![a, b]).unwrap();

    for kind in [PlaylistKind::M3u, PlaylistKind::Pls, PlaylistKind::Xspf] {
        for path_style in [PathStyle::Relative, PathStyle::Absolute] {
            let out = dir.join(format!("iceland-{path_style:?}.{}", kind.extension()));
            let report = library
                .export_playlist(
                    list,
                    &out,
                    ExportOptions {
                        kind,
                        path_style,
                        skip_dead: false,
                    },
                )
                .unwrap();
            assert_eq!((report.entries, report.dead), (2, 0));
            assert!(report.warnings.is_empty(), "{kind}: {:?}", report.warnings);
            let back = library
                .import_playlist(&out, &ImportOptions::default())
                .unwrap();
            assert_eq!(back.by_path, 2, "{kind} {path_style:?}");
            assert_eq!(tracks(&library, back.playlist), [Some(a), Some(b)]);
        }
    }
    // M3U8 is UTF-8: the Latin-1 name is written as text and reported.
    let out = dir.join("iceland.m3u8");
    let report = library
        .export_playlist(list, &out, ExportOptions::default())
        .unwrap();
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.warnings[0].kind, WriteWarningKind::NotUtf8);
    // It still finds the track by the end of its path and by its tags.
    let back = library
        .import_playlist(&out, &ImportOptions::default())
        .unwrap();
    assert_eq!(tracks(&library, back.playlist), [Some(a), Some(b)]);
    // No temporary files are left beside the exports.
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().as_bytes().ends_with(b".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn cp1251_playlists_find_both_spellings() {
    let dir = folder("cp1251");
    let (library, root) = library(&dir);
    // One file named in UTF-8 as today, one still named in cp1251 bytes.
    let cp = cp1251("Кино/Пачка сигарет.mp3");
    let modern = add(
        &library,
        record(
            root,
            dir.join("music/Кино/Группа крови.mp3"),
            "a",
            ("Группа крови", "Кино", 281),
        ),
    );
    let legacy = add(
        &library,
        record(
            root,
            bytes_path(&dir.join("music"), &cp),
            "b",
            ("Пачка сигарет", "Кино", 268),
        ),
    );
    let file = cp1251(
        "#EXTM3U\r\n#EXTINF:281,Кино - Группа крови\r\nmusic\\Кино\\Группа крови.mp3\r\n\
         #EXTINF:268,Кино - Пачка сигарет\r\nmusic\\Кино\\Пачка сигарет.mp3\r\n",
    );
    let path = dir.join("kino.m3u");
    fs::write(&path, &file).unwrap();
    let report = library
        .import_playlist(&path, &ImportOptions::default())
        .unwrap();
    assert_eq!(report.encoding.name(), "windows-1251");
    assert_eq!(report.by_path, 2);
    assert_eq!(
        tracks(&library, report.playlist),
        [Some(modern), Some(legacy)]
    );
}

#[test]
fn export_replaces_files_and_keeps_dead_entries() {
    let dir = folder("export");
    let (library, root) = library(&dir);
    let a = add(
        &library,
        record(root, dir.join("music/a.flac"), "a", ("A", "X", 61)),
    );
    let path = dir.join("in.m3u8");
    fs::write(
        &path,
        "music/a.flac\n#EXTINF:5,Ghost - Gone\n/nowhere/gone.flac\nhttps://x.invalid/s\n",
    )
    .unwrap();
    let imported = library
        .import_playlist(&path, &ImportOptions::default())
        .unwrap();
    assert_eq!(tracks(&library, imported.playlist), [Some(a), None, None]);

    let out = dir.join("out.m3u8");
    fs::write(&out, "old contents").unwrap();
    let report = library
        .export_playlist(
            imported.playlist,
            &out,
            ExportOptions {
                path_style: PathStyle::Absolute,
                ..ExportOptions::default()
            },
        )
        .unwrap();
    assert_eq!((report.entries, report.dead), (3, 2));
    let text = fs::read_to_string(&out).unwrap();
    assert_eq!(
        text,
        format!(
            "#EXTM3U\n#PLAYLIST:in\n#EXTINF:61,X - A\n#EXTALB:Album\n{}\n\
             #EXTINF:5,Ghost - Gone\n/nowhere/gone.flac\nhttps://x.invalid/s\n",
            dir.join("music/a.flac").display()
        )
    );
    let report = library
        .export_playlist(
            imported.playlist,
            &out,
            ExportOptions {
                kind: PlaylistKind::Xspf,
                skip_dead: true,
                ..ExportOptions::default()
            },
        )
        .unwrap();
    assert_eq!((report.entries, report.dead), (1, 2));
    let text = fs::read_to_string(&out).unwrap();
    assert!(text.contains("<location>music/a.flac</location>"), "{text}");
    assert!(!text.contains("gone"));

    // A folder that does not exist: an error, and nothing written.
    assert!(matches!(
        library.export_playlist(
            imported.playlist,
            &dir.join("no/such/x.m3u8"),
            ExportOptions::default()
        ),
        Err(PlaylistError::Write { .. })
    ));
}

#[test]
fn hostile_files_are_refused_or_bounded() {
    let dir = folder("hostile");
    let (library, _) = library(&dir);
    let missing = dir.join("missing.m3u");
    assert!(matches!(
        library.import_playlist(&missing, &ImportOptions::default()),
        Err(PlaylistError::Read(_))
    ));
    let big = dir.join("big.m3u");
    fs::write(&big, vec![b'#'; 2048]).unwrap();
    let options = ImportOptions {
        limits: helikodu_meta::playlists::PlaylistLimits {
            max_bytes: 1024,
            ..Default::default()
        },
        ..ImportOptions::default()
    };
    assert!(matches!(
        library.import_playlist(&big, &options),
        Err(PlaylistError::Read(_))
    ));
    // Garbage still becomes an empty playlist, not an error.
    let junk = dir.join("junk.xspf");
    fs::write(&junk, b"\x00\xff<playlist><trackList><track><location>").unwrap();
    let report = library
        .import_playlist(&junk, &ImportOptions::default())
        .unwrap();
    assert_eq!(report.entries, 0);
    assert!(library.playlist(report.playlist).unwrap().is_some());
}

#[test]
fn favourites_is_built_in() {
    let dir = folder("favourites");
    let (library, root) = library(&dir);
    let a = add(
        &library,
        record(root, dir.join("music/a.flac"), "a", ("A", "X", 100)),
    );
    let b = add(
        &library,
        record(root, dir.join("music/b.flac"), "b", ("B", "X", 100)),
    );
    let favourites = library.favourites().unwrap();
    let summary = library.playlist(favourites).unwrap().unwrap();
    assert!(summary.builtin);
    assert_eq!(summary.name, "Favourites");
    assert_eq!(summary.items, 0);

    // The heart adds a track once, and takes it out again.
    assert!(!library.is_favourite(a).unwrap());
    assert!(library.set_favourite(a, true).unwrap());
    assert!(library.set_favourite(a, true).unwrap());
    assert!(library.set_favourite(b, true).unwrap());
    assert_eq!(tracks(&library, favourites), [Some(a), Some(b)]);
    assert!(library.is_favourite(a).unwrap());
    assert_eq!(library.favourite_tracks(vec![a, b]).unwrap(), [a, b]);
    assert!(!library.set_favourite(a, false).unwrap());
    assert!(!library.is_favourite(a).unwrap());
    assert_eq!(tracks(&library, favourites), [Some(b)]);
    // Added by hand too, it is the same list.
    library.add_to_playlist(favourites, vec![a]).unwrap();
    assert!(library.is_favourite(a).unwrap());

    // It stays first and cannot be renamed, moved, locked or deleted.
    let other = library.create_playlist("Other").unwrap();
    library.move_playlist(other, None, Some(0)).unwrap();
    assert_eq!(
        library.playlists(Page::default()).unwrap().items[0].id,
        favourites
    );
    let folder = library.create_playlist_folder("Folder", None).unwrap();
    for refused in [
        library.rename_playlist(favourites, "Loved"),
        library.delete_playlist(favourites),
        library.move_playlist(favourites, Some(folder), None),
        library.set_playlist_locked(favourites, true),
    ] {
        assert!(
            matches!(refused, Err(PlaylistError::Builtin(id)) if id == favourites),
            "{refused:?}"
        );
    }
    assert!(library.playlist(favourites).unwrap().is_some());
    assert_eq!(library.favourites().unwrap(), favourites);
}
