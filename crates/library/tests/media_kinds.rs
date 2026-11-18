// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Browsing by year and decade, media kinds the owner chooses, audiobooks left out of
//! shuffle, and the sections of a file with chapters.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use std::time::Duration;

use helikodu_library::{
    AlbumFilter, AlbumQuery, Library, Page, TrackFilter, TrackKind, TrackQuery, TrackSection,
    YearQuery, YearSpan,
};
use helikodu_model::library::TrackId;
use support::{library, tagged};

fn add(
    library: &Library,
    root: helikodu_library::RootId,
    path: &str,
    album: &str,
    year: Option<i32>,
    original: Option<i32>,
) -> TrackId {
    let mut record = tagged(root, path, path, path, "Okto", album);
    record.tags.year = year;
    record.catalog.original_year = original;
    library.upsert_track(record).unwrap().track
}

fn album_titles(library: &Library, filter: AlbumFilter) -> Vec<String> {
    let mut titles: Vec<String> = library
        .albums(&AlbumQuery {
            filter,
            page: Page::first(100),
            ..AlbumQuery::default()
        })
        .unwrap()
        .items
        .into_iter()
        .map(|a| a.title.unwrap_or_default())
        .collect();
    titles.sort();
    titles
}

fn track_count(library: &Library, filter: TrackFilter) -> usize {
    library
        .tracks(&TrackQuery {
            filter,
            page: Page::first(100),
            ..TrackQuery::default()
        })
        .unwrap()
        .total
}

#[test]
fn years_are_listed_newest_first_with_unknown_last() {
    let (library, root) = library();
    add(
        &library,
        root,
        "/music/a/1.flac",
        "Night Ferry",
        Some(2021),
        None,
    );
    add(
        &library,
        root,
        "/music/a/2.flac",
        "Night Ferry",
        Some(2021),
        None,
    );
    // The original year wins over the year of the release.
    add(
        &library,
        root,
        "/music/b/1.flac",
        "Reissue",
        Some(2020),
        Some(1998),
    );
    add(&library, root, "/music/c/1.flac", "Undated", None, None);
    add(&library, root, "/music/d/1.flac", "Amber", Some(2019), None);

    let years = library.years(&YearQuery::default()).unwrap();
    let shown: Vec<(Option<i32>, usize, usize)> = years
        .iter()
        .map(|y| (y.year, y.album_count, y.track_count))
        .collect();
    assert_eq!(
        shown,
        vec![
            (Some(2021), 1, 2),
            (Some(2019), 1, 1),
            (Some(1998), 1, 1),
            (None, 1, 1),
        ]
    );
}

#[test]
fn albums_and_tracks_of_a_year_a_decade_and_no_year() {
    let (library, root) = library();
    add(
        &library,
        root,
        "/music/a/1.flac",
        "Night Ferry",
        Some(2021),
        None,
    );
    add(
        &library,
        root,
        "/music/a/2.flac",
        "Night Ferry",
        Some(2021),
        None,
    );
    add(
        &library,
        root,
        "/music/b/1.flac",
        "Reissue",
        Some(2020),
        Some(1998),
    );
    add(&library, root, "/music/c/1.flac", "Undated", None, None);
    add(&library, root, "/music/d/1.flac", "Amber", Some(2019), None);

    assert_eq!(
        album_titles(&library, AlbumFilter::Years(YearSpan::year(2021))),
        ["Night Ferry"]
    );
    assert_eq!(
        album_titles(&library, AlbumFilter::Years(YearSpan::decade(2010))),
        ["Amber"]
    );
    assert_eq!(
        album_titles(&library, AlbumFilter::Years(YearSpan::decade(1990))),
        ["Reissue"]
    );
    assert!(album_titles(&library, AlbumFilter::Years(YearSpan::year(2020))).is_empty());
    assert_eq!(
        album_titles(&library, AlbumFilter::Years(YearSpan::Unknown)),
        ["Undated"]
    );
    assert_eq!(
        track_count(&library, TrackFilter::Years(YearSpan::year(2021))),
        2
    );
    assert_eq!(
        track_count(&library, TrackFilter::Years(YearSpan::decade(2010))),
        1
    );
    assert_eq!(
        track_count(&library, TrackFilter::Years(YearSpan::Unknown)),
        1
    );
}

