// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tag edits and ratings through the library, with real worker processes over
//! files made with ffmpeg: the database follows a write at once, tracks keep their
//! ids and identities, and ratings reach the files only when that is switched on.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[allow(dead_code)]
#[path = "support/scanning.rs"]
mod support;

use std::path::Path;

use helikodu_library::{
    CancelToken, DatabaseConfig, Library, Rating, ScanOptions, SearchRequest, TagWriteSettings,
};
use helikodu_meta::tags::writing::{BatchItem, TagChanges, TagField, load_for_editing};
use helikodu_meta::tags::{ReadOptions, read_tags};
use helikodu_model::library::TrackId;
use support::{flac, folder, have_ffmpeg, worker_program};

fn options(root: &Path) -> ScanOptions {
    let mut options = ScanOptions::new([root]);
    options.workers.program = Some(worker_program());
    options.workers.count = 1;
    options
}

fn column(library: &Library, track: TrackId, column: &str) -> Option<String> {
    let id = i64::try_from(track.get()).unwrap();
    let sql = format!("SELECT {column} FROM tracks WHERE id = ?1");
    library
        .database()
        .read(|c| c.query_row(&sql, [id], |r| r.get(0)))
        .unwrap()
}

fn file_rating(path: &Path) -> Option<u8> {
    read_tags(path, &ReadOptions::default())
        .unwrap()
        .tags
        .rating
        .map(|r| r.half_stars())
}

#[test]
fn a_tag_write_updates_the_library_at_once() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("tag-writes").join("music");
    let path = root.join("Artist/Album/01.flac");
    let other = root.join("Artist/Album/02.flac");
    flac(&path, 440, 1, &[("title", "Old title"), ("album", "Album")]);
    flac(&other, 550, 1, &[("title", "Other"), ("album", "Album")]);
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let options = options(&root);
    library.scan(&options, None, &CancelToken::new()).unwrap();
    let track = library.track_at(&path, 0).unwrap().unwrap();
    let identity = library.identity_of(track).unwrap();
    library.set_rating(track, Rating::from_stars(4)).unwrap();

    let file = load_for_editing(&path, &ReadOptions::default()).unwrap();
    let mut changes = TagChanges::new();
    changes
        .set_one(TagField::Title, "Brand new title")
        .set(TagField::Genre, ["Jazz"]);
    let items = [BatchItem::for_file(&file, changes)];
    let mut steps = 0;
    let report = library
        .write_tags(
            &items,
            &TagWriteSettings::default(),
            &options,
            &mut |_| steps += 1,
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(steps, 1);
    assert_eq!(report.batch.written().count(), 1);
    let refresh = report.refresh.unwrap();
    assert_eq!(refresh.updated, 1, "{refresh:?}");
    assert_eq!(refresh.read, 1, "only the written file is read again");

    // The same track and identity, with the new tags and its rating.
    assert_eq!(library.track_at(&path, 0).unwrap(), Some(track));
    assert_eq!(library.identity_of(track).unwrap(), identity);
    assert_eq!(
        column(&library, track, "title").as_deref(),
        Some("Brand new title")
    );
    assert_eq!(library.stats(track).unwrap().rating, Rating::from_stars(4));
    let hits = library
        .search(&SearchRequest::new("brand new"))
        .unwrap()
        .hits;
    assert_eq!(hits.first().map(|h| h.track), Some(track));
}

#[test]
fn ratings_reach_the_file_only_when_switched_on() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("rating-writes").join("music");
    let path = root.join("a.flac");
    flac(&path, 440, 1, &[("title", "Rated")]);
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let options = options(&root);
    library.scan(&options, None, &CancelToken::new()).unwrap();
    let track = library.track_at(&path, 0).unwrap().unwrap();

    let off = TagWriteSettings::default();
    assert!(!off.write_ratings_to_tags, "off by default");
    let outcome = library.rate_track(track, Some(4), &off, &options).unwrap();
    assert_eq!(outcome.rating.map(|r| r.half_stars()), Some(8));
    assert!(outcome.file.is_none());
    assert_eq!(
        library.stats(track).unwrap().rating,
        Rating::from_half_stars(8)
    );
    assert_eq!(file_rating(&path), None);

    let on = TagWriteSettings {
        write_ratings_to_tags: true,
        ..TagWriteSettings::default()
    };
    let outcome = library.rate_track(track, Some(3), &on, &options).unwrap();
    outcome.file.unwrap().unwrap();
    outcome.refresh.unwrap().unwrap();
    assert_eq!(file_rating(&path), Some(6));
    assert_eq!(
        library.stats(track).unwrap().rating,
        Rating::from_half_stars(6)
    );
    assert_eq!(library.track_at(&path, 0).unwrap(), Some(track));

    let outcome = library.rate_track(track, None, &on, &options).unwrap();
    outcome.file.unwrap().unwrap();
    assert_eq!(file_rating(&path), None);
    assert_eq!(library.stats(track).unwrap().rating, None);

    assert!(library.rate_track(track, Some(6), &on, &options).is_err());
}
