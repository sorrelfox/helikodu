// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The library facade over a database file filled through the library crate:
//! search with paths, a track's details and statistics, a library file that cannot
//! be read any more, totals and the most played tracks, and the errors of a missing
//! library and of a scan without folders.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use helikodu_library::{
    AudioProperties, DatabaseConfig, FileStamp, Library, Listen, ListenEnd, PlayRules, TrackRecord,
    TrackTags,
};
use helikodu_model::library::TrackId;
use helikodu_service::library::{LibraryError, MusicLibrary, ScanRequest, TagOrigin};

fn database(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("service-library")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    dir.join("data").join("library.db")
}

fn record(library: &Library, path: &str, title: &str, album: &str) -> TrackId {
    let root = library.add_root("/nonexistent-music").unwrap();
    let mut record = TrackRecord::new(
        root,
        path,
        FileStamp {
            size: 1000,
            mtime_ns: Some(1),
            inode: None,
            quick_hash: path.as_bytes().to_vec(),
        },
    );
    record.audio = AudioProperties {
        duration: Some(Duration::from_secs(200)),
        sample_rate: Some(44_100),
        channels: Some(2),
        codec: Some("flac".into()),
        ..AudioProperties::default()
    };
    record.tags = TrackTags {
        title: Some(title.into()),
        artists: vec!["Ann".into(), "Bo".into()],
        album_artists: vec!["Ann".into()],
        composers: vec!["Cy".into()],
        album: Some(album.into()),
        genres: vec!["Folk".into(), "Pop".into()],
        track_no: Some(3),
        year: Some(1999),
        ..TrackTags::default()
    };
    library.upsert_track(record).unwrap().track
}

fn play(library: &Library, track: TrackId, times: usize) {
    for _ in 0..times {
        let listen = Listen {
            started_at: SystemTime::now(),
            played: Duration::from_secs(200),
            end: ListenEnd::Finished,
            position: None,
            context: None,
        };
        library
            .record_listen(track, listen, PlayRules::default())
            .unwrap();
    }
}

#[test]
fn a_filled_library() {
    let path = database("filled");
    // Opening creates the folder and the database.
    drop(MusicLibrary::open(&path).unwrap());
    let (river, rain, quiet) = {
        let library = Library::open(&DatabaseConfig::file(&path)).unwrap();
        let river = record(
            &library,
            "/nonexistent-music/a/river.flac",
            "River",
            "Water",
        );
        let rain = record(&library, "/nonexistent-music/a/rain.flac", "Rain", "Water");
        let quiet = record(
            &library,
            "/nonexistent-music/b/quiet.flac",
            "Quiet",
            "Other",
        );
        play(&library, river, 1);
        play(&library, rain, 3);
        (river, rain, quiet)
    };

    let library = MusicLibrary::open_existing(&path).unwrap();
    assert_eq!(library.path(), path);

    let page = library.search("ri", 0, 10).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.hits[0].hit.track, river);
    assert_eq!(
        page.hits[0].path.as_deref(),
        Some(Path::new("/nonexistent-music/a/river.flac"))
    );
    assert_eq!(library.search("water", 1, 10).unwrap().hits.len(), 1);

    let info = library.track_info(rain).unwrap().unwrap();
    assert_eq!(info.origin, TagOrigin::Library);
    assert_eq!(info.path, Path::new("/nonexistent-music/a/rain.flac"));
    assert_eq!(info.tags.title.as_deref(), Some("Rain"));
    assert_eq!(info.tags.artists, ["Ann", "Bo"]);
    assert_eq!(info.tags.album_artists, ["Ann"]);
    assert_eq!(info.tags.composers, ["Cy"]);
    assert_eq!(info.tags.genres, ["Folk", "Pop"]);
    assert_eq!(info.tags.track_no, Some(3));
    assert_eq!(info.tags.year, Some(1999));
    assert_eq!(info.audio.duration, Some(Duration::from_secs(200)));
    assert_eq!(info.audio.codec.as_deref(), Some("flac"));
    // The file does not exist: no loudness values, and why.
    assert_eq!(info.loudness, None);
    assert!(info.file_error.is_some());
    let entry = info.library.unwrap();
    assert_eq!(entry.track, rain);
    assert_eq!(entry.stats.play_count, 3);
    assert!(entry.stats.last_played.is_some());
    assert_eq!(entry.missing_since, None);
    assert!(library.track_info(TrackId::new(9999)).unwrap().is_none());

    // A library file that cannot be read is described from the library.
    let info = library
        .file_info(Path::new("/nonexistent-music/b/quiet.flac"))
        .unwrap();
    assert_eq!(info.origin, TagOrigin::Library);
    assert_eq!(info.library.unwrap().track, quiet);
    // One outside the library is an error.
    assert!(matches!(
        library.file_info(Path::new("/nonexistent-music/none.flac")),
        Err(LibraryError::ReadFile { .. })
    ));

    let overview = library.overview(5).unwrap();
    assert_eq!(overview.roots, [PathBuf::from("/nonexistent-music")]);
    let totals = &overview.totals;
    assert_eq!(totals.roots, 1);
    assert_eq!(totals.tracks, 3);
    assert_eq!(totals.files, 3);
    assert_eq!(totals.albums, 2);
    assert_eq!(totals.artists, 2);
    assert_eq!(totals.genres, 2);
    assert_eq!(totals.duration, Duration::from_secs(600));
    assert_eq!(totals.size, 3000);
    assert_eq!(totals.plays, 4);
    assert_eq!(totals.played_tracks, 2);
    assert_eq!(totals.rated_tracks, 0);
    let top: Vec<(TrackId, u64)> = overview
        .most_played
        .iter()
        .map(|t| (t.track, t.play_count))
        .collect();
    assert_eq!(top, [(rain, 3), (river, 1)]);
    assert_eq!(overview.most_played[0].artist.as_deref(), Some("Ann, Bo"));
    assert_eq!(overview.most_played[0].album.as_deref(), Some("Water"));
    assert_eq!(library.overview(1).unwrap().most_played.len(), 1);
}

#[test]
fn missing_library_and_scan_without_folders() {
    let path = database("empty");
    assert!(matches!(
        MusicLibrary::open_existing(&path),
        Err(LibraryError::NoLibrary(_))
    ));
    let library = MusicLibrary::open(&path).unwrap();
    assert!(matches!(
        library.scan(ScanRequest::default()),
        Err(LibraryError::NoFolders)
    ));
    let missing_worker = ScanRequest {
        folders: vec![path.parent().unwrap().to_path_buf()],
        worker_program: Some(path.with_file_name("no-such-worker")),
        ..ScanRequest::default()
    };
    assert!(matches!(
        library.scan(missing_worker),
        Err(LibraryError::Scan(_))
    ));
    let overview = library.overview(10).unwrap();
    assert_eq!(overview.totals, Default::default());
    assert!(overview.roots.is_empty());
}
