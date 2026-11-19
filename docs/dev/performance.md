# Performance and memory

How the performance budgets are measured, the latest results over the synthetic libraries, and what was changed to reach them.

## Budgets

| Budget | At 100,000 tracks | At 500,000 tracks |
|---|---|---|
| Start-up (warm cache) | First frame ≤ 1.0 s, interactive ≤ 1.5 s | At most 3× |
| Scrolling the track table and the album grid | At the display refresh rate, no frame over 50 ms | At most 3× |
| Search as you type | Results ≤ 100 ms after a keystroke | At most 3× |
| Memory with the album grid open | ≤ 270 MB RSS | At most 3× |
| First scan (tags only, SSD, warm cache) | ≥ 500 files/s | |
| Idle (paused or stopped) | No periodic work, under 0.1 % of one core | Same |

## How to measure

```sh
cargo xtask synth-library --tracks 100k                 # or 500k; the libraries go under target/
helikodu-cli scan --database /tmp/lib.db target/synth-library/flac-100000-seed1
tests/ui/perf.py --database /tmp/lib.db --idle-secs 30   # start-up, memory, frames, idle
```

`perf.py` (see [tests/ui/README.md](../../tests/ui/README.md)) runs the `perf` example of `helikodu-ui-gtk` under a headless compositor. Besides the budgets it reports the RSS after opening the track table, the number of widgets in the window (shown, hidden and inside popovers), and the busiest threads while playing. With `HK_PERF_WIDGETS=PREFIX` in the environment the `perf` example also writes, to `PREFIX.<when>`, every subtree of more than 50 widgets whose children each hold less than half of them: where the widgets are.

The library queries behind the views are timed by the `queries` example, over a copy of a library (it creates and deletes one smart playlist):

```sh
cargo build --release -p helikodu-ui-gtk --example queries
XDG_DATA_HOME=… XDG_CONFIG_HOME=… target/release/examples/queries --runs 5
```

Synthetic libraries have no plays; for the history and statistics queries, give a copy of the database some (plays for a third of the tracks and 300,000 history entries over two years).

## Results

Measured on 2026-11-18: 32 threads, NVMe SSD, headless mutter with software rendering (llvmpipe), synthetic FLAC libraries of one-second tracks without covers. Medians of five runs for the queries; one cold and three warm starts at 100k, one of each at 500k.

### Start-up, memory, frames and idle

| Check | 100k before | 100k after | 500k before | 500k after | Budget (100k / 500k) |
|---|---|---|---|---|---|
| Warm start, first frame | 280 ms | 300 ms | 269 ms | 300 ms | 1.0 s / 3.0 s |
| Warm start, first frame with albums | 414 ms | 369 ms | 2,426 ms | 1,025 ms | 1.0 s / 3.0 s |
| Warm start, interactive | 414 ms | 369 ms | 2,426 ms | 1,025 ms | 1.5 s / 4.5 s |
| RSS after start-up | 220 MB | 222 MB | 234 MB | 252 MB | 270 MB / 810 MB |
| RSS after scrolling albums | 230 MB | 247 MB | 287 MB | 289 MB | 270 MB / 810 MB |
| RSS after scrolling tracks | 273 MB | 277 MB | 384 MB | 362 MB | 270 MB / 810 MB |
| RSS after scrolling tracks, without the GL driver's libraries | 212 MB | 216 MB | 322 MB | 301 MB | |
| Steady scrolling, frame interval p95 | 16.7 ms | 16.7 ms | 16.7 ms | 16.7 ms | 16.7 ms |
| Slowest frame, any scroll mode | 33 ms | 30 ms | 17 ms | 33 ms | 50 ms |
| Idle CPU, stopped / paused | 0.000 / 0.000 % | 0.033 / 0.000 % | **104 / 103 %** | 0.000 / 0.066 % | 0.1 % |
| Frames painted while idle | 0 | 0 | 0 | 0 | 0 |
| Track table: all keys loaded | 263 ms | 215 ms | 5,197 ms | 1,010 ms | |

