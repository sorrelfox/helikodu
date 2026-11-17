// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Plain search through the public API: scripts with and without word spacing, short
//! queries, ranking, paging, and the indexes kept in step with the tracks.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use helikodu_library::raw::rusqlite;
use helikodu_library::{
    Library, MatchRank, RootId, SearchRequest, TrackChange, TrackRecord, TrackTags,
};
use helikodu_model::library::TrackId;
use support::{library, tagged};

/// `(title, artist, album)` of each test track; the track ids are 1, 2, ….
const TRACKS: &[(&str, &str, &str)] = &[
    ("月亮代表我的心", "邓丽君", "淡淡幽情"),
    ("千と千尋の神隠し", "久石譲", "ジブリの歌"),
    ("ลมหายใจ", "บอดี้สแลม", "คราม"),
    ("Jóga", "Björk", "Homogenic"),
    ("Straße der Lieder", "Die Ärzte", "Geräusch"),
    ("Symphony No. 9", "Антонин Дворжак", "Новый Свет"),
    ("夜に駆ける", "YOASOBI", "THE BOOK"),
    ("Ελάτε να χορέψουμε", "Χαρούλα Αλεξίου", "Τα τσιγάρα"),
    ("사랑했지만", "김광석", "다시 부르기"),
    ("Café del Mar", "Energy 52", "Café del Mar"),
    ("ひこうき雲", "荒井由実", "ひこうき雲"),
];

fn record(root: RootId, id: usize) -> TrackRecord {
    let (title, artist, album) = TRACKS[id - 1];
    tagged(
        root,
        &format!("/music/{id}.flac"),
        &format!("content {id}"),
        title,
        artist,
        album,
    )
}

/// A library holding the test tracks.
fn scripts() -> Library {
    let (library, root) = library();
    let records = (1..=TRACKS.len()).map(|id| record(root, id)).collect();
    let written = library.upsert_tracks(records).unwrap();
    for (expected, write) in (1_u64..).zip(&written) {
        assert_eq!(write.track, TrackId::new(expected));
        assert_eq!(write.change, TrackChange::Added);
    }
    library
}

/// The ids of the tracks found for `query`, sorted.
fn found(library: &Library, query: &str) -> Vec<u64> {
    let results = library
        .search(&SearchRequest::new(query).page(0, 1000))
        .unwrap();
    assert_eq!(results.total, results.hits.len(), "{query}");
    let mut ids: Vec<u64> = results.hits.iter().map(|hit| hit.track.get()).collect();
    ids.sort_unstable();
    ids
}

const NONE: [u64; 0] = [];

#[test]
fn latin_without_case_or_accents() {
    let library = scripts();
    assert_eq!(found(&library, "bjork"), [4]);
    assert_eq!(found(&library, "BJÖRK"), [4]);
    assert_eq!(found(&library, "joga"), [4]);
    assert_eq!(found(&library, "JORK"), [4]);
    assert_eq!(found(&library, "strasse"), [5]);
    assert_eq!(found(&library, "Straße"), [5]);
    assert_eq!(found(&library, "arzte"), [5]);
    assert_eq!(found(&library, "aerzte"), NONE);
    assert_eq!(found(&library, "cafe"), [10]);
    assert_eq!(found(&library, "CAFÉ DEL"), [10]);
    // Decomposed input is the same as composed input.
    assert_eq!(found(&library, "Cafe\u{301}"), [10]);
}

#[test]
fn cyrillic_and_greek() {
    let library = scripts();
    assert_eq!(found(&library, "дворж"), [6]);
    assert_eq!(found(&library, "ДВОРЖАК"), [6]);
    assert_eq!(found(&library, "новый свет"), [6]);
    // Greek accents and final sigma fold away.
    assert_eq!(found(&library, "ελατε"), [8]);
    assert_eq!(found(&library, "ΧΟΡΕΨΟΥΜΕ"), [8]);
    assert_eq!(found(&library, "αλεξιου"), [8]);
    assert_eq!(found(&library, "τσιγαρα"), [8]);
}

#[test]
fn chinese_with_queries_of_one_to_four_characters() {
    let library = scripts();
    assert_eq!(found(&library, "代表我的"), [1]);
    assert_eq!(found(&library, "我的心"), [1]);
    assert_eq!(found(&library, "亮代"), [1]);
    assert_eq!(found(&library, "心"), [1]);
    assert_eq!(found(&library, "邓丽君"), [1]);
    assert_eq!(found(&library, "幽情"), [1]);
}

