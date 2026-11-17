// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Albums from real files through the scanner and its worker processes: a CUE album
//! (a FLAC file and its sheet), the chapters of an audiobook, and an album with its
//! discs in folders of their own and a classical credit.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[allow(dead_code)]
#[path = "support/scanning.rs"]
mod support;

use std::path::Path;
use std::process::Command;

use helikodu_library::{
    AlbumQuery, ArtistQuery, ArtistRole, CancelToken, DatabaseConfig, Library, Page, ScanOptions,
    TrackKind, TrackSection,
};
use support::{flac, folder, have_ffmpeg, worker_program, write};

fn scan(library: &Library, root: &Path) {
    let mut options = ScanOptions::new([root]);
    options.workers.program = Some(worker_program());
    options.workers.count = 2;
    let summary = library.scan(&options, None, &CancelToken::new()).unwrap();
    assert_eq!(summary.failed, 0, "{summary:?}");
}

const CUE_SHEET: &str = "REM DATE 2001\r\nPERFORMER \"Cue Artist\"\r\nTITLE \"Cue Album\"\r\n\
FILE \"album.flac\" WAVE\r\n\
  TRACK 01 AUDIO\r\n    TITLE \"First\"\r\n    INDEX 01 00:00:00\r\n\
  TRACK 02 AUDIO\r\n    TITLE \"Second\"\r\n    PERFORMER \"Guest\"\r\n    INDEX 01 00:03:00\r\n\
  TRACK 03 AUDIO\r\n    TITLE \"Third\"\r\n    INDEX 01 00:06:00\r\n";

#[test]
fn a_cue_album_is_one_album_of_virtual_tracks() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("catalog-cue");
    flac(root.join("Cue/album.flac"), 660, 10, &[]);
    write(root.join("Cue/album.cue"), CUE_SHEET);
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    scan(&library, &root);

    let albums = library.albums(&AlbumQuery::default()).unwrap();
    assert_eq!(albums.total, 1, "{albums:?}");
    let album = &albums.items[0];
    assert_eq!(album.title.as_deref(), Some("Cue Album"));
    assert_eq!(album.artist.as_deref(), Some("Cue Artist"));
    assert_eq!(album.year, Some(2001));
    assert_eq!(album.track_count, 3);
    let detail = library.album(album.id).unwrap().unwrap();
    let tracks: Vec<_> = detail.discs[0]
        .tracks
        .iter()
        .map(|t| {
            (
                t.title.clone().unwrap(),
                t.artist.clone().unwrap(),
                t.sub_index,
                t.section,
                t.duration.unwrap().as_secs(),
            )
        })
        .collect();
    assert_eq!(
        tracks,
        [
            (
                "First".into(),
                "Cue Artist".into(),
                1,
                TrackSection::CueSheet,
                3
            ),
            (
                "Second".into(),
                "Guest".into(),
                2,
                TrackSection::CueSheet,
                3
            ),
            (
                "Third".into(),
                "Cue Artist".into(),
                3,
                TrackSection::CueSheet,
                4
            ),
        ]
    );
    // Each part of the file has its own identity.
    let identities: Vec<_> = detail.discs[0]
        .tracks
        .iter()
        .map(|t| library.identity_of(t.id).unwrap())
        .collect();
    assert!(identities[0] != identities[1] && identities[1] != identities[2]);
    let guest = library
        .artists(&ArtistQuery::new(ArtistRole::Artist))
        .unwrap()
        .items
        .into_iter()
        .find(|a| a.name == "Guest")
        .unwrap();
    assert_eq!((guest.album_count, guest.track_count), (1, 1));
}

