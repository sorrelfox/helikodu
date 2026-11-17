# helikodu-library

The SQLite schema and its migrations, the scan coordinator and file watcher, the album and artist model, search, the query language and smart playlists, play statistics and the undo journal.

- **May depend on:** `model`, `meta`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## The public API

`Library` is the entry point. It opens the database (`Library::open`, `Library::open_read_only`) and offers typed operations; no SQLite type appears in them.

| Area | Operations |
|---|---|
| Catalogue | `add_root`, `upsert_track` / `upsert_tracks` (a `TrackRecord`: path, `FileStamp`, sub-track, `AudioProperties`, `TrackTags`, a rating from the tags), `move_file`, `mark_file_missing`, `remove_file`, `remove_track`, `track_at`, `identity_of`, `identities_with` |
| Statistics | `record_listen` (a `Listen` judged by `PlayRules`), `stats`, `set_rating` (`Option<Rating>`), `set_resume_position`, `history` (`HistoryQuery`) |
| Search | `search` (`SearchRequest` → `SearchResults`), `rebuild_search_index` |
| Scanning | `scan` (`ScanOptions`, an optional `Sender<ScanEvent>`, a `CancelToken` → `ScanSummary`), `unreadable_files`; `ScanHandle::spawn` runs a scan on its own thread |

Errors are `DbError`; an error from SQLite is the opaque `SqliteError`. A scan fails with `ScanError` only when the database fails or no worker can be started; a file that cannot be read is an event, not an error.

The module `raw` gives direct SQL access below `Library`: `raw::Database` with its read and write callbacks, the `Writer`, and the `rusqlite` bindings. It exists for this crate's tests and benchmarks and for diagnostics, and it is not a stable interface. Other crates use `Library`.

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

