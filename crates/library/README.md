# helikodu-library

The SQLite schema and its migrations, the scan coordinator and file watcher, the album and artist model, search, the query language and smart playlists, play statistics and the undo journal.

- **May depend on:** `model`, `meta`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## The database

`src/db/` holds the connections, the migration runner, the collation and the SQL functions.

- **One writer.** A thread named `helikodu-db-writer` owns the only writing connection. `Writer::submit` queues a write job and returns a ticket; `Writer::write` waits for it. The thread takes the first queued job and every job already waiting, up to `max_batch` (512), and runs them in one transaction, each inside its own savepoint. A job that fails is rolled back alone; the others still commit. A result is delivered only after its batch has committed. If the commit fails, the whole batch is rolled back and every job of it gets `DbError::BatchFailed`. If a job panics, the open transaction is rolled back and the writer stops. The queue is bounded (4096 jobs), so a fast producer slows down to the speed of the disk.
- **Readers.** A pool of read-only connections (4 by default, opened with `SQLITE_OPEN_READ_ONLY` and `query_only`). `Database::read` runs a closure inside a read transaction, so all its queries see one snapshot.
- **Pragmas.** The writer sets `journal_mode=WAL`, `synchronous=FULL` (a committed batch survives a power loss; writes are batched, so the extra sync is cheap), `foreign_keys=ON`, a 5 s busy timeout, `temp_store=MEMORY`, a 16 MiB page cache and a 64 MiB journal size limit. The writer runs `PRAGMA optimize` when it stops.
- **In memory.** `DatabaseConfig::in_memory()` uses SQLite's `memdb` VFS, so the writer and the readers share one database in memory. This is for tests.
- **Shutdown.** Dropping the `Database` commits the writes already queued and stops the thread. `Writer` handles that outlive it get `DbError::WriterStopped`.

### Migrations

`migrations/` holds one numbered SQL file per schema change, and `MIGRATIONS` in `src/db/migrations.rs` lists them. A migration that has been released is never edited; a change is a new file. A unit test checks that every file is listed once and in order.

- The `schema_migrations` table records the applied migrations. The schema version is the highest one.
- Each migration runs in its own `BEGIN IMMEDIATE` transaction together with its record, so it is applied completely or not at all. Foreign keys are off while it runs, so that a table can be rebuilt, and `PRAGMA foreign_key_check` must pass before it commits.
- `Database::open` refuses a database with a newer schema (`DbError::NewerSchema`). `Database::open_read_only` opens such a database without migrating it; writes then fail with `DbError::ReadOnly`.

### Schema

Version 1 (`0001_library_schema.sql`, `0002_search_index.sql`):

| Area | Tables |
|---|---|
| Files | `roots`, `dirs`, `files` |
| Tracks and identity | `identities`, `tracks`, `track_tags` |
| Catalogue | `albums`, `people`, `track_people`, `genres`, `track_genres`, `covers` |
| Use | `stats`, `history`, `analysis` |
| Playlists and state | `playlists`, `playlist_items`, `session`, `undo_journal`, `db_meta` |
| Search | `tracks_fts` (words), `tracks_tri` (trigrams) |

- **Paths are bytes.** `roots.path`, `dirs.path`, `files.name`, `covers.source_path` and `playlist_items.missing_ref` are BLOBs holding the bytes the kernel returned. `path_to_blob` and `path_from_blob` convert between them and `Path` without going through `String`. The `*_text` columns beside them hold `display_text`, which is for display and search only. Uniqueness and lookups use the bytes. Every table is `STRICT`, so a path stored as TEXT by mistake is an error.
- **Statistics** hang off `identities`, not off paths, so they survive moves. `stats.rating` holds half stars from 0 to 10; `NULL` means not rated.
- **Folded columns** (`title_fold`, `people.fold`, `genres.fold`) hold `hk_fold()` of the text.
- **Sort columns** (`tracks.title_sort`, `albums.title_sort`, `people.sort_name`) are declared `COLLATE HK_LOCALE`, and `genres` has an index on `name COLLATE HK_LOCALE`. No unique constraint uses the collation, so rebuilding the indexes for another locale cannot fail.
- **Search tables** are contentless with `contentless_delete=1`, so a track's row can be deleted and inserted again when its tags change. Their rowid is `tracks.id`, and they are filled with folded text.

### SQL functions and the collation

Every connection registers:

