# helikodu-library

The SQLite schema and its migrations, the scan coordinator and file watcher, the album and artist model, search, the query language and smart playlists, play statistics and the undo journal.

- **May depend on:** `model`, `meta`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## The public API

`Library` is the entry point. It opens the database (`Library::open`, `Library::open_read_only`) and offers typed operations; no SQLite type appears in them.

| Area | Operations |
|---|---|
| Catalogue | `add_root`, `upsert_track` / `upsert_tracks` (a `TrackRecord`: path, `FileStamp`, sub-track and `TrackSection`, `AudioProperties`, `TrackTags`, `CatalogTags`, a rating from the tags, `ReplayGain`, `TrackCovers`; a batch regroups its albums once), `move_file`, `move_folder`, `mark_file_missing`, `remove_missing_files` (the end of the grace period), `remove_file`, `remove_track`, `track_at`, `identity_of`, `identities_with` |
| Statistics | `record_listen` (a `Listen` judged by `PlayRules`), `stats`, `set_rating` (`Option<Rating>`), `rate_track` (whole stars, written into the file too when `TagWriteSettings::write_ratings_to_tags` is on; `RatingOutcome`, `RateError`), `set_resume_position`, `history` (`HistoryQuery`) |
| Tag edits | `write_tags` (`BatchItem`s from `meta`, `TagWriteSettings`, the `ScanOptions` of the workers, progress and a `CancelToken` → `TagWriteReport`: each file's result and the re-read), `refresh_files` (re-read files whose tags changed); `TagWriteSettings::from_settings` takes the `[advanced]` settings |
| Search | `search` (`SearchRequest` → `SearchResults`; tracks of masked library folders are left out unless `include_masked`), `rebuild_search_index` |
| Covers | `album_cover`, `track_cover` (`CoverInfo`), `cover_locations` (`CoverLocation`s), `cover_hashes`; `AlbumSummary::cover`. `ThumbnailCache` and `ArtworkPool` make and keep the thumbnails (`ThumbnailSize`, `Thumbnail`, `ThumbnailEvent`) |
| Scanning | `scan` (`ScanOptions`, an optional `Sender<ScanEvent>`, a `CancelToken` → `ScanSummary`), `unreadable_files`; `ScanHandle::spawn` runs a scan on its own thread |
| Details | `track_details` (a `TrackDetails`: path, tags, audio properties, kind, added and missing times), `track_source` (a `TrackSource`: the file and, for a CUE sheet track or a chapter, its span as a `SourceSection`, exact to the frame; `MediaSource::from` it), `roots`, `totals` (`LibraryTotals`), `most_played` (`PlayedTrack`s); `read_file_details` reads a file's tags, audio properties and loudness values in this process (`src/details.rs`) |
| Watching | `Watcher::spawn` (`WatchOptions` → `WatchEvent`s), `Watcher::rescan`, `Watcher::stop` |
| Browsing | `albums` (`AlbumQuery`: `AlbumFilter`, `AlbumQuality` (any, lossless, hi-res), `AlbumSort`, `Page` → `Paged<AlbumSummary>`, each with its `AlbumFormat`: the most frequent codec, the highest rate, depth and bit rate, lossless and hi-res), `album` / `album_including_masked` (`AlbumDetail` with `Disc`s of `TrackSummary`s), `artists` (`ArtistQuery` by `ArtistRole` → `ArtistSummary`s), `genres` (`GenreQuery` → `GenreSummary`s), `tracks` (`TrackQuery`: `TrackFilter` of an album, a person or a genre → `TrackSummary`s) |
| Playlists | `create_playlist`, `rename_playlist`, `duplicate_playlist`, `delete_playlist`, `add_to_playlist`, `insert_into_playlist`, `move_playlist_items`, `remove_from_playlist`; `playlists` (`Page` → `Paged<PlaylistSummary>`), `playlist`, `playlist_items` (`Paged<PlaylistItem>`: `ItemContent::Track` or `ItemContent::Dead` with a `DeadItem`), `dead_playlist_items`; `import_playlist` (`ImportOptions` → `ImportReport`), `export_playlist` (`ExportOptions` → `ExportReport`). Errors are `PlaylistError` |
| Library folders | `root_states` (`RootState`: masked, since when, `mask_when_missing`, polled), `set_root_masked`, `set_mask_when_missing` |

The row ids (`AlbumId`, `PersonId`, `GenreId`) give their number with `get()` or `i64::from`, and `from_raw` makes one from a number again, so a front end can keep them as plain integers. Such an id is valid only if it was obtained from this library; one the library did not give out, or whose row has gone since, finds nothing.

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

Migrations 5 and 6 clear the modification time of every file that was read, so the next scan reads the files again and fills in the new tag fields, chapters and albums (5), and the covers and loudness columns (6).

- The `schema_migrations` table records the applied migrations. The schema version is the highest one.
- Each migration runs in its own `BEGIN IMMEDIATE` transaction together with its record, so it is applied completely or not at all. Foreign keys are off while it runs, so that a table can be rebuilt, and `PRAGMA foreign_key_check` must pass before it commits.
- `Database::open` refuses a database with a newer schema (`DbError::NewerSchema`). `Database::open_read_only` opens such a database without migrating it; writes then fail with `DbError::ReadOnly`.

### Schema

Version 7 (`0001_library_schema.sql`, `0002_search_index.sql`, `0003_identity_fingerprint.sql`, `0004_search_support.sql`, `0005_album_model.sql`, `0006_covers.sql`, `0007_static_playlists.sql`):

| Area | Tables |
|---|---|
| Files | `roots`, `dirs`, `files` |
| Tracks and identity | `identities`, `tracks`, `track_tags` |
| Catalogue | `albums`, `album_people`, `people`, `track_people`, `genres`, `track_genres`, `covers` |
| Use | `stats`, `history`, `analysis` |
| Playlists and state | `playlists`, `playlist_items`, `session`, `undo_journal`, `db_meta` |
| Search | `tracks_fts` (words), `tracks_tri` (trigrams) |

- **Paths are bytes.** `roots.path`, `dirs.path`, `dirs.cover_path`, `files.name`, `covers.source_path` and `playlist_items.missing_ref` are BLOBs holding the bytes the kernel returned. `path_to_blob` and `path_from_blob` convert between them and `Path` without going through `String`. The `*_text` columns beside them hold `display_text`, which is for display and search only. Uniqueness and lookups use the bytes. Every table is `STRICT`, so a path stored as TEXT by mistake is an error.
- **Statistics** hang off `identities`, not off paths, so they survive moves. `stats.rating` holds half stars from 0 to 10; `NULL` means not rated.
- **Identity fingerprints.** `identities.content_hash` (the file's quick hash) and `identities.audio_key` (sub-track, frame range, duration, sample rate, channels) find an identity again when its track comes back.
- **`tracks.artists_fold`** holds the folded artist and album artist names, separated by U+001F, for short-query scans and ranking. The library writes it with `track_people`.
- **Folded columns** (`title_fold`, `people.fold`, `genres.fold`) hold `hk_fold()` of the text.
- **Sort columns** (`tracks.title_sort`, `albums.title_sort`, `people.sort_name`) are declared `COLLATE HK_LOCALE`, and `genres` has an index on `name COLLATE HK_LOCALE`. No unique constraint uses the collation, so rebuilding the indexes for another locale cannot fail.
- **Albums** are formed from album tags stored on each track (`album_title`, `album_fold`, `album_artist`, `main_artist`, `compilation`, `release_mbid`, the years) by the rules under "Albums and artists" below. `tracks.album_group` is the track's group key and `tracks.album_dir_id` its album folder. `album_people` holds an album's artists as the library decided them; `track_people.instrument` a performer's instrument.
- **Orphans.** Triggers remove an album when its last track leaves it (removed, or regrouped), and a person or genre when no track or album refers to it any more. The rows of a track's people and genres are changed only where they differ, so a person who stays is never removed and created again.
- **Sections.** `tracks.section` is 0 for a whole file, 1 for a CUE sheet track, 2 for a container chapter; `sub_index` is the track or chapter number.
- **Masking.** `roots.masked` and `roots.masked_since` are set by the watcher while a library folder is gone; `roots.watch_mode` records whether it is polled. The browsing queries hide the tracks of a masked folder whose `mask_when_missing` is set (the default).
- **Playlist items** (`0007_static_playlists.sql`) refer to a track's identity (`identity_id`), not to its row, so they follow a track through moves and come back with it. Each has its own `id`, a `position` without gaps (unique per playlist), and the path or URL (`missing_ref`, with `ref_kind`: a local path, a path for another system, a URL) and the title, artist, album and length it was added or imported with, which describe it while it is dead.
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

### Albums and artists

`src/catalog/albums.rs` decides which tracks form an album; `src/catalog/browse/` lists them.

- **Album identity.** Every track with an album title or a MusicBrainz release ID gets a group key:
  1. the MusicBrainz release ID (`MUSICBRAINZ_ALBUMID`), when the track has one, whatever its album title says;
  2. otherwise the folded album artist, the folded album title and the year (the original year when known, else the year; within an album folder the year most of its tracks with that title and album artist have).
- **The album artist** of the key is the folded `ALBUMARTIST` (as credited, or the album artists joined). `Various Artists`, `VA`, `V.A.` and their spellings in a few languages count as "various artists". Without an album artist, a track with the compilation flag is by "various artists"; the other tracks with the same album title in the same album folder decide together: the artist on more than half of them (the first track artist, without `feat.`, `ft.` or `featuring` and what follows) is the album artist, and without such an artist the album is by "various artists".
- **The album folder** is the track's folder, or the folder above when its name is a disc (`CD1`, `cd 02`, `Disc 2`, `Disk_3`, `Disc 2 - Live`), so discs in folders of their own form one album. A disc folder that is itself a library folder is its own album folder.
- **The split by folder.** The tracks with one key form one album, unless two of them in different album folders have the same disc and track number: then each album folder is an album of its own (two copies or two editions of a release). Its `album_key` is the group key and the folder.
- **Discs.** The disc number is the tag's, or else the one in the folder name (`CD2`, also `Album (CD2)`, `Album [Disc 1]`); the disc subtitle is the tag's, or the text after the number in a disc folder. An album's disc total is the largest disc total or disc number of its tracks. Tracks without a disc number are on disc 1.
- **The album row** takes from its tracks the most frequent title, sort title, album artist credit and sort name, year, original year, release type and IDs. A compilation (the flag on a track, or "various artists") without an album artist has none: `artist` is NULL, which the interface shows in its own language, and `album_people` is empty. A compilation with a real album artist (a DJ mix) keeps it.
- **People and roles.** One person per folded name, so `ABBA` and `Abba` are one; a MusicBrainz artist ID tells apart two people with one name. Roles (`ArtistRole`): artist, album artist, composer, performer (with an instrument), conductor, lyricist, writer, arranger, remixer, producer, engineer, mixer, DJ mixer. The sort tags (`ARTISTSORT`, `ALBUMARTISTSORT`, `COMPOSERSORT`) name a person's sort name when the role has one person; without one, the name sorts. The album artists of an album are its tagged album artists, or the artist taken from the track artists; `ArtistRole::AlbumArtist` in the queries means these.
- **Regrouping.** Writing, moving or removing tracks regroups what they were and are in: the album folder and folded album title (for the "various artists" decision), and the group keys. So retitling a track splits it off, retitling it back merges it again, a third artist in a folder without album artists makes the album a compilation, and removing one of two copies merges the split albums back into one. An album keeps its id while its key stays; one that splits or merges may get a new one.
- **Queries.** Typed and paged (`Page`, at most 1000 items; `Paged` carries the total). Albums by title (the sort title in the locale order, through its index), by artist (compilations last), by year, or newest first; all, by album artist, the ones an artist appears on (a track artist but not an album artist), by credit in any role, compilations, or by genre. An album's detail has its artists, genres, IDs and discs with their tracks in order (disc, track number, path, position in the file). People by role in the order of their sort names, with album and track counts; genres by name; the tracks of an album, a person (in one role or any) or a genre. Every query leaves out the tracks of masked library folders unless asked (`include_masked`, `album_including_masked`).
- **CUE sheets and chapters.** The tracks of a CUE sheet are parts of their file (`TrackSection::CueSheet`) and form one album from the sheet's `TITLE` and `PERFORMER`. A file with two chapters or more and no sheet becomes one track per chapter (`TrackSection::Chapter`); an audiobook's chapters form one album from its title and author. Each part has its own identity.

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
- **Records** (`records.rs`). A file described by a CUE sheet becomes one track per sheet track (`sub_index` is the track number, the frame range at the file's sample rate); otherwise it is one whole-file track. A sheet track that starts after the end of the file is left out. A file without a sheet but with two chapters or more (MP4/M4B, Matroska, Ogg, ID3v2 `CHAP`) becomes one track per chapter, numbered from 1. CUE sheets are recorded as file rows with `scan_state` 2 and no tracks, so that a changed sheet is noticed.
- **Write** (`store.rs`). Each file is one write job: its tracks through the same upsert as `upsert_track`, then the removal of the file's tracks that are no longer in it (a sheet replaced the whole-file track, or lost tracks). The jobs go to the writer without waiting, so it commits them in batches of up to 512; one job that fails is rolled back alone.
- **Covers** (`ScanOptions::covers`, on by default). The worker reports the best embedded picture of each file (by picture type, then size) and the first folder image (`cover`, `folder`, `front`, `albumart`, `album`, `artwork`, `art`, then the album title; also in the folder above a disc folder) in JPEG, PNG or WebP. It hashes each (SHA-256), checks its header against the picture limits (32 MiB, 64 megapixels) without decoding pixels, and sends only the hash, where the picture is, its format and size: no picture bytes cross the pipe or enter the database. A folder image's hash is remembered per worker by path, size and modification time. Pictures that are left out or have a bad header are passed over (noted in the report) and never fail the file. `covers` holds each picture once by hash; `tracks.cover_id` is a track's embedded picture, `dirs.cover_id` its folder's image, and `albums.cover_id` the embedded picture most of an album's tracks have (ties: the first track's), or else the folder image most of them have. Triggers remove a cover nothing refers to. A folder image that changes while the audio files do not is noticed only when the files are read again.
- **Loudness.** `tracks.rg_track_gain`, `rg_track_peak`, `rg_album_gain` and `rg_album_peak` are filled from the tags, relative to the ReplayGain 2.0 reference (−18 LUFS): `REPLAYGAIN_*`, or else Opus `R128_*_GAIN` converted (+5 dB). Playback still reads the loudness tags from the file when a track is queued (`read_file_details`), so the columns are for queries and display for now.
- **Events** (`ScanEvent`): `Walking`, `RootUnavailable`, `Compared` (found, to read, missing), `Progress` (at most every 100 ms), `FileFailed`, `Finished` with the `ScanSummary`. `CancelToken::cancel` stops a scan within a fraction of a second: the workers are killed and what was read is kept.

## Tag edits and ratings

`src/tag_writes.rs` connects the tag writer of `meta` (see its README) to the library.

- **After a write.** `write_tags` writes a batch of files and then re-reads the files written as a rescan of their folders does: only those folders are walked and compared with the database, the files whose size, modification time or inode changed are read by the worker processes, and their tracks are updated in place. Tracks keep their ids and identities, so statistics, ratings and playlist items stay with them; the albums, people, genres and search indexes follow in the same transaction. The watcher, if one runs, sees the same change later and finds nothing left to read. Files outside every library folder are written but not re-read.
- **Settings.** `TagWriteSettings` holds the ID3v2 version of new tags, the size above which files are changed in place, and whether ratings go into files (off by default), as the `[advanced]` table sets them.
- **Ratings.** `rate_track` takes whole stars (0 to 5) and stores them on the half-star scale (0, 2, …, 10); `None` clears the rating. The database is the reference: a rating read from tags only seeds a new identity. With ratings going to files, the rating is also written into the file of a whole-file track and the file is re-read: ID3v2 `POPM` with the e-mail field `Helikodu` (1, 64, 128, 196 or 255 for one to five stars, no frame for zero stars) and `TXXX:FMPS_Rating`; `FMPS_RATING` (0.0 to 1.0) and `RATING` (0 to 100, left out for zero stars) in Vorbis comments and APEv2; freeform `FMPS_Rating` and `RATING` atoms in MP4. A CUE sheet track or a chapter shares its file with other tracks, so its rating stays in the database only. A failed file write is reported in the `RatingOutcome`; the rating is stored either way.

## Covers and thumbnails

`src/catalog/covers.rs` keeps the covers in the database; `src/covers/` makes the thumbnails.

- **Cache** (`ThumbnailCache`). Files at `$XDG_CACHE_HOME/helikodu/thumbnails/<size>/<hash>.<ext>` (`ThumbnailCacheOptions::default_dir`; any folder in tests), `<hash>` the 64 hexadecimal digits of the cover's hash, `<ext>` `jpg` (quality 88) or `png` for pictures with transparency. Each file is written under a temporary name, synced and renamed into place; temporary files left behind are removed when the cache opens. The total size is limited (`DEFAULT_CACHE_BYTES`, 2 GiB); past it the least recently used files are evicted. A file's modification time records its last use (updated at most once a minute), so the order survives restarts. `remove_unreferenced` with `Library::cover_hashes` deletes the thumbnails of covers the library no longer has. The cache is safe to delete.
- **Sizes** (`ThumbnailSize`): 48 and 96 pixels for covers of 22–44 logical pixels at scale 1 and 2 (rows, deck, queue), 192 and 384 for 104–184 (panel, album tiles, album hero); `for_display` picks one. Larger views decode the picture themselves (`read_cover`).
- **Artwork pool** (`ArtworkPool`). `thumbnail` returns the file when it is cached, or schedules it. Worker threads (one per processor, two to four) take the newest request first; the queue holds 256 requests and drops the oldest beyond that; `cancel` and `retain` drop requests no longer visible. A worker asks the `CoverLocator` (the `Library`) where the picture is, reads it again (the embedded picture through the tag library, or the image file), checks its hash, decodes and scales it within the picture limits, stores it and calls the notification with a `ThumbnailEvent`. A cover that cannot be had is reported once and then `Unavailable` until `forget_failures`.

## The watcher

`src/watch/` keeps the library in step with its folders while the program runs. `Watcher::spawn` starts a thread named `helikodu-watch`; dropping the `Watcher` or calling `stop` ends it, cancelling a running rescan.

- **Watches** (`thread.rs`). Each library folder gets a recursive inotify watch through `notify`'s inotify backend (its own reader thread; no async runtime). Symbolic links are followed by the watch only with `SymlinkPolicy::Follow`. With `initial_scan` (on by default) every folder is scanned once at the start, after its watch is set up, so nothing that changes meanwhile is lost.
- **Debouncing** (`changes.rs`). Events wait until none has come for `debounce` (500 ms), or `max_delay` (1 s) after the first, so a change appears within two seconds and a burst becomes one rescan. Reads (open, close without writing) are ignored, including the scan's own. A folder holding a file that is still being written (changed, not yet closed) is held back until the file is closed, for 30 s at most, so a file being copied in is read once.
- **Rescans.** An event on a media file or CUE sheet rescans the files of its folder; an event on a folder, or on the ignore marker, rescans that subtree. The rescans of one flush run as one scan of those parts (`Scope`: the root, a subtree, or one folder's files), through the same walk, comparison, workers and writes as a full scan, so a file that disappeared from one part and turned up in another is matched by size and quick hash and keeps its tracks. A part whose folder is gone marks its files missing; a part that cannot be read, or that lies in an excluded, hidden or `.nomedia` folder, is treated as a full scan would treat it.
- **Moves.** A rename seen from both sides (`MOVED_FROM` and `MOVED_TO` with the same cookie, also between library folders) is applied with `move_file` or `move_folder` before the rescan, so the tracks keep their ids and are not read again (`WatchEvent::Moved`). A file renamed over another replaces it. A rename seen from one side only is a removal or an addition, and the scanner's quick-hash matching finds the move.
- **Overflow.** When the kernel's event queue overflows (`IN_Q_OVERFLOW`, `notify`'s rescan flag), the library folder is rescanned whole (`WatchEvent::Overflow`).
- **Watch limit.** When adding a watch fails because `fs.inotify.max_user_watches` is reached, at the start or for a new folder later, the library folder is rescanned whole and then every `poll_interval` (30 minutes by default) instead. `WatchEvent::Watching` reports `PollReason::WatchLimit` with the limit read from `/proc/sys/fs/inotify/max_user_watches`. Since Linux 5.11 the default limit is 1 % of memory, clamped to 8 192–1 048 576 watches.
- **File systems inotify cannot see** (`mounts.rs`). The file system of each library folder is looked up in `/proc/self/mountinfo` (`WatchOptions::mountinfo`): the mount with the longest mount point above the folder's real path. Network file systems (NFS, SMB/CIFS and others), FUSE mounts (`fuse`, `fuse.*`, among them the Flatpak document portal `fuse.portal`) and virtual-machine shared folders are polled every `poll_interval` instead of watched (`PollReason::FileSystem`). `fuseblk` (NTFS or exFAT through FUSE on a local disk) is watched: every change to it goes through this kernel.
- **Masking.** Every `check_interval` (2 s), and when an event says the library folder itself was removed or renamed, and before every rescan, the watcher checks that each folder is there. A folder is gone when it cannot be read, or when it is empty and the file system it was on is no longer mounted there (a mount point left behind); without a mount table, an empty folder in which the library has files counts as gone too. A gone folder is masked (`WatchEvent::RootMasked`): its watch is dropped, its pending changes are discarded, nothing in it is scanned or marked missing, and its missing files are not removed. When it is back it is unmasked, watched again and rescanned (`RescanReason::Remounted`); so is a folder on which another file system was mounted. The mask is recorded in the database (`roots.masked`), where the browsing queries see it, and cleared when the watcher finds the folder at start; whether a folder is polled is recorded too (`roots.watch_mode`).
- **Grace period.** The scanner marks files that disappeared as missing; their tracks stay. Every quarter of `grace_period` (30 days by default; at least every second, at most every hour) the watcher removes, in the folders that are not masked, the files that have been missing for the grace period, with their tracks (`Library::remove_missing_files`, `WatchEvent::MissingRemoved`). Their identities and statistics stay for when they come back. Missing times are whole seconds.
- **Events** (`WatchEvent`): `Watching` (with the `WatchMode`), `RootMasked`, `RootUnmasked`, `Overflow`, `Moved`, `RescanStarted` (with the `RescanReason`), `Scan` (the `ScanEvent`s of the running rescan), `RescanFinished` with the `ScanSummary`, `MissingRemoved`, `Error`. `Watcher::rescan` asks for a rescan of a folder or of everything.

Measured by `tests/watch.rs` (debug build, Zen 5, NVMe SSD, two workers): a created, rewritten, moved or deleted file appears in the library 0.50–0.55 s after the change (the quiet period, plus about 40 ms to read a file); a file in a polled folder with a 1 s interval after 0.93 s; 1 000 files copied into a folder within 20 ms become one rescan and are all in the library after 1.3–1.4 s.

## Playlists

`src/playlists/` holds the static playlists; `helikodu-meta` reads and writes the files.

- **Edits.** Creating, renaming, duplicating, deleting, adding, inserting, moving and removing are each one write job, so each is one transaction committed with `synchronous=FULL`: after a crash a playlist is as it was before or after an edit. A refused edit (an unknown playlist or track, a position past the end) rolls back whole; adding a list with one unknown track adds nothing. Positions are renumbered through negative values, so the uniqueness of positions holds after every statement. Moving takes a selection, keeps its order and puts it before a given item, as dragging does; only the span that changed is rewritten.
- **Identity.** An item names a track identity. A watched rename or a file found again elsewhere keeps the item's track; a track that leaves the library leaves a dead item that remembers its identity, title and path, and the item is alive again when a track with that identity returns. Dead items are listed in place (`ItemContent::Dead`) and on their own (`dead_playlist_items`), and counted in `PlaylistSummary::dead`.
- **Import.** The file is read and matched on a reader connection, then written as a new playlist in one job. Each entry is found by its path (both spellings of a name in a legacy charset), then by the end of its path (the file name, and the folders above it as far as they agree, without regard to ASCII case: playlists made on Windows or before a move), then by its folded title, its artist among the track's folded artists, and its length within 3 s. An entry that matches nothing is kept as a dead item with its path and tags; a URL is kept as an unsupported dead item and never fetched; an entry naming another playlist is skipped. The playlist is named as asked, or by the file's title, or by its file name. `ImportReport` counts each way of matching and carries the parser's warnings.
- **Export.** Live items are written with their track's current path and tags, dead items with what they were known by (or left out with `skip_dead`), in M3U8 by default, with paths relative to the file's folder by default. The file is written under a temporary name beside the target, synced, renamed over it, and the folder synced.
- **Not yet:** playlist folders, pinning, picking up playlist files found in library folders, undo and locking.

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

Writing 100 000 tracks with both indexes through `upsert_tracks` takes 14 s (143 µs per track, including people, albums and genres); rebuilding both indexes takes 0.44 s at 100 000 tracks and 2.6 s at 500 000. Every query meets the 100 ms budget at 100 000 tracks. At 500 000, queries of three characters or more stay well inside it, while one- and two-character queries, which scan, take up to a third of a second.

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
| First scan, 8 workers (the default on 32 threads) | 25.4 s | 3 900 |
| First scan with covers looked for (an embedded-picture read and a folder listing per file) | 15.9 s (16.6 s before, same machine and day) | 6 300 |
| Rescan without changes | 0.25 s | |

The budget is 500 files per second (100 000 files in under 4 minutes). The writer is the limit, not the workers: the time hardly changes between 2 and 16 workers. The first scan includes forming the albums; a track costs the writer about 140 µs with the album model, measured by `benches/search.rs`. The writer keeps 256 prepared statements per connection: with SQLite's default of 16, writing a track prepared most of its statements again, which cost more than running them.

`benches/thumbnails.rs` measures the artwork pool on 200 noisy 1200 × 1200 JPEG covers (about 1 MB each), Zen 5, release build: 192-pixel thumbnails at 126 per second with two threads and 1 200 per second with 32; 384-pixel ones at 83 and 607 per second. A lookup of a cached thumbnail takes under 1 µs.

```sh
cargo bench -p helikodu-library --bench thumbnails [-- 500 --side 3000]
```

After the scan, the bench measures the browsing queries on the same database (8 543 albums, 777 of them compilations, 1 338 album artists; median of 15 runs, 50 per page, release build, Zen 5, file database, warm cache):

| Query at 100 000 tracks | Median | With a masked library folder |
|---|---|---|
| Albums by title, first or middle page | 0.2 ms | 7–13 ms |
| Albums by artist, first / middle page | 1.3 / 6.6 ms | |
| Albums by year, middle page | 7.8 ms | |
| Albums newest first, first page | 0.2 ms | |
| Compilations, first page | 0.4 ms | |
| Albums of an album artist; albums an artist appears on | 0.03 ms; 0.07 ms | |
| Album detail with discs and tracks (200 albums) | 0.05 ms (slowest 0.2 ms) | 0.07 ms |
| Artists, first page; album artists, middle page; composers | 0.9 ms; 0.6 ms; 1.3 ms | 3.2 ms (album artists) |
| Genres with counts | 14 ms | |
| Tracks of a genre, first page; of an album artist | 1.8 ms; 0.05 ms | |

Sorting by title follows the index on `albums.title_sort`; by artist and by year the albums are sorted for each query, which stays under 10 ms at 8 500 albums. While a folder is masked, every query looks up the folder of each track it considers.

```sh
cargo bench -p helikodu-library --bench scan                   # 100 000 tracks
cargo bench -p helikodu-library --bench scan -- 10k --workers 4 --keep
cargo bench -p helikodu-library --bench scan -- 100k --reuse   # the queries on a kept database
```

## Tests

```sh
cargo test -p helikodu-library
cargo test -p helikodu-library --features bundled-sqlite
```

- `tests/database.rs`: migrating an empty database, reopening, refusing a newer schema and opening it read-only, a failing migration that leaves nothing behind, file names that are not UTF-8, the locale order in English, Swedish, German and Estonian, and the rebuild of the sort indexes when the locale changes.
- `tests/search.rs`: search through `Library` in Latin with diacritics, Cyrillic, Greek, Chinese, Japanese (kana and kanji), Thai and Korean, with queries of one to four characters; short queries; every term must match; FTS5 syntax as plain text; genre, composer, album artist and path through the word index; ranking and paging; the indexes after tag changes, moves and removals, and after a rebuild; `regexp()`; tracks of masked library folders left out.
- `tests/stats.rs`: plays, skips and partial listens; ratings from 0 to 10 and not rated; the rating from the tags taken once; statistics after a watched move, after a file that disappears and reappears elsewhere, and after a track that leaves and comes back; a re-encoded file; CUE tracks; the history and its filters; resume positions.
- `tests/track_source.rs`: the file and span of a whole-file track and of two CUE sheet tracks, an unknown track, and album, person and genre ids rebuilt from their numbers.
- `tests/scan.rs`: a small library made with the `ffmpeg` tool and scanned by real worker processes: a folder and a file name in Latin-1 bytes, a CUE album, an excluded folder, a hidden folder, a link loop, a picture and a fake video; rescans after a tag change, a removal, a move into a new folder, an addition, a file that comes back and a sheet that appears; a library folder that disappeared; a missing worker program. In debug builds, files that make the worker crash, hang or exhaust its memory limit (see `apps/scan-worker`) are reported and recorded while the scan goes on, are not read again until asked, and a scan stuck on hanging files is cancelled. The tests build `helikodu-scan-worker` themselves; without `ffmpeg` they print `SKIPPED:` and pass.
- `tests/watch.rs`: the watcher over real folders with real workers: a created, rewritten, moved and deleted file each appear within 2 s (the latencies are printed); 1 000 files copied in at once become at most a few rescans; folder renames keep the tracks' identities, and folders moved in, out and back in; a folder on a fake `nfs4` mount is polled; a library folder that is renamed away, and a mount point that is left empty, are masked without marking anything missing (the mask is in the database and hides the tracks from the browsing queries) and rescanned when they return; missing files are removed after the grace period. Without `ffmpeg` they print `SKIPPED:` and pass. The unit tests in `src/watch/` feed a queue overflow, the watch-limit error and other errors to a running watcher, and cover the debouncing, rename pairing and mount-table parsing on their own.
- `tests/albums.rs`: album identity through `Library`: compilations without album artist, by flag and by a "Various Artists" album artist, a DJ mix with the flag and an artist; the main artist of an album without album artist (`feat.`); discs in `CD1`, `CD2` and `Disc 3 - Live` folders, and tagged discs with subtitles and totals; classical credits (composer with a sort name, conductor, performer with an instrument) and the people by role; one title by two artists, one artist's title in two years; two copies of an album in two folders split, and `Album (CD1)`/`Album (CD2)` merged; MusicBrainz release IDs over the tags; tag changes that split and merge albums and change their artist; a compilation that forms and dissolves as artists come and go; a removed copy merging the split albums; orphaned albums, people and genres removed, and people's rows kept across rewrites; a move into a disc folder; the album list in Swedish, English and Estonian order; the sorts and paging; genres and their tracks; the tracks of an album and of a person; masked library folders hidden from every query, and shown with `include_masked` or without `mask_when_missing`. A batch written by `upsert_tracks` forms the same albums as the same tracks written one at a time, also for a second batch that moves tracks between albums.
- `tests/catalog_scan.rs`: albums from real files through the scanner: a FLAC file with a CUE sheet as one album of three virtual tracks with their own identities and a guest performer; an M4B audiobook made by `ffmpeg` with three chapters stored as chapter tracks of one album; discs in `CD1`/`CD2` folders with composer and conductor tags. Without `ffmpeg` they print `SKIPPED:` and pass.
- `tests/scan_covers.rs`: covers through the scanner with real workers: FLAC files with embedded front covers (one album sharing one, a track with another) and folder images (beside discs in `CD1`/`CD2`, and one equal to another album's embedded cover) stored once per picture and given to the albums; the places a cover can be read; a thumbnail made from an embedded picture; covers updated after the pictures are removed; hostile pictures (garbage after a PNG signature, a PNG header claiming 100 000 × 100 000 pixels, a GIF, a truncated JPEG) passed over without a failed file; covers turned off; the ReplayGain columns filled; a worker greeting with protocol version 1 refused.
- `tests/thumbnails.rs`: thumbnails made on request, sized and in the right format, reused from the cache and found again after reopening; a cover whose file changed reported once and then unavailable; undecodable pictures failing alone; the newest request first, a full queue dropping the oldest, cancellation; eviction of the least recently used files at the size limit and when it is lowered; thumbnails of covers no longer in the library removed.
- `tests/playlists.rs`: creating, renaming, duplicating and deleting playlists and paging through them; adding, inserting, moving a selection and removing items, with refused edits rolled back whole; items that keep their track through a watched rename, a file found elsewhere, and a track that leaves and returns; importing an M3U with absolute, relative, `file://`, Windows, renamed, URL, nested and unknown entries matched by path, by the end of the path and by tags; non-UTF-8 names through export and import in M3U, PLS and XSPF with relative and absolute paths, and the M3U8 warning; a cp1251 M3U matching a UTF-8 name and a cp1251 name; export with dead entries kept and left out, over an existing file, without leftovers; missing, oversized and garbage files.
- `tests/tag_writes.rs`: a tag edit through `Library::write_tags` with real workers: the track keeps its id, identity and rating, its new title is in the database and the search index at once, and only the written file is read again; ratings set in whole stars, kept in the database only while writing to files is off, and written into the file and cleared from it when it is on.
- `tests/writer.rs`: batching, a failed job rolled back alone, a batch that cannot commit rolled back whole, a panicking job, and the writes queued at shutdown.