/// An M4B audiobook of six seconds with three chapters.
fn audiobook(path: &Path) {
    let meta = path.with_extension("txt");
    write(
        &meta,
        ";FFMETADATA1\ntitle=The Book\nartist=Author\nalbum=The Book\ndate=2020\n\
         [CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=2000\ntitle=Opening\n\
         [CHAPTER]\nTIMEBASE=1/1000\nSTART=2000\nEND=4000\ntitle=Middle\n\
         [CHAPTER]\nTIMEBASE=1/1000\nSTART=4000\nEND=6000\ntitle=Ending\n",
    );
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg("sine=frequency=440:duration=6:sample_rate=44100")
        .arg("-i")
        .arg(&meta)
        .args([
            "-map_metadata",
            "1",
            "-map_chapters",
            "1",
            "-c:a",
            "aac",
            "-f",
            "mp4",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_file(meta).unwrap();
}

#[test]
fn the_chapters_of_an_audiobook_are_stored_as_tracks() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("catalog-chapters");
    let book = root.join("Author/The Book.m4b");
    std::fs::create_dir_all(book.parent().unwrap()).unwrap();
    audiobook(&book);
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    scan(&library, &root);

    assert_eq!(
        library.track_at(&book, 0).unwrap(),
        None,
        "no whole-file track"
    );
    let albums = library.albums(&AlbumQuery::default()).unwrap();
    assert_eq!(albums.total, 1, "{albums:?}");
    assert_eq!(albums.items[0].title.as_deref(), Some("The Book"));
    assert_eq!(albums.items[0].artist.as_deref(), Some("Author"));
    let detail = library.album(albums.items[0].id).unwrap().unwrap();
    let chapters: Vec<_> = detail.discs[0]
        .tracks
        .iter()
        .map(|t| {
            (
                t.title.clone().unwrap(),
                t.track_no,
                t.sub_index,
                t.section,
                t.kind,
            )
        })
        .collect();
    assert_eq!(
        chapters,
        [
            (
                "Opening".into(),
                Some(1),
                1,
                TrackSection::Chapter,
                TrackKind::Audiobook
            ),
            (
                "Middle".into(),
                Some(2),
                2,
                TrackSection::Chapter,
                TrackKind::Audiobook
            ),
            (
                "Ending".into(),
                Some(3),
                3,
                TrackSection::Chapter,
                TrackKind::Audiobook
            ),
        ]
    );
    let durations: Vec<u128> = detail.discs[0]
        .tracks
        .iter()
        .map(|t| t.duration.unwrap().as_millis())
        .collect();
    assert!(
        durations[..2].iter().all(|ms| (1900..=2100).contains(ms)),
        "{durations:?}"
    );
}

#[test]
fn discs_in_folders_and_credits_from_real_tags() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("catalog-discs");
    for (disc, number, title) in [("CD1", "1", "A"), ("CD1", "2", "B"), ("CD2", "1", "C")] {
        flac(
            root.join(format!("Orchestra/Symphonies/{disc}/{number}.flac")),
            440,
            1,
            &[
                ("title", title),
                ("artist", "Orchestra"),
                ("albumartist", "Orchestra"),
                ("album", "Symphonies"),
                ("date", "1999"),
                ("tracknumber", number),
                ("composer", "Composer"),
                ("conductor", "Conductor"),
                ("genre", "Classical"),
            ],
        );
    }
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    scan(&library, &root);
    let albums = library
        .albums(&AlbumQuery {
            page: Page::first(10),
            ..AlbumQuery::default()
        })
        .unwrap();
    assert_eq!(albums.total, 1, "{albums:?}");
    assert_eq!(albums.items[0].disc_count, 2);
    let detail = library.album(albums.items[0].id).unwrap().unwrap();
    let discs: Vec<_> = detail
        .discs
        .iter()
        .map(|d| (d.number, d.tracks.len()))
        .collect();
    assert_eq!(discs, [(1, 2), (2, 1)]);
    for role in [ArtistRole::Composer, ArtistRole::Conductor] {
        let people = library.artists(&ArtistQuery::new(role)).unwrap();
        assert_eq!(people.total, 1, "{role:?}: {people:?}");
        assert_eq!(people.items[0].track_count, 3);
    }
}