| Name | What it does |
|---|---|
| `HK_LOCALE` (collation) | The alphabet order of the interface language, from ICU4X (`icu_collator`). `LocaleCollation::new` takes a BCP 47 tag (`sv-SE`) or a POSIX locale name (`sv_SE.UTF-8`); `C` and `POSIX` select the root order. |
| `regexp(pattern, value)` | `value REGEXP pattern` with the `regex` crate. NULL if either side is NULL. The compiled pattern is cached per statement, and its size is limited to 1 MiB because patterns come from user queries. |
| `hk_fold(text)` | The folded form: NFKD, lower case, Latin and Greek diacritics removed, `ß → ss`, `ё → е`, `ς → σ`, then NFC. Marks that belong to the letters of other scripts (Thai vowels, kana voicing marks, Devanagari) are kept. The same function is available in Rust as `fold`. |

`db_meta` records the locale the `HK_LOCALE` indexes were built for. `Database::open` with another locale, and `Database::set_collation`, run `REINDEX HK_LOCALE`. Reader connections take the new order the next time they are lent out.

### Substring search across scripts

`tracks_fts` (tokenizer `unicode61 remove_diacritics 2`) matches whole words and word prefixes. `tracks_tri` (tokenizer `trigram`) holds the folded title, artists and album and answers substring queries of three characters or more in any script, including Chinese, Japanese and Thai. A shorter query scans the folded columns instead (`instr(title_fold, …)`). `tests/search.rs` shows both.

## SQLite: bundled or system

The crate needs SQLite 3.43 or newer, built with FTS5 (contentless-delete tables and the trigram tokenizer). It checks the version when it opens a connection and FTS5 when it opens the writer.

- **`bundled-sqlite` feature, off by default:** builds the SQLite that `rusqlite` bundles (3.54.1 with `rusqlite` 0.43; FTS5 and the trigram tokenizer are compiled in). CI builds with `--all-features`, so it uses this copy, and so does the Flatpak.
- **Without the feature:** links the system `libsqlite3` through pkg-config. This is what distributions use. Debian 13 ships 3.46 with FTS5.

## Performance

`benches/collation.rs` measures the collation and folding over generated titles that mix Latin with diacritics, Greek, Cyrillic, Chinese, Japanese and Thai, one to five words each. On a current desktop CPU (Zen 5, one thread), with the system SQLite 3.53:

| Measurement | 100 000 titles | 500 000 titles |
|---|---|---|
| Sort in Rust, code-point order | 7 ms (5 ns per comparison) | 43 ms |
| Sort in Rust, root collation | 93 ms (59 ns per comparison) | 510 ms |
| Sort in Rust, Swedish collation | 105 ms (67 ns per comparison) | 558 ms |
| `fold` of every title | 37 ms (373 ns per title) | 189 ms |
| Insert the tracks, with the collation index | 561 ms (5.6 µs per track) | 3.2 s |
| Switch locale (`REINDEX HK_LOCALE`) | 216 ms | 1.2 s |
| A page of 50 rows at the middle, through the index | 0.8 ms | 3.8 ms |
| All rows in order, through the index | 31 ms | 191 ms |
| All rows in order, sorted without an index | 219 ms | 1.3 s |

A collation comparison costs about 12 times a byte comparison. Through the index, ordered reads cost the same as code-point order, because the index is already sorted; the collation's cost falls on writes and on a locale switch. Creating a collator takes well under a millisecond.

The ICU4X data is compiled into the binary. In a stripped release build, `icu_collator` with the collation data of every locale adds about 1.3 MB, and `icu_normalizer` (which it needs anyway, and which `fold` uses) about 0.14 MB.

```sh
cargo bench -p helikodu-library --bench collation            # 100 000 titles
cargo bench -p helikodu-library --bench collation -- 500000
```

## Tests

```sh
cargo test -p helikodu-library
cargo test -p helikodu-library --features bundled-sqlite
```

- `tests/database.rs`: migrating an empty database, reopening, refusing a newer schema and opening it read-only, a failing migration that leaves nothing behind, file names that are not UTF-8, the locale order in English, Swedish, German and Estonian, and the rebuild of the sort indexes when the locale changes.
- `tests/search.rs`: trigram substring search in Chinese, Japanese and Thai with queries of one to four characters, word search without accents, replacing and deleting index rows, and `regexp()`.
- `tests/writer.rs`: batching, a failed job rolled back alone, a batch that cannot commit rolled back whole, a panicking job, and the writes queued at shutdown.
