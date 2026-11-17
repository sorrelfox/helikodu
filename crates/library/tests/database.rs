// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening, migrating and reopening the database; paths as bytes; the locale collation.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use helikodu_library::raw::Database;
use helikodu_library::raw::rusqlite::{self, Connection, OptionalExtension};
use helikodu_library::{
    DatabaseConfig, DbError, LocaleCollation, SCHEMA_VERSION, display_text, path_from_blob,
    path_to_blob,
};

/// A fresh directory for one test's database files.
fn scratch_dir(test: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "library-db-{test}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn tables(db: &Database) -> Vec<String> {
    db.read(|c| {
        let mut statement =
            c.prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")?;
        statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()
    })
    .unwrap()
}

#[test]
fn migrates_an_empty_database() {
    let db = Database::open(&DatabaseConfig::in_memory()).unwrap();
    assert_eq!(db.schema_version(), SCHEMA_VERSION);
    let tables = tables(&db);
    for table in [
        "schema_migrations",
        "db_meta",
        "roots",
        "dirs",
        "files",
        "identities",
        "tracks",
        "people",
        "track_people",
        "albums",
        "genres",
        "track_genres",
        "track_tags",
        "covers",
        "stats",
        "history",
        "playlists",
        "playlist_items",
        "session",
        "undo_journal",
        "analysis",
        "tracks_fts",
        "tracks_tri",
    ] {
        assert!(tables.iter().any(|t| t == table), "missing table {table}");
    }
    let applied: Vec<(u32, String)> = db
        .read(|c| {
            let mut statement =
                c.prepare("SELECT version, name FROM schema_migrations ORDER BY version")?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .unwrap();
    assert_eq!(applied.len(), SCHEMA_VERSION as usize);
    assert_eq!(applied[0], (1, "library_schema".to_owned()));
}

#[test]
fn reopening_keeps_data_and_does_not_migrate_again() {
    let dir = scratch_dir("reopen");
    let path = dir.join("library.db");
    {
        let db = Database::open(&DatabaseConfig::file(&path)).unwrap();
        db.writer()
            .unwrap()
            .write(|c| {
                c.execute(
                    "INSERT INTO session (key, value) VALUES ('queue', x'0102')",
                    [],
                )
            })
            .unwrap();
        let mode: String = db
            .read(|c| c.query_row("PRAGMA journal_mode", [], |row| row.get(0)))
            .unwrap();
        assert_eq!(mode, "wal");
    }
    let db = Database::open(&DatabaseConfig::file(&path)).unwrap();
    assert_eq!(db.schema_version(), SCHEMA_VERSION);
    let (value, migrations): (Vec<u8>, u32) = db
        .read(|c| {
            Ok((
                c.query_row("SELECT value FROM session WHERE key = 'queue'", [], |row| {
                    row.get(0)
                })?,
                c.query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                    row.get(0)
                })?,
            ))
        })
        .unwrap();
    assert_eq!(value, [1, 2]);
    assert_eq!(migrations, SCHEMA_VERSION);
    drop(db);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn refuses_a_newer_schema_but_opens_it_read_only() {
    let dir = scratch_dir("newer");
    let path = dir.join("library.db");
    drop(Database::open(&DatabaseConfig::file(&path)).unwrap());
    // A later version adds a migration.
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, 'future', 0)",
            [SCHEMA_VERSION + 1],
        )
        .unwrap();
    drop(connection);

    match Database::open(&DatabaseConfig::file(&path)) {
        Err(DbError::NewerSchema { found, supported }) => {
            assert_eq!(found, SCHEMA_VERSION + 1);
            assert_eq!(supported, SCHEMA_VERSION);
        }
        other => panic!("expected NewerSchema, got {other:?}"),
    }

    let db = Database::open_read_only(&DatabaseConfig::file(&path)).unwrap();
    assert_eq!(db.schema_version(), SCHEMA_VERSION + 1);
    assert!(matches!(db.writer(), Err(DbError::ReadOnly)));
    let roots: i64 = db
        .read(|c| c.query_row("SELECT count(*) FROM roots", [], |row| row.get(0)))
        .unwrap();
    assert_eq!(roots, 0);
    // Read connections cannot write, even by mistake.
    let written = db.read(|c| c.execute("DELETE FROM roots", []));
    assert!(written.is_err());
    drop(db);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_failing_migration_leaves_the_database_unchanged() {
    // Simulate a broken migration by making the schema's first table exist already: the
    // first migration then fails, and nothing of it may remain.
    let dir = scratch_dir("failing-migration");
    let path = dir.join("library.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE files (id INTEGER PRIMARY KEY)")
        .unwrap();
    drop(connection);
    match Database::open(&DatabaseConfig::file(&path)) {
        Err(DbError::Migration { version: 1, .. }) => {}
        other => panic!("expected a failed migration, got {other:?}"),
    }
    let connection = Connection::open(&path).unwrap();
    let left: Option<String> = connection
        .query_row(
            "SELECT name FROM sqlite_schema WHERE name IN ('roots', 'db_meta')",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(left, None);
    let recorded: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name = 'schema_migrations'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recorded, 0);
    drop(connection);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_names_that_are_not_utf8_round_trip() {
    let db = Database::open(&DatabaseConfig::in_memory()).unwrap();
    let root = Path::new(OsStr::from_bytes(b"/music/\xc4lteres"));
    let dir = root.join(OsStr::from_bytes(b"Bj\xf6rk"));
    // The same name in Latin-1 and in UTF-8: two different files.
    let latin1 = OsStr::from_bytes(b"J\xf3ga.flac");
    let utf8 = OsStr::new("Jóga.flac");
    let shift_jis = OsStr::from_bytes(b"\x83\x65\x83\x58\x83\x67.mp3");

    let (root_blob, dir_blob) = (path_to_blob(root).to_vec(), path_to_blob(&dir).to_vec());
    let root_text = display_text(root.as_os_str()).into_owned();
    let dir_text = display_text(dir.as_os_str()).into_owned();
    let names: Vec<(Vec<u8>, String)> = [latin1, utf8, shift_jis]
        .iter()
        .map(|name| (name.as_bytes().to_vec(), display_text(name).into_owned()))
        .collect();
    db.writer()
        .unwrap()
        .write(move |c| {
            c.execute(
                "INSERT INTO roots (id, path, path_text) VALUES (1, ?1, ?2)",
                (&root_blob, &root_text),
            )?;
            c.execute(
                "INSERT INTO dirs (id, root_id, path, path_text) VALUES (1, 1, ?1, ?2)",
                (&dir_blob, &dir_text),
            )?;
            for (name, text) in &names {
                c.execute(
                    "INSERT INTO files (dir_id, name, name_text) VALUES (1, ?1, ?2)",
                    (name, text),
                )?;
            }
            Ok(())
        })
        .unwrap();

    let (dir_path, files): (PathBuf, Vec<(PathBuf, String)>) = db
        .read(|c| {
            let dir: Vec<u8> =
                c.query_row("SELECT path FROM dirs WHERE id = 1", [], |row| row.get(0))?;
            let mut statement =
                c.prepare("SELECT name, name_text FROM files WHERE dir_id = 1 ORDER BY id")?;
            let files = statement
                .query_map([], |row| Ok((path_from_blob(row.get(0)?), row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((path_from_blob(dir), files))
        })
        .unwrap();
    assert_eq!(dir_path, dir);
    assert_eq!(files.len(), 3);
    assert_eq!(files[0].0.as_os_str(), latin1);
    assert_eq!(files[0].1, "J\u{FFFD}ga.flac");
    assert_eq!(files[1].0.as_os_str(), utf8);
    assert_eq!(files[1].1, "Jóga.flac");
    assert_eq!(files[2].0.as_os_str(), shift_jis);

    // A lookup by bytes finds exactly the Latin-1 file.
    let found: i64 = db
        .read(|c| {
            c.query_row(
                "SELECT count(*) FROM files WHERE dir_id = 1 AND name = ?1",
                [latin1.as_bytes()],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(found, 1);
}

#[test]
fn path_columns_refuse_text() {
    let db = Database::open(&DatabaseConfig::in_memory()).unwrap();
    let result = db.writer().unwrap().write(|c| {
        c.execute(
            "INSERT INTO roots (path, path_text) VALUES ('/music', '/music')",
            [],
        )
    });
    assert!(result.is_err(), "a STRICT BLOB column accepted text");
}

/// Inserts one track per title, with `title_sort` set to the title.
fn insert_titles(db: &Database, titles: &[&str]) {
    let titles: Vec<String> = titles.iter().map(|&t| t.to_owned()).collect();
    db.writer()
        .unwrap()
        .write(move |c| {
            c.execute_batch(
                "INSERT INTO roots (id, path, path_text) VALUES (1, x'2f', '/');
                 INSERT INTO dirs (id, root_id, path, path_text) VALUES (1, 1, x'2f', '/');",
            )?;
            for (number, title) in (1_i64..).zip(&titles) {
                c.execute(
                    "INSERT INTO files (id, dir_id, name, name_text) VALUES (?1, 1, ?2, ?3)",
                    (number, title.as_bytes(), title),
                )?;
                c.execute(
                    "INSERT INTO identities (id, created_at) VALUES (?1, 0)",
                    [number],
                )?;
                c.execute(
                    "INSERT INTO tracks (file_id, identity_id, title, title_sort, title_fold,
                                         added_at)
                     VALUES (?1, ?1, ?2, ?2, hk_fold(?2), 0)",
                    (number, title),
                )?;
            }
            Ok(())
        })
        .unwrap();
}

fn titles_in_order(db: &Database) -> Vec<String> {
    db.read(|c| {
        let mut statement = c.prepare("SELECT title FROM tracks ORDER BY title_sort")?;
        statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()
    })
    .unwrap()
}

const MIXED: &[&str] = &[
    "Zeppelin", "Öresund", "Åland", "Ärzte", "apple", "Apfel", "šokk", "zoo", "žanr", "tuli",
    "Õhtu", "Über",
];

#[test]
fn sorts_in_the_locale_order() {
    let open = |locale: &str| {
        let config =
            DatabaseConfig::in_memory().with_collation(LocaleCollation::new(locale).unwrap());
        let db = Database::open(&config).unwrap();
        insert_titles(&db, MIXED);
        titles_in_order(&db)
    };
    assert_eq!(
        open("en"),
        [
            "Åland", "Apfel", "apple", "Ärzte", "Õhtu", "Öresund", "šokk", "tuli", "Über", "žanr",
            "Zeppelin", "zoo"
        ]
    );
    // Å, Ä and Ö are letters after Z; Õ and Š are accented O and S; Ü sorts as Y.
    assert_eq!(
        open("sv"),
        [
            "Apfel", "apple", "Õhtu", "šokk", "tuli", "Über", "žanr", "Zeppelin", "zoo", "Åland",
            "Ärzte", "Öresund"
        ]
    );
    // Umlauts are accented letters, as in the root order.
    assert_eq!(
        open("de"),
        [
            "Åland", "Apfel", "apple", "Ärzte", "Õhtu", "Öresund", "šokk", "tuli", "Über", "žanr",
            "Zeppelin", "zoo"
        ]
    );
    // … s š z ž t u v w õ ä ö ü x y; Å is an accented A.
    assert_eq!(
        open("et"),
        [
            "Åland", "Apfel", "apple", "šokk", "Zeppelin", "zoo", "žanr", "tuli", "Õhtu", "Ärzte",
            "Öresund", "Über"
        ]
    );
}

#[test]
fn the_sort_index_uses_the_collation() {
    let db = Database::open(&DatabaseConfig::in_memory()).unwrap();
    let plan: Vec<String> = db
        .read(|c| {
            let mut statement =
                c.prepare("EXPLAIN QUERY PLAN SELECT title FROM tracks ORDER BY title_sort")?;
            statement
                .query_map([], |row| row.get(3))?
                .collect::<rusqlite::Result<Vec<String>>>()
        })
        .unwrap();
    assert!(
        plan.iter().any(|step| step.contains("tracks_title_sort")),
        "{plan:?}"
    );
}

#[test]
fn changing_the_locale_rebuilds_the_sort_indexes() {
    let dir = scratch_dir("locale");
    let path = dir.join("library.db");
    let swedish = DatabaseConfig::file(&path).with_collation(LocaleCollation::new("sv").unwrap());
    let db = Database::open(&swedish).unwrap();
    insert_titles(&db, &["Zeppelin", "Åland", "Apfel"]);
    assert_eq!(titles_in_order(&db), ["Apfel", "Zeppelin", "Åland"]);

    db.set_collation(LocaleCollation::new("en").unwrap())
        .unwrap();
    assert_eq!(titles_in_order(&db), ["Åland", "Apfel", "Zeppelin"]);
    let integrity: String = db
        .read(|c| c.query_row("PRAGMA integrity_check", [], |row| row.get(0)))
        .unwrap();
    assert_eq!(integrity, "ok");
    drop(db);

    // Opening with another locale rebuilds the indexes too.
    let db = Database::open(&swedish).unwrap();
    assert_eq!(titles_in_order(&db), ["Apfel", "Zeppelin", "Åland"]);
    let (locale, integrity): (String, String) = db
        .read(|c| {
            Ok((
                c.query_row(
                    "SELECT value FROM db_meta WHERE key = 'collation_locale'",
                    [],
                    |row| row.get(0),
                )?,
                c.query_row("PRAGMA integrity_check", [], |row| row.get(0))?,
            ))
        })
        .unwrap();
    assert_eq!(locale, "sv");
    assert_eq!(integrity, "ok");
    drop(db);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn readers_share_the_pool_across_threads() {
    let mut config = DatabaseConfig::in_memory();
    config.read_connections = 2;
    let db = Database::open(&config).unwrap();
    insert_titles(&db, MIXED);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..50 {
                    assert_eq!(titles_in_order(&db).len(), MIXED.len());
                }
            });
        }
    });
}