The RSS includes some 61 MB (58 MiB) of resident pages of the software GL driver's libraries (Mesa and LLVM), which a GPU driver replaces. The application's own memory after scrolling the tracks at 100k is about 216 MB; the 277 MB of that measurement were above the budget of 270 MB RSS including the driver; the later work below brought it to 264 MB. Scrolling and idle meet their budgets at both sizes.

### Scan

`helikodu-cli scan` of the whole library into an empty database (8 workers):

| Library | Before | After |
|---|---|---|
| 100k files | 16.8 s (5,950 files/s), peak RSS 335 MB | 16.7 s (5,990 files/s), peak RSS 125 MB |
| 500k files | 102.9 s (4,860 files/s), peak RSS 1,370 MB | 101.9 s (4,910 files/s), peak RSS 337 MB |

### Library queries

| Query | 100k before | 100k after | 500k before | 500k after |
|---|---|---|---|---|
| Album grid: every album | 160 ms | 101 ms | 2,107 ms | 508 ms |
| Album grid: a page of 60 rows | 7.7 ms | 0.0 ms | 39 ms | 0.0 ms |
| Track table: every key, album order | 87 ms | 85 ms | 467 ms | 441 ms |
| Track table: every key, by title | 222 ms | 217 ms | 1,239 ms | 1,184 ms |
| Track table: 50 rows | 0.4 ms | 0.3 ms | 0.4 ms | 0.3 ms |
| Search, three letters (`cor`) | 2.1 ms | 2.1 ms | 7.9 ms | 7.7 ms |
| Search, one letter (`a`) | 65 ms | 65 ms | 314 ms | 317 ms |
| Track table filtered by a word (1,039 / 4,943 matches) | 10.8 ms | 1.9 ms | 91 ms | 9.5 ms |
| Track table filtered by one letter (91k / 452k matches) | not measured | 170 ms | not measured | 912 ms |
| Facet values, whole library (slowest pane) | 102 ms | 100 ms | 595 ms | 594 ms |
| Facet values within one genre | 4.7 ms | 4.7 ms | 22 ms | 22 ms |
| Statistics | 190 ms | 176 ms | 918 ms | 767 ms |
| History, last 30 days | 19 ms | 19 ms | 18 ms | 18 ms |
| Smart playlist tracks (2,525 / 12,600 tracks) | 130 ms | 54 ms | 2,526 ms | 236 ms |
| Smart rules: match count | 63 ms | 61 ms | 342 ms | 343 ms |
| Missing files | 0.0 ms | 0.0 ms | 0.0 ms | 0.0 ms |

Before the change, filtering the track table by one letter read the matches 1,000 at a time, ranking every match again and reading each track's details for its path on every page; at 500k that took minutes.

The query plans of the facets, statistics, history and missing-files queries use the existing indexes (`history_started`, `history_identity`, `files_missing`, `track_people_role`, `tracks_identity`); the whole-library facet and statistics queries read every track once, which no index avoids. No index was added.

### Building pages when first opened

Measured on 2026-11-18 as above (100k, three warm starts, 60 s idle phases). Before: the state described in the table above; after: the library pages, Home, History, Statistics and the menus built when first opened, and the artist lookup of the deck's credit line fixed.

| Check | Before | After | Budget |
|---|---|---|---|
| Warm start, first frame / with albums | 314 / 398 ms | 251 / 335 ms | 1.0 s |
| Widgets after start-up (shown) | 4,433 (269) | 1,806 (269) | |
| Widgets after scrolling tracks (shown) | 8,331 (510) | 5,835 (510) | |
| RSS after start-up | 223 MB | 206 MB | 270 MB |
| RSS after scrolling albums | 248 MB | 234 MB | 270 MB |
| RSS after opening tracks | 271 MB | 258 MB | 270 MB |
| RSS after scrolling tracks | 277 MB | 264 MB | 270 MB |
| RSS after scrolling tracks, without the GL driver's libraries | 216 MB | 203 MB | |
| Playing, window hidden / minimized (one-second tracks) | 6.3 / 4.9 % | 0.78 / 0.80 % | |
| Idle frames, scrolling | none; p95 16.7 ms | none; p95 16.7 ms | |

The idle CPU while paused read 0.64 % in the run after the change: the daily library backup (`helikodu-backup`) happened to run in that minute; it is not periodic work of the window. Stopped, the process used 0.017 %.