fn kind_of(library: &Library, track: TrackId) -> TrackKind {
    library
        .tracks(&TrackQuery::default())
        .unwrap()
        .items
        .into_iter()
        .find(|t| t.id == track)
        .unwrap()
        .kind
}

#[test]
fn a_chosen_kind_survives_a_rescan() {
    let (library, root) = library();
    let path = "/music/book/part.mp3";
    let track = add(&library, root, path, "Book", None, None);
    assert_eq!(kind_of(&library, track), TrackKind::Music);

    library.set_kind(&[track], TrackKind::Audiobook).unwrap();
    assert_eq!(kind_of(&library, track), TrackKind::Audiobook);

    // The scanner reads the file again and finds music; the choice stays.
    let again = add(&library, root, path, "Book", Some(2001), None);
    assert_eq!(again, track);
    assert_eq!(kind_of(&library, track), TrackKind::Audiobook);

    library.set_kind(&[track], TrackKind::Music).unwrap();
    assert_eq!(kind_of(&library, track), TrackKind::Music);
}

#[test]
fn audiobooks_are_left_out_of_shuffle() {
    let (library, root) = library();
    let song = add(&library, root, "/music/a/song.flac", "A", None, None);
    let mut book = tagged(root, "/music/b/book.m4b", "book", "Book", "Liis", "Book");
    book.kind = TrackKind::Audiobook;
    let book = library.upsert_track(book).unwrap().track;

    let keys = library.play_keys(&[song, book]).unwrap();
    assert!(!keys[&song].flags.skip_in_shuffle);
    assert!(keys[&book].flags.skip_in_shuffle);

    library.set_kind(&[song], TrackKind::Audiobook).unwrap();
    let keys = library.play_keys(&[song]).unwrap();
    assert!(keys[&song].flags.skip_in_shuffle);
}

#[test]
fn the_sections_of_a_file_with_chapters() {
    let (library, root) = library();
    let base = tagged(root, "/music/book.m4b", "book", "", "Liis", "Book");
    let mut records = Vec::new();
    for (n, (title, start, end)) in [
        ("Prologue", 0u64, Some(44_100 * 60)),
        ("January", 44_100 * 60, Some(44_100 * 150)),
        ("February", 44_100 * 150, None),
    ]
    .into_iter()
    .enumerate()
    {
        let mut chapter = base.clone();
        chapter.sub_index = u32::try_from(n + 1).unwrap();
        chapter.section = TrackSection::Chapter;
        chapter.frames = Some((start, end));
        chapter.tags.title = Some(title.to_owned());
        chapter.audio.duration = end.map(|end| Duration::from_secs((end - start) / 44_100));
        records.push(chapter);
    }
    let written = library.upsert_tracks(records).unwrap();
    let sections = library.file_sections(written[1].track).unwrap();
    let shown: Vec<(u32, &str, Duration)> = sections
        .iter()
        .map(|s| (s.number, s.title.as_deref().unwrap_or(""), s.start))
        .collect();
    assert_eq!(
        shown,
        vec![
            (1, "Prologue", Duration::ZERO),
            (2, "January", Duration::from_secs(60)),
            (3, "February", Duration::from_secs(150)),
        ]
    );
    assert_eq!(sections[1].id, written[1].track);
    assert_eq!(sections[1].length, Some(Duration::from_secs(90)));

    // A whole file has no sections.
    let song = add(&library, root, "/music/a/song.flac", "A", None, None);
    assert!(library.file_sections(song).unwrap().is_empty());
}