#[test]
fn japanese_kana_and_kanji() {
    let library = scripts();
    assert_eq!(found(&library, "千尋の"), [2]);
    assert_eq!(found(&library, "神隠し"), [2]);
    assert_eq!(found(&library, "駆ける"), [7]);
    assert_eq!(found(&library, "の"), [2]);
    assert_eq!(found(&library, "久石譲"), [2]);
    assert_eq!(found(&library, "ジブリ"), [2]);
    // Half-width katakana fold to the indexed full-width form.
    assert_eq!(found(&library, "ｼﾞﾌﾞﾘ"), [2]);
    assert_eq!(found(&library, "ひこうき"), [11]);
    assert_eq!(found(&library, "雲"), [11]);
    assert_eq!(found(&library, "由実"), [11]);
    assert_eq!(found(&library, "yoasobi"), [7]);
    assert_eq!(found(&library, "the book"), [7]);
}

#[test]
fn thai_and_korean() {
    let library = scripts();
    assert_eq!(found(&library, "หายใจ"), [3]);
    assert_eq!(found(&library, "สแลม"), [3]);
    assert_eq!(found(&library, "คราม"), [3]);
    assert_eq!(found(&library, "사랑"), [9]);
    assert_eq!(found(&library, "사랑했지"), [9]);
    assert_eq!(found(&library, "김광석"), [9]);
    assert_eq!(found(&library, "부르기"), [9]);
    assert_eq!(found(&library, "다시 부르"), [9]);
}

#[test]
fn short_queries_scan_the_folded_columns() {
    let library = scripts();
    assert_eq!(found(&library, "9"), [6]);
    assert_eq!(found(&library, "52"), [10]);
    assert_eq!(found(&library, "jó"), [4]);
    assert_eq!(found(&library, "ö"), found(&library, "o"));
    assert_eq!(found(&library, "ø"), NONE);
    assert_eq!(found(&library, "χ"), [8]);
    // A short term combined with a long one.
    assert_eq!(found(&library, "no 9"), [6]);
    assert_eq!(found(&library, "symphony 8"), NONE);
}

#[test]
fn every_term_must_match() {
    let library = scripts();
    assert_eq!(found(&library, "cafe mar"), [10]);
    assert_eq!(found(&library, "cafe bjork"), NONE);
    assert_eq!(found(&library, "  "), NONE);
    // FTS5 syntax in the query is plain text.
    assert_eq!(found(&library, "title:bjork"), NONE);
    // The word index ignores punctuation, so a stray quote does not matter.
    assert_eq!(found(&library, "\"bjork"), [4]);
    assert_eq!(found(&library, "bjork OR hunter"), NONE);
    assert_eq!(found(&library, "NOT"), NONE);
    assert_eq!(found(&library, "(*)"), NONE);
}

#[test]
fn word_index_covers_genre_composer_album_artist_and_path() {
    let (library, root) = library();
    let mut record = tagged(
        root,
        "/music/Brahms/Op77.flac",
        "x",
        "Violin Concerto",
        "Hilary Hahn",
        "Brahms",
    );
    record.tags.composers = vec!["Johannes Brahms".to_owned()];
    record.tags.album_artists = vec!["Academy of St Martin".to_owned()];
    record.tags.genres = vec!["Classical".to_owned()];
    let track = library.upsert_track(record).unwrap().track;
    for query in ["johannes", "classic", "academy", "op77", "martin"] {
        assert_eq!(found(&library, query), [track.get()], "{query}");
    }
}

#[test]
fn ranking_exact_then_prefix_then_word_then_substring() {
    let (library, root) = library();
    let titles = ["Glove", "My Love", "Lovesong", "Love", "Unrelated"];
    for (n, title) in titles.iter().enumerate() {
        let record = tagged(
            root,
            &format!("/music/{n}.flac"),
            &format!("{n}"),
            title,
            "Someone",
            "Album",
        );
        library.upsert_track(record).unwrap();
    }
    let mut by_artist = tagged(root, "/music/artist.flac", "a", "Zzz", "Love", "Album");
    by_artist.tags.title_sort = Some("Zzz".to_owned());
    library.upsert_track(by_artist).unwrap();

    let results = library.search(&SearchRequest::new("love")).unwrap();
    let ranked: Vec<(Option<&str>, MatchRank)> = results
        .hits
        .iter()
        .map(|hit| (hit.title.as_deref(), hit.rank))
        .collect();
    assert_eq!(
        ranked,
        [
            (Some("Love"), MatchRank::Exact),
            (Some("Zzz"), MatchRank::Exact),
            (Some("Lovesong"), MatchRank::Prefix),
            (Some("My Love"), MatchRank::WordPrefix),
            (Some("Glove"), MatchRank::Substring),
        ]
    );
    assert_eq!(results.total, 5);
    assert_eq!(results.hits[1].artist.as_deref(), Some("Love"));
    assert_eq!(results.hits[0].album.as_deref(), Some("Album"));
}