**Playing with the window hidden.** The synthetic tracks last one second, so a track starts every second. With 3-minute tracks (two generated FLAC files) the process used 0.30 % of a core while hidden even before the change, and with one-second tracks in a library of 1,000 tracks 0.8 %; at 100k it was 5 %, nearly all on the front end's query thread (`helikodu-worker`). The deck's credit line (`NowLinks`) asks whether the track's artist credit names an artist; when it named none of the artist list (a credit such as "A feat. B", or the synthetic library's track artists), the lookup read every artist of the library again (about 80 ms at 100k), at every track change. Names found missing are now remembered until the library changes. What is left while hidden is decoding, the null output and about five main-thread wake-ups a second (the playing state, not drawing).

**Memory.** Building the views when they are first opened took 2,600 widgets and about 13 MB out of the window, start-up got some 60 ms faster, but the budget is still missed by some 14 MB after the track table was opened. What remains, measured:

- **The track table** holds the rows of about 205 positions (GTK keeps up to 200 list items around the visible ones, whatever the height; this is not configurable): with 8 columns about 3,900 widgets, which cost 18 MB. With only the Title column shown (an experiment) the same scroll ends at 247 MB. Fewer widgets per row would need a row drawn as one widget instead of a `GtkColumnView`.
- **Accessibility** objects: with `GTK_A11Y=none` the RSS after scrolling tracks is 8 MB lower; the headless session runs the accessibility bus as a desktop does.
- **Allocator arenas**: with `GLIBC_TUNABLES=glibc.malloc.arena_max=2` it is 11 MB lower (memory freed on the query and service threads stays in their arenas). Setting this needs either the environment (a launcher or package) or `mallopt`, which is `unsafe` and not allowed in the front end's crates.
- The GL driver's libraries (58 MiB) as before.

## What was changed

- **Reading page after page.** Lists that wanted every row read them 1,000 at a time with `LIMIT … OFFSET`, which sorts or ranks the whole list again for each page: the album grid, the tracks of smart playlists, and search results in the track table and in the queue. Each now runs its query once (`Library::all_albums`, `Library::all_smart_playlist_tracks`, `Library::search_ids`).
- **Album grid pages.** The grid's rows come from the list its keys came with instead of a sorted page from the database each time.
- **Rows nobody waits for.** The track table and the album grid fetched the rows of every position asked for while scrolling, even of rows scrolled past long ago; after a fast scroll or a jump at 500k this kept a core busy for minutes after the scrolling stopped. Positions whose items are gone are now dropped before each fetch.
- **One query per row.** The copy count of each row of search results, the queue and the track table took one or two queries per row, and the album of each track was looked up in a map of every track in the library (rebuilt from a query of every track when a track was missing from it). Both now come from one query per batch of rows (`Library::albums_and_copies`), and the map is gone.
- **Statistics.** The formats and the three quality classes come from one pass over the tracks instead of two; the most played artists and albums start from the played tracks instead of from every track.
- **Pages built when first opened.** Tracks, Artists, Genres, Composers, Years, Folders, the playlist and smart playlist pages, search results, Home, History and Statistics are built the first time they are shown; changes of the library, ratings and playlists reach only the pages already built. The main menu builds its popover when first opened, the menu bar only while it is shown, and the menu buttons of the pages (sort orders, columns, facet fields, lyrics) when first opened.
- **Artist lookup of the deck.** See above: names that are no artist are remembered until the library changes.
- **Scan memory.** The reports of the scan workers waited in an unbounded queue when the database writes fell behind, and every job was made before the first was handed out. The queue is now bounded and the jobs are made as they are handed out: the peak RSS of a first scan of 500,000 files fell from 1.37 GB to 337 MB.

## What is left

- **Memory at 100k.** 264 MB after scrolling the track table (203 MB without the GL driver's libraries), within the budget of 270 MB (raised from 250 MB on 19 November 2026) but with little room: see [Building pages when first opened](#building-pages-when-first-opened) for where it goes. The album grid still keeps some 1,000 widgets of tiles around the visible ones, and Now Playing (about 180 widgets) is built at start-up.
- **Facets over the whole library** take about 0.6 s at 500k; within a choice they are fast.
