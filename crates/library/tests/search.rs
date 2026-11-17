// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The search indexes: whole words, trigram substrings across scripts, and `regexp()`.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

use helikodu_library::rusqlite;
use helikodu_library::{Database, DatabaseConfig, fold};

/// `(title, artist, album)` of each test track; the track ids are 1, 2, ….
const TRACKS: &[(&str, &str, &str)] = &[
    ("月亮代表我的心", "邓丽君", "淡淡幽情"),
    ("千と千尋の神隠し", "久石譲", "ジブリの歌"),
    ("ลมหายใจ", "บอดี้สแลม", "คราม"),
    ("Jóga", "Björk", "Homogenic"),
    ("Straße der Lieder", "Die Ärzte", "Geräusch"),
    ("Symphony No. 9", "Антонин Дворжак", "Новый Свет"),
    ("夜に駆ける", "YOASOBI", "THE BOOK"),
];

/// A database with the test tracks in `tracks` and in both search indexes.
fn library() -> Database {
    let db = Database::open(&DatabaseConfig::in_memory()).unwrap();
    db.writer()
        .unwrap()
        .write(|c| {
            c.execute_batch(
                "INSERT INTO roots (id, path, path_text) VALUES (1, x'2f', '/');
                 INSERT INTO dirs (id, root_id, path, path_text) VALUES (1, 1, x'2f', '/');",
            )?;
            for (id, (title, artist, album)) in (1_i64..).zip(TRACKS) {
                c.execute(
                    "INSERT INTO files (id, dir_id, name, name_text) VALUES (?1, 1, ?2, ?3)",
                    (id, format!("{id}.flac").into_bytes(), format!("{id}.flac")),
                )?;
                c.execute(
                    "INSERT INTO identities (id, created_at) VALUES (?1, 0)",
                    [id],
                )?;
                c.execute(
                    "INSERT INTO tracks (id, file_id, identity_id, title, title_sort,
                                         title_fold, added_at)
                     VALUES (?1, ?1, ?1, ?2, ?2, hk_fold(?2), 0)",
                    (id, title),
                )?;
                index(c, id, title, artist, album)?;
            }
            Ok(())
        })
        .unwrap();
    db
}

fn index(
    c: &rusqlite::Connection,
    id: i64,
    title: &str,
    artist: &str,
    album: &str,
) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO tracks_fts (rowid, title, artists, album)
         VALUES (?1, hk_fold(?2), hk_fold(?3), hk_fold(?4))",
        (id, title, artist, album),
    )?;
    c.execute(
        "INSERT INTO tracks_tri (rowid, title, artists, album)
         VALUES (?1, hk_fold(?2), hk_fold(?3), hk_fold(?4))",
        (id, title, artist, album),
    )?;
    Ok(())
}

fn ids(db: &Database, sql: &str, param: &str) -> Vec<i64> {
    db.read(|c| {
        let mut statement = c.prepare(sql)?;
        statement
            .query_map([param], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()
    })
    .unwrap()
}

/// An FTS5 string literal: the text in double quotes, quotes doubled.
fn phrase(text: &str) -> String {
    format!("\"{}\"", fold(text).replace('"', "\"\""))
}

/// Substring search: the trigram index from three characters, a scan of the folded
/// columns below that.
fn substring(db: &Database, query: &str) -> Vec<i64> {
    if query.chars().count() >= 3 {
        ids(
            db,
            "SELECT rowid FROM tracks_tri WHERE tracks_tri MATCH ?1 ORDER BY rowid",
            &phrase(query),
        )
    } else {
        ids(
            db,
            "SELECT id FROM tracks WHERE instr(title_fold, ?1) > 0 ORDER BY id",
            &fold(query),
        )
    }
}

fn words(db: &Database, query: &str) -> Vec<i64> {
    ids(
        db,
        "SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ?1 ORDER BY rowid",
        query,
    )
}

#[test]
fn trigram_finds_substrings_in_chinese_japanese_and_thai() {
    let db = library();
    // Chinese: four, three, two and one characters.
    assert_eq!(substring(&db, "代表我的"), [1]);
    assert_eq!(substring(&db, "我的心"), [1]);
    assert_eq!(substring(&db, "亮代"), [1]);
    assert_eq!(substring(&db, "心"), [1]);
    // Japanese, inside a title and in the artist and album columns.
    assert_eq!(substring(&db, "千尋の"), [2]);
    assert_eq!(substring(&db, "神隠し"), [2]);
    assert_eq!(substring(&db, "駆ける"), [7]);
    assert_eq!(substring(&db, "の"), [2]);
    assert_eq!(substring(&db, "久石譲"), [2]);
    assert_eq!(substring(&db, "ジブリ"), [2]);
    // Half-width katakana in the query folds to the indexed full-width form.
    assert_eq!(substring(&db, "ｼﾞﾌﾞﾘ"), [2]);
    // Thai, written without spaces between words.
    assert_eq!(substring(&db, "หายใจ"), [3]);
    assert_eq!(substring(&db, "สแลม"), [3]);
    // Latin text, case- and accent-insensitive.
    assert_eq!(substring(&db, "JORK"), [4]);
    assert_eq!(substring(&db, "aerzte"), Vec::<i64>::new());
    assert_eq!(substring(&db, "ARZT"), [5]);
    assert_eq!(substring(&db, "дворж"), [6]);
}

#[test]
fn word_index_matches_words_and_prefixes_without_accents() {
    let db = library();
    assert_eq!(words(&db, "bjork"), [4]);
    assert_eq!(words(&db, &phrase("Björk")), [4]);
    assert_eq!(words(&db, "homo*"), [4]);
    assert_eq!(words(&db, "strasse"), [5]);
    assert_eq!(words(&db, "artists:arzte"), [5]);
    assert_eq!(words(&db, "title:arzte"), Vec::<i64>::new());
    assert_eq!(words(&db, "дворжак"), [6]);
    assert_eq!(words(&db, "symphony AND 9"), [6]);
}

#[test]
fn index_rows_can_be_replaced_and_deleted() {
    let db = library();
    db.writer()
        .unwrap()
        .write(|c| {
            // The tags of track 4 change.
            c.execute("DELETE FROM tracks_fts WHERE rowid = 4", [])?;
            c.execute("DELETE FROM tracks_tri WHERE rowid = 4", [])?;
            index(c, 4, "Hunter", "Björk", "Homogenic")?;
            // Track 1 leaves the library.
            c.execute("DELETE FROM tracks_fts WHERE rowid = 1", [])?;
            c.execute("DELETE FROM tracks_tri WHERE rowid = 1", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(words(&db, "joga"), Vec::<i64>::new());
    assert_eq!(words(&db, "hunter"), [4]);
    assert_eq!(substring(&db, "unte"), [4]);
    assert_eq!(substring(&db, "代表我的"), Vec::<i64>::new());
}

#[test]
fn regexp_filters_tracks() {
    let db = library();
    assert_eq!(
        ids(
            &db,
            "SELECT id FROM tracks WHERE title REGEXP ?1 ORDER BY id",
            r"^(Jóga|Symphony No\. \d+)$"
        ),
        [4, 6]
    );
    assert_eq!(
        ids(
            &db,
            "SELECT id FROM tracks WHERE title REGEXP ?1 ORDER BY id",
            r"\p{Han}{2}"
        ),
        [1, 2]
    );
}