#[test]
fn paging() {
    let (library, root) = library();
    let records = (0..25)
        .map(|n| {
            tagged(
                root,
                &format!("/music/{n:02}.flac"),
                &format!("{n}"),
                &format!("Track {n:02}"),
                "Band",
                "Album",
            )
        })
        .collect();
    library.upsert_tracks(records).unwrap();
    let page = |offset, limit| {
        library
            .search(&SearchRequest::new("track").page(offset, limit))
            .unwrap()
    };
    let first = page(0, 10);
    assert_eq!(first.total, 25);
    assert_eq!(first.hits.len(), 10);
    assert_eq!(first.hits[0].title.as_deref(), Some("Track 00"));
    let last = page(20, 10);
    assert_eq!(last.total, 25);
    assert_eq!(
        last.hits
            .iter()
            .map(|hit| hit.title.clone().unwrap())
            .collect::<Vec<_>>(),
        ["Track 20", "Track 21", "Track 22", "Track 23", "Track 24"]
    );
    assert!(page(30, 10).hits.is_empty());
    assert_eq!(page(30, 10).total, 25);
}

#[test]
fn index_follows_tag_changes_moves_and_removals() {
    let (library, root) = library();
    let first = library
        .upsert_track(tagged(
            root,
            "/music/a/1.flac",
            "one",
            "Jóga",
            "Björk",
            "Homogenic",
        ))
        .unwrap()
        .track;
    let second = library
        .upsert_track(tagged(
            root,
            "/music/a/2.flac",
            "two",
            "月亮代表我的心",
            "邓丽君",
            "淡淡幽情",
        ))
        .unwrap()
        .track;

    // New tags replace the old ones in both indexes.
    let write = library
        .upsert_track(tagged(
            root,
            "/music/a/1.flac",
            "one",
            "Hunter",
            "Björk",
            "Homogenic",
        ))
        .unwrap();
    assert_eq!(write.track, first);
    assert_eq!(write.change, TrackChange::Updated);
    assert_eq!(found(&library, "joga"), NONE);
    assert_eq!(found(&library, "hunter"), [first.get()]);
    assert_eq!(found(&library, "unte"), [first.get()]);
    assert_eq!(found(&library, "bjork"), [first.get()]);

    // A move changes the path in the word index.
    library
        .move_file(root, "/music/a/1.flac", "/music/b/renamed.flac")
        .unwrap();
    assert_eq!(found(&library, "renamed"), [first.get()]);
    assert_eq!(found(&library, "hunter"), [first.get()]);

    // Removing a track or its file takes it out of both indexes.
    library.remove_track(second).unwrap();
    assert_eq!(found(&library, "代表我的"), NONE);
    assert_eq!(found(&library, "心"), NONE);
    assert_eq!(library.remove_file("/music/b/renamed.flac").unwrap(), 1);
    assert_eq!(found(&library, "hunter"), NONE);
    assert_eq!(found(&library, "unte"), NONE);
    assert_eq!(index_rows(&library), (0, 0));
}

#[test]
fn rebuilding_the_index_finds_the_same_tracks() {
    let library = scripts();
    library.rebuild_search_index().unwrap();
    assert_eq!(index_rows(&library), (TRACKS.len(), TRACKS.len()));
    assert_eq!(found(&library, "我的心"), [1]);
    assert_eq!(found(&library, "bjork"), [4]);
}

/// The number of rows in the word and the trigram index.
fn index_rows(library: &Library) -> (usize, usize) {
    library
        .database()
        .read(|c| {
            let count = |table: &str| {
                c.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
            };
            Ok((count("tracks_fts")? as usize, count("tracks_tri")? as usize))
        })
        .unwrap()
}

#[test]
fn regexp_filters_tracks() {
    let library = scripts();
    let ids = |pattern: &str| {
        library
            .database()
            .read(|c| {
                c.prepare("SELECT id FROM tracks WHERE title REGEXP ?1 ORDER BY id")?
                    .query_map([pattern], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()
            })
            .unwrap()
    };
    assert_eq!(ids(r"^(Jóga|Symphony No\. \d+)$"), [4, 6]);
    assert_eq!(ids(r"\p{Han}{2}"), [1, 2]);
}

#[test]
fn tags_without_title_are_found_by_file_name() {
    let (library, root) = library();
    let mut record = tagged(root, "/music/untitled demo.flac", "u", "", "", "");
    record.tags = TrackTags::default();
    let track = library.upsert_track(record).unwrap().track;
    assert_eq!(found(&library, "untitled"), [track.get()]);
    assert_eq!(found(&library, "demo"), [track.get()]);
}