Version 4 (`0001_library_schema.sql`, `0002_search_index.sql`, `0003_identity_fingerprint.sql`, `0004_search_support.sql`):

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
- **Identity fingerprints.** `identities.content_hash` (the file's quick hash) and `identities.audio_key` (sub-track, frame range, duration, sample rate, channels) find an identity again when its track comes back.
- **`tracks.artists_fold`** holds the folded artist and album artist names, separated by U+001F, for short-query scans and ranking. The library writes it with `track_people`.
- **Folded columns** (`title_fold`, `people.fold`, `genres.fold`) hold `hk_fold()` of the text.
- **Sort columns** (`tracks.title_sort`, `albums.title_sort`, `people.sort_name`) are declared `COLLATE HK_LOCALE`, and `genres` has an index on `name COLLATE HK_LOCALE`. No unique constraint uses the collation, so rebuilding the indexes for another locale cannot fail.
- **Search tables** are contentless with `contentless_delete=1`, so a track's row can be deleted and inserted again when its tags change. Their rowid is `tracks.id`, and they are filled with folded text. The library refills a track's rows whenever it writes the track or moves its file; a trigger removes them when the track row is deleted, including by `ON DELETE CASCADE`.

### SQL functions and the collation

Every connection registers:

| Name | What it does |
|---|---|
| `HK_LOCALE` (collation) | The alphabet order of the interface language, from ICU4X (`icu_collator`). `LocaleCollation::new` takes a BCP 47 tag (`sv-SE`) or a POSIX locale name (`sv_SE.UTF-8`); `C` and `POSIX` select the root order. |
| `regexp(pattern, value)` | `value REGEXP pattern` with the `regex` crate. NULL if either side is NULL. The compiled pattern is cached per statement, and its size is limited to 1 MiB because patterns come from user queries. |
| `hk_match_rank(field, query)` | How a folded field matches a folded query, 0 (equal) to 4 (not as a whole); a field with several values separated by U+001F ranks as its best one. Used to order search results. |
| `hk_fold(text)` | The folded form: NFKD, lower case, Latin and Greek diacritics removed, `ß → ss`, `ё → е`, `ς → σ`, then NFC. Marks that belong to the letters of other scripts (Thai vowels, kana voicing marks, Devanagari) are kept. The same function is available in Rust as `fold`. |

`db_meta` records the locale the `HK_LOCALE` indexes were built for. `Database::open` with another locale, and `Database::set_collation`, run `REINDEX HK_LOCALE`. Reader connections take the new order the next time they are lent out.

### Track identity and moves

A track's statistics, rating and history belong to its identity (`identities`), not to its path.

- A file that is renamed while watched is moved with `move_file`: its row changes, so the tracks keep their ids and identities.
- A file marked missing (`mark_file_missing`) that turns up elsewhere with the same size and quick hash is moved there by `upsert_track` (`TrackChange::Moved`).
- A track that left the library (`remove_file`, `remove_track`) leaves its identity behind. A new track with the same fingerprint gets it back (`TrackChange::Returned`), with its statistics.
- A re-encoded or re-ripped file has another fingerprint and gets a new identity.
- The tracks of a CUE sheet or the subsongs of a file each have their own identity.
- The rating found in a file's tags is taken only when a new identity is created. From then on the database wins, also over a cleared rating.

### Statistics rules

`PlayRules` decides what a listen counts as. Its functions are pure and tested on their own.

- A **play** when the time actually played (seeks excluded) reaches the threshold: half the track or 4 minutes, whichever comes first; without a known length, 4 minutes. Both numbers are configurable. A play raises `play_count` and sets `first_played` and `last_played` (the start of the listen).
- A **skip** when the user moved to another track before the threshold. It raises `skip_count` and sets `last_skipped`.
- Anything else (stopped, or finished after seeking past most of it) counts as neither.
- Every listen that played some audio is appended to `history`, with `counted` telling whether it was a play.
- Audiobooks and tracks over 20 minutes remember where listening stopped (`resume_ms`) and forget it when they finish.

### Search

`Library::search` folds the text (`fold`), splits it at white space and returns the tracks that match every term, one page at a time, with the total count.

- A term of three characters or more is looked up in `tracks_tri` (a substring of the title, artists or album in any script, including Chinese, Japanese, Korean and Thai) and in `tracks_fts` (a word prefix in any indexed column: also album artist, genre, composer and path). FTS5 syntax in the query is plain text.
- A term of one or two characters is too short for trigrams. It is found by scanning the folded title, artists, album and genre columns with `instr()`.
- Results are ranked by how the whole query matches the title, an artist or the album: exactly, as a prefix, at the start of a word, inside a word, and last the tracks that only match term by term (`MatchRank`). Within a rank they are in the locale order of `title_sort`.
- The query language is not part of this yet; it will compile to the same indexes.

## The scanner

`src/scan/` walks the library folders, compares them with the database, has new and changed files read by `helikodu-scan-worker` processes, and writes what they read.

- **Walk** (`walk.rs`). Paths are bytes from start to end. Entries are taken in byte order. Left out: names starting with a dot (unless `include_hidden`), `ExcludeRule::Path` (a folder or file) and `ExcludeRule::Name` (a pattern with `*` and `?`, matched against every name), folders holding the ignore marker (`.nomedia` by default), files that are neither audio nor CUE sheets by their extension, and video files unless `include_video` (their tracks get the kind `AudioFromVideo`). `.m4b` files are audiobooks.
- **Symbolic links** (`SymlinkPolicy`): `WithinRoots` (the default) follows a link whose target is inside a library folder, `Follow` every link, `Ignore` none. Real folders are walked before links, and every folder and file is taken once by its device and inode, so a link loop ends at once and a file reachable through a link (or a hard link) is recorded under one path.
- **Compare** (`compare.rs`). A file is read when it is new, when its size, modification time (ns) or inode changed, or when a CUE sheet in its folder appeared, changed or went away. A file that disappeared is marked missing (`missing_since`); its tracks stay until the grace period ends, which the watcher handles. A file that comes back is unmarked; a missing file that turns up under another path with the same size and quick hash is moved there with its tracks. A library folder that cannot be read (not mounted, gone) marks nothing missing, and neither does a subfolder that cannot be read.
- **Read** (`pool.rs`). A pool of worker processes (one per processor, at most 8, by default) reads one file per worker at a time. Each worker limits its address space (1 GiB by default) and turns core dumps off. A worker that does not answer within the time limit (30 s by default) is killed; one that dies (a crash, or the memory limit) is noticed when its output ends. Either way the file is recorded as unreadable with the reason (`files.scan_state` 1, `scan_error`), a `ScanEvent::FileFailed` is sent, and the next file goes to a new worker. An unreadable file is not read again until it changes, unless `retry_unreadable` is set. The worker program is `WorkerOptions::program`, or `helikodu-scan-worker` beside the running program or in `../libexec/helikodu/` from it.
- **Records** (`records.rs`). A file described by a CUE sheet becomes one track per sheet track (`sub_index` is the track number, the frame range at the file's sample rate); otherwise it is one whole-file track. A sheet track that starts after the end of the file is left out. Chapters are read by the worker but not stored yet. CUE sheets are recorded as file rows with `scan_state` 2 and no tracks, so that a changed sheet is noticed.
- **Write** (`store.rs`). Each file is one write job: its tracks through the same upsert as `upsert_track`, then the removal of the file's tracks that are no longer in it (a sheet replaced the whole-file track, or lost tracks). The jobs go to the writer without waiting, so it commits them in batches of up to 512; one job that fails is rolled back alone.
- **Events** (`ScanEvent`): `Walking`, `RootUnavailable`, `Compared` (found, to read, missing), `Progress` (at most every 100 ms), `FileFailed`, `Finished` with the `ScanSummary`. `CancelToken::cancel` stops a scan within a fraction of a second: the workers are killed and what was read is kept.

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

`benches/search.rs` measures `Library::search` (the first page of 50 and the total count) over a generated library in memory: titles, artists and albums of one to four words in Latin with diacritics, Cyrillic, Greek, Chinese, Japanese, Thai and Korean, 20 tracks per artist, 10 per album, five genres.

Median of seven runs, Zen 5, one thread, bundled SQLite 3.54:

| Query | 100 000 tracks: matches | time | 500 000 tracks: matches | time |
|---|---|---|---|---|
| `love` (trigram and word index) | 13 912 | 11 ms | 69 136 | 53 ms |
| `我的心` (trigram) | 8 976 | 8 ms | 46 162 | 38 ms |
| `LOVE song`, `night rain` (two long terms) | 1 906, 2 355 | 7–8 ms | | |
| `lo`, `мо`, `αγ` (two characters, scan) | 13 912–14 365 | 24 ms | 69 136 | 117 ms |
| `心`, `사랑`, `代表` (one or two CJK characters, scan) | 8 967–9 258 | 20–22 ms | 45 816–46 162 | 101–108 ms |
| `9` (scan, no match) | 0 | 15 ms | 0 | 79 ms |
| `a`, `o`, `e` (one letter, nearly every track) | 86 472–87 876 | 59–65 ms | 437 416 | 325 ms |
| `a b c` (three short terms) | 15 304 | 47 ms | 76 425 | 240 ms |

Writing 100 000 tracks with both indexes through `upsert_tracks` takes 15 s (154 µs per track, including people, albums and genres); rebuilding both indexes takes 0.44 s at 100 000 tracks and 2.6 s at 500 000. Every query meets the 100 ms budget at 100 000 tracks. At 500 000, queries of three characters or more stay well inside it, while one- and two-character queries, which scan, take up to a third of a second.

A collation comparison costs about 12 times a byte comparison. Through the index, ordered reads cost the same as code-point order, because the index is already sorted; the collation's cost falls on writes and on a locale switch. Creating a collator takes well under a millisecond.

The ICU4X data is compiled into the binary. In a stripped release build, `icu_collator` with the collation data of every locale adds about 1.3 MB, and `icu_normalizer` (which it needs anyway, and which `fold` uses) about 0.14 MB.

```sh
cargo bench -p helikodu-library --bench collation            # 100 000 titles
cargo bench -p helikodu-library --bench collation -- 500000
cargo bench -p helikodu-library --bench search               # 100 000 tracks, every query
cargo bench -p helikodu-library --bench search -- 500000 a 我的心   # 500 000 tracks, chosen queries
```

`benches/scan.rs` measures a full scan of a library made by `cargo xtask synth-library` (one-second FLAC files in artist, album and disc folders, about 2.5 % of them with names that are not UTF-8), into a database file, with release builds of the library and the worker. It generates the library under `target/scan-bench/` and deletes it afterwards.

| 100 000 tracks, Zen 5, NVMe SSD, warm cache | Time | Files per second |
|---|---|---|
| First scan, 8 workers (the default on 32 threads) | 27.8 s | 3 600 |
| First scan, 2 or 16 workers | 26.9–27.4 s | 3 650–3 700 |
| Rescan without changes | 0.25 s | |

The budget is 500 files per second (100 000 files in under 4 minutes). The writer is the limit, not the workers: the time hardly changes between 2 and 16 workers.

```sh
cargo bench -p helikodu-library --bench scan                   # 100 000 tracks
cargo bench -p helikodu-library --bench scan -- 10k --workers 4 --keep
```

## Tests

```sh
cargo test -p helikodu-library
cargo test -p helikodu-library --features bundled-sqlite
```

- `tests/database.rs`: migrating an empty database, reopening, refusing a newer schema and opening it read-only, a failing migration that leaves nothing behind, file names that are not UTF-8, the locale order in English, Swedish, German and Estonian, and the rebuild of the sort indexes when the locale changes.
- `tests/search.rs`: search through `Library` in Latin with diacritics, Cyrillic, Greek, Chinese, Japanese (kana and kanji), Thai and Korean, with queries of one to four characters; short queries; every term must match; FTS5 syntax as plain text; genre, composer, album artist and path through the word index; ranking and paging; the indexes after tag changes, moves and removals, and after a rebuild; `regexp()`.
- `tests/stats.rs`: plays, skips and partial listens; ratings from 0 to 10 and not rated; the rating from the tags taken once; statistics after a watched move, after a file that disappears and reappears elsewhere, and after a track that leaves and comes back; a re-encoded file; CUE tracks; the history and its filters; resume positions.
- `tests/scan.rs`: a small library made with the `ffmpeg` tool and scanned by real worker processes: a folder and a file name in Latin-1 bytes, a CUE album, an excluded folder, a hidden folder, a link loop, a picture and a fake video; rescans after a tag change, a removal, a move into a new folder, an addition, a file that comes back and a sheet that appears; a library folder that disappeared; a missing worker program. In debug builds, files that make the worker crash, hang or exhaust its memory limit (see `apps/scan-worker`) are reported and recorded while the scan goes on, are not read again until asked, and a scan stuck on hanging files is cancelled. The tests build `helikodu-scan-worker` themselves; without `ffmpeg` they print `SKIPPED:` and pass.
- `tests/writer.rs`: batching, a failed job rolled back alone, a batch that cannot commit rolled back whole, a panicking job, and the writes queued at shutdown.
