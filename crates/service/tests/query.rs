// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The query language through the library facade: lenient parsing for the search
//! entry, queries with paging, quick filters, and saved searches with their counts,
//! also through the library service's thread.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use helikodu_library::{
    AudioProperties, DatabaseConfig, FileStamp, Library, TrackRecord, TrackTags,
};
use helikodu_model::query::{Expr, QuickFilter};
use helikodu_service::bus::EventBus;
use helikodu_service::library::{
    LibraryError, LibraryService, LibraryServiceOptions, MusicLibrary, QueryError, SyntaxErrorKind,
    parse_query, parse_query_lenient, print_query,
};

/// A library of 12 tracks: years 1990 to 2001, FLAC for the even ones, genre Jazz for
/// every third.
fn library() -> MusicLibrary {
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let root = library.add_root("/music").unwrap();
    let records = (0..12)
        .map(|n| {
            let path = format!("/music/{n:02}.audio");
            let mut record = TrackRecord::new(
                root,
                &path,
                FileStamp {
                    size: 1000 + n,
                    mtime_ns: Some(1),
                    inode: None,
                    quick_hash: path.clone().into_bytes(),
                },
            );
            record.audio = AudioProperties {
                duration: Some(Duration::from_secs(100 + n * 10)),
                codec: Some(if n % 2 == 0 { "flac" } else { "mp3" }.into()),
                ..AudioProperties::default()
            };
            record.tags = TrackTags {
                title: Some(format!("Song {n}")),
                artists: vec!["Anton Verde".into()],
                genres: if n % 3 == 0 {
                    vec!["Jazz".into()]
                } else {
                    vec![]
                },
                year: Some(1990 + n as i32),
                ..TrackTags::default()
            };
            record
        })
        .collect();
    library.upsert_tracks(records).unwrap();
    MusicLibrary::from_library(Arc::new(library), ":memory:")
}

#[test]
fn queries_run_with_paging() {
    let library = library();
    let query = parse_query("codec:flac year>=1994 sort:year desc").unwrap();
    let page = library.query_tracks(&query, 1, 2).unwrap();
    assert_eq!(page.total, 4);
    let years: Vec<Option<i32>> = page.items.iter().map(|t| t.year).collect();
    assert_eq!(years, [Some(1998), Some(1996)]);
    assert_eq!(library.query_count(&query).unwrap(), 4);
    let ids = library.query_track_ids(&query, 0).unwrap();
    assert_eq!(ids.len(), 4);
    // Every quick filter runs.
    for filter in QuickFilter::ALL {
        library.query_count(&filter.query()).unwrap();
    }
    assert_eq!(
        library.query_count(&QuickFilter::Unplayed.query()).unwrap(),
        12
    );
    assert_eq!(
        library.query_count(&QuickFilter::Lossless.query()).unwrap(),
        6
    );
}

#[test]
fn the_search_entry_never_fails() {
    let library = library();
    let parsed = parse_query_lenient("song yaer:1990");
    let error = parsed.error.unwrap();
    assert_eq!(error.span(), 5..9);
    assert_eq!(error.kind(), &SyntaxErrorKind::UnknownField("yaer".into()));
    assert!(error.example().is_some());
    // The words are still searched for.
    assert_eq!(
        parsed.query.filter,
        Some(Expr::All(vec![Expr::text("song"), Expr::text("yaer:1990")]))
    );
    assert_eq!(library.query_count(&parsed.query).unwrap(), 0);
    let fine = parse_query_lenient("genre:jazz");
    assert_eq!(fine.error, None);
    assert_eq!(library.query_count(&fine.query).unwrap(), 4);
    assert!(parse_query_lenient("song 1").query.is_plain_text());
}

#[test]
fn saved_searches_come_with_counts() {
    let library = library();
    let jazz = parse_query("genre:jazz").unwrap();
    let old = parse_query("year<1995").unwrap();
    let a = library.save_search("Jazz", &jazz).unwrap();
    let b = library.save_search("Nineties", &old).unwrap();
    library.move_saved_search(b, 0).unwrap();
    let entries = library.saved_searches().unwrap();
    let summary: Vec<(&str, String, usize)> = entries
        .iter()
        .map(|e| {
            (
                e.search.name.as_str(),
                print_query(&e.search.query),
                e.count,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("Nineties", "year<1995".to_owned(), 5),
            ("Jazz", "genre:jazz".to_owned(), 4)
        ]
    );
    library.rename_saved_search(a, "All jazz").unwrap();
    library.set_saved_search_query(a, &old).unwrap();
    library.delete_saved_search(b).unwrap();
    let entries = library.saved_searches().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].search.name, "All jazz");
    assert_eq!(entries[0].count, 5);
    assert!(matches!(
        library.delete_saved_search(b),
        Err(LibraryError::Query(QueryError::UnknownSavedSearch(_)))
    ));
}

#[test]
fn the_service_thread_answers_queries() {
    let library = library();
    library
        .save_search("Jazz", &parse_query("genre:jazz").unwrap())
        .unwrap();
    let service =
        LibraryService::start(library, EventBus::new(), LibraryServiceOptions::default()).unwrap();
    let (send, receive) = mpsc::channel();
    let pages = send.clone();
    service
        .query_tracks(
            parse_query("year:1990..1992").unwrap(),
            0,
            50,
            move |result| {
                pages.send(result.map(|page| page.total)).unwrap();
            },
        )
        .unwrap();
    assert_eq!(
        receive
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap(),
        3
    );
    service
        .saved_searches(move |result| {
            send.send(result.map(|entries| entries[0].count)).unwrap();
        })
        .unwrap();
    assert_eq!(
        receive
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap(),
        4
    );
}
