# helikodu-ui-gtk

The desktop user interface, built with GTK 4 without libadwaita. It talks to the core only through `service`, never to `engine` or `library` directly, so another front end could be added without touching the core.

- **May depend on:** `model`, `service`, `desktop`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).
- **GTK:** 4.18 or newer. Newer behaviour (the colour scheme in `GtkSettings` from 4.20, the accent from 4.24) is looked up at run time.

## Layout

| Path | What it holds |
|---|---|
| `src/application.rs` | `run`: the GtkApplication, the player actions and their shortcuts |
| `src/backend.rs`, `src/backend/` | The adapter between the widgets and the core: `Backend`, `LibrarySource`, `PlayerControl` and the value types the widgets use. `service.rs` is the one over the running service, `sample.rs` one over invented content for tests, screenshots and `helikodu --demo` |
| `src/style.rs`, `src/style/` | The theme system: `tokens.rs` (generated), `color.rs` (OKLab mixing, contrast, the accent variants), `theme.rs` (dark or light, the accent, high contrast and reduced motion from `GtkSettings` and the Settings portal), `css.rs` (the providers) |
| `src/window.rs`, `src/window/` | The main window: sidebar, content header and view history, the panel with its tabs (Queue, and Signal path in `panel/signal_path.rs`: the path list, the playback counters, Copy diagnostics), resizable panes, toasts |
| `src/deck.rs`, `src/deck/` | The player deck; `modes.rs`: the shuffle and repeat buttons (a click switches between off and the mode used last, right click or long press opens the mode's menu; both use bundled icons, since Breeze's shuffle icon is drawn smaller than its repeat icon; every deck icon is 16 px), the A-B loop's flags on the seek bar and its chip, the sleep chip |
| `src/shortcuts.rs` | Every action that can have a shortcut, its default keys, and the keys from `[shortcuts]` in the settings given to the application |
| `src/mini_player.rs` | The mini player (Ctrl+Shift+M): its own window with the cover, the track, the seek bar, the transport and the output's lamp, and a drawer with the queue and the lyrics; like the deck it is glass in the dark scheme and light in the light one |
| `src/play_modes.rs` | The play modes in words, their menus (the Playback menu with the modes in its labels, Loop a section, Queue) and the application actions behind them; loop shortcuts Ctrl+[, Ctrl+] and Ctrl+\\ |
| `src/views/` | Albums (the grid), the album page, Artists, Genres, Tracks (the column view), Folders, the playlist page, search and the play history (`history.rs`, opened with `win.show-history`; `win.export-history` writes the history to a file); the one context menu and its window actions (`actions.rs`, with "Copies…" for a track whose file has copies), drag and drop (`drag.rs`), the table cell (`table.rs`, which can carry the copies badge after its text), and stand-ins for the views that come later |
| `src/tools/` | The tool windows: the shared frame (`tool_window.rs`), Find duplicates (`duplicates.rs`: the four ways of matching with their counts, Keep and Mark, a list view of groups and files, the trash), the copies window of one track (`copies.rs`: main-copy radio, "Make main copy", tick boxes and the trash), and the confirmation and per-file results of a move to the desktop trash (`trash.rs`); Convert (`converter.rs`), Sync and export (`sync_export.rs`) and the cover manager (`cover_manager.rs`), opened from the Tools menu, with the running conversion or sync kept on the main loop in `jobs.rs` so that a window opened again shows it. Their access to the core is `backend::export::ExportServices` (`Services::export`), over the service and over the sample library. Your fields (`user_fields.rs`, Library menu; `user_fields/draft.rs` checks a field before it is saved and words its problems) reaches the core through `backend::fields::FieldServices` (`Services::user_fields`) |
| `src/window/desktop.rs`, `src/window/home.rs`, `src/views/home.rs` | The window's desktop parts: the mini player, the tray icon and its menu, the shortcuts in other applications, pausing on lock and sleep (the D-Bus services start only with `MainWindow::start_desktop_services`, which the application calls; windows in tests have none); Home with its one-row tile rows (`TileRow`) and its lists, read from the existing library queries |
| `src/views/facet_browser.rs`, `src/views/statistics.rs` | The facet browser above the Tracks table (three linked panes, Browse button) and Library statistics (`win.show-statistics` in the Library menu; `window/statistics.rs` adds the page) |
| `src/window/narrow.rs`, `src/deck/compact.rs` | The narrow window: the queue panel and then the sidebar open over the content when it would be narrower than 560 px, with a dark layer behind them; the compact deck below 720 px. The width is followed through an empty drawing area's `resize` signal, so nothing runs while the size stays |
| `src/remote_api.rs` | Serves the application's D-Bus interface (`helikodu_desktop::app_api`) on the application's bus connection; every call becomes a backend request or a library query |
| `src/models/` | ID-backed list models: the album and track lists fetch their rows in batches on worker threads; cover thumbnails load newest first into a texture cache |
| `src/widgets/` | Drawn widgets: cover art with the generated placeholder, the waveform seek bar, the signal lamp, rating stars, the playing indicator, the brand mark, the bipolar scale of the preamps and the balance, the equalizer's response with its faders (EqGraph), the artist avatar with initials; the copies badge (`copies_badge.rs`, a bundled two-squares icon whose tooltip and accessible name say "N identical copies" and which opens the copies window) and the title line that puts it after a list row's title (`title_line.rs`); the pattern entry of the file tools with its Insert button (`pattern_entry.rs`) and the Insert popover of fields, your fields and functions (`insert_popover.rs`, its choices and the text they insert in `insert_popover/catalogue.rs`) |
| `src/dialogs/` | Settings (Library, Playback, Audio output, Loudness, DSP and EQ with the chain and the editors of the equalizer, crossfeed, channel mix and limiter, Appearance, Advanced), the loudness measurement in the sidebar footer, the choice of a SoundFont or SID song-length file, the full-output confirmation, the tag editor, the first run with the scan footer, Keyboard shortcuts (Ctrl+?), Supported formats, About; the application actions that open them. Settings › Shortcuts (`settings/shortcuts.rs`) records new keys, warns about keys in use and shows the shortcuts in other applications; the settings search (`settings/search.rs`) builds the pages once more and shows their matching rows, working; the Track analysis group of Settings › Library (`settings/track_analysis.rs`) follows `Update::Analysis` |
| `src/popovers/` | The output quick switcher above the deck's output button; the sleep timer (`sleep_timer.rs`: 15 min to 1.5 hours or a typed number of minutes, at once with a fade or after the track, +10 min, Turn off) |
| `resources/` | Compiled into the resource bundle by `build.rs`: `style/helikodu.css`, `style/tokens.css` (generated), the templates in `ui/` and the bundled symbolic icons |

`resources/style/tokens.json` holds the design's colours, type styles and sizes. `src/style/tokens.rs` and `resources/style/tokens.css` are generated from it; never edit them by hand:

```sh
cargo xtask codegen          # after changing tokens.json
cargo xtask codegen --check  # what CI runs
```

User-visible strings go through gettext (`i18n.rs`, domain `helikodu`); the templates mark theirs `translatable="yes"`.

## The service adapter

Widgets see only the types of `backend.rs`. `backend/service.rs` maps them onto the running `service::Service`:

- **Library:** album lists, pages of rows and album details through `MusicLibrary`, on the front end's worker threads. The album list gives no audio format, so the tiles show the track count and the album page takes the format from its first track. The Lossless and Hi-res quick filters stay off until the library can filter by format.
- **Covers:** `LibraryService::thumbnail` (the artwork pool and thumbnail cache, newest request first). Files are decoded on the backend's worker thread (one long-lived thread that also runs the library queries: each thread would keep its own allocator arena with the memory a large query freed) into a texture cache with a memory limit; the grid's visibility pass asks again for visible covers once per frame after scrolling and drops the requests of recycled tiles. The grid gets its model only once it is shown, with its column limit set to the columns that fit, as a list view keeps tiles for some 30 rows of its widest layout.
- **Player:** commands, and the service's events read on a thread of the adapter, which also feeds MPRIS (`helikodu-desktop`) and keeps the system from suspending while playing.
- **Window state:** pane widths, visible panes and the window size in `ui-state.toml`.
- **Browsing:** album artists, genres and their albums, an artist's releases (albums, EPs and singles by release type, appears on) and most played tracks, through `MusicLibrary`. The album list carries each album's format summary, so the tiles show it and the Lossless and Hi-res filters run in the library. The track table's keys come from `sorted_track_ids`, one sorted query that gives only ids and albums (filtered by `search` while there is text): the library sorts by every column, Format, Rating and Plays included, several keys at once. Rows are fetched by key with `track_details` and `stats`.
- **Copies:** `TrackRow::copies` comes from `TrackSummary::copies` for list rows, and from `MusicLibrary::copies_of` (the distinct files of the group but the track's own) for rows read by key, search hits and queue entries. The copies window and Find duplicates call `Services::copies_of`, `set_main_copy`, `trash_files` (`trash_copies` for identical files, `trash_duplicates` otherwise, through `LibraryService`, which announces the change), `find_duplicates` and `duplicate_counts`; a refused file comes back as a `TrashProblem`. A change of `[library.duplicates]` is applied by the service; its progress arrives as `Update::Scan` and its end as `Update::DuplicatesApplied`, which Settings › Library shows and which hides the scan footer. The sample backend invents copies (one or two for some tracks, a second path to some main copies) and only forgets a file it is asked to trash; the second copy of a group always "changed".
- **Playlists:** the static playlists through `MusicLibrary` (list, entries with dead ones, create, rename, delete, add, move, remove, import, and export as M3U8, M3U, PLS or XSPF, chosen by the extension of the file name).
- **Folders:** the library folders and their states from `root_states`; their subfolders and files are read from the file system on the worker threads, and each audio file is matched to its track with `track_at`. The library's folder table holds only folders with scanned files, not the folders between them nor the other files the status bar counts, and the view is to show folders outside the library later; so the tree stays on the file system.
- **Dialogs** (`backend/services.rs`, `Backend::services`): the settings store (every change validated and saved at once), the output backends and their devices, the library folders with scans and the folder watcher (started at launch when the settings ask for it, and after each scan), tag editing, ratings, and the decoders for Supported formats. The tag editor opens each file with the library's tag reader (`tag_editing::load_for_editing`) and keeps its stamp; saving writes the files one by one through `LibraryService::write_tags`, which refuses a file changed since it was opened and reads the written files again, and each file's result reaches the editor and its toast. A CUE sheet track, a file of a format whose tags cannot be written and a file that cannot be read open read only. Ratings are kept in the library. The sample backend keeps edits in memory.
- **Sound processing:** the DSP editors write the `[dsp]` settings through the store, which the player applies live; while a fader or slider is dragged the changes go to the store at most every 80 ms. The equalizer's response is computed with `helikodu_model::eq`, the functions the engine builds its filters with. AutoEQ `ParametricEQ.txt` files are read and written with `helikodu_service::autoeq` through the file chooser (the portal where there is one). Loudness measurements (`LibraryService::measure_loudness`, "Measure the rest" and "Measure loudness" in the context menu) report as `Update::Loudness`. The limiter's activity is not reported by the player yet, so its readout shows dashes over the running service.
- **Full output:** a gate on `Backend::send` holds requests that would start playback while the output's volume is fixed at 100 % on a device the user has not confirmed, and asks first. Playback asked for from outside the window passes the same gate: files opened with the application go through `Backend::send`, and MPRIS `Play`, `PlayPause` and `OpenUri` reach the window as `Update::Request` instead of the player.

When the service's interface changes, only `service.rs` follows it.

## Tests

```sh
cargo test -p helikodu-ui-gtk
```

`tests/insights.rs` covers the facet browser (panes narrowing each other and the table, several values, another field, a pane of labels), the Labels column and the columns of the user's fields in Tracks (their values, which of them sort), the search completion of labels and the user's fields, Library statistics (the counts, the rows that open Tracks, no frames while idle), the narrow window (the panes over the content at 640 px, closing them, the compact deck) and the D-Bus interface served by a window of an application that owns its name on the test's session bus. With `HELIKODU_INSIGHTS_SHOTS=DIR` it saves the screens to `DIR`:

```sh
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test insights
```

The unit tests check the accent maths against the design's worked examples, the theme rules, the formats and the sample backend. The widget tests in `tests/widgets.rs` build the main window over the sample backend; they need a display and run only under the headless compositor script, which starts `mutter --headless` (or KWin) on a private D-Bus session:

```sh
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test widgets
```

They also cover the Signal path tab and the output button for a sample path of each lamp the engine reports (`backend::sample::signal_path`), and the device capabilities in the output switcher. Arguments that do not start with `-` select tests by name (`-- signal`); with `HELIKODU_SIGNAL_SHOTS=DIR` those tests save the window and the switcher, dark and light, to `DIR`. Without it they report that they were skipped. `tests/dialogs.rs` does the same for the dialogs (names given after `--` pick tests): settings that round-trip through the store, the DSP editors and the other sound-processing controls applying live, Supported formats asking for a SoundFont, the full-output confirmation (also for requests from other programs), the tag editor with mixed values, the first run, the output switcher, Supported formats, About and Keyboard shortcuts; and the copies (`-- copies`): the two rows of Settings › Library applied with their progress, the badge in the track table opening the copies window, the main copy changed and copies moved to the trash only after the confirmation with each file's result, Find duplicates marking by the keep rule (never a main copy or a second path to one file), and both windows drawing no frames while idle; and the settings of the track analysis (`-- analysis`): the crossfade from the analysis and the Silence group on Playback, the leveller with its rules and the readout on Loudness, Track analysis and Waveforms on Library, play counts in files on Advanced, and the settings search finding them. With `HELIKODU_SETTINGS_SHOTS=DIR` it saves those pages to `DIR`:

```sh
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test dialogs
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test dialogs -- copies
```

`tests/tag_editing.rs` runs the tag editor over the running service: it generates FLAC and Matroska files with the `ffmpeg` tool, scans them (building the scan worker first), renames an album through the dialog and checks the files and the library, and that the Matroska file keeps its tags with a per-file error. Without `ffmpeg` it prints `SKIPPED:`:

```sh
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test tag_editing
```

`tests/desktop.rs` covers Home (one row of tiles, the lists, History), the mini player (taking the window's place, the playing track, the drawer and its remembered state), the tray menu and its actions, the tray rows of Settings › Appearance shown only with a tray, pausing on lock and sleep as the settings say, Settings › Shortcuts (recording, a key in use taken over, removing, resetting) and the settings search (results, nothing found, Esc, Open the page). With `HELIKODU_DESKTOP_SHOTS=DIR` it saves Home, the mini player with and without its drawer, Settings › Shortcuts and a search, dark and light:

```sh
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test desktop
```

`tests/labels.rs` covers labels: the sidebar row shown once a label exists, the Labels view and its empty state, the Labels popover (ticked, mixed and inherited rows, typing, Enter, one Undo for everything done while it was open), the chips of the album and artist pages, and renaming, merging and removing with Undo. With `HELIKODU_LABELS_SHOTS=DIR` it saves the screens to `DIR`:

```sh
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test labels
```

`tests/play_modes.rs` covers the play modes: the Playback menu's labels and the mode actions, the deck's shuffle and repeat buttons, stop after, the loop chip and the loop shortcuts, the sleep timer popover and chip, the queue panel's selection as the cursor of "Continue from the selected track" (a single click selects, a double click or Enter plays), the tag editor's Playback group (kept in the library and applied at once, not part of Save), the play button's menu with its Stop after submenu, every deck icon at 16 px under Adwaita and Breeze, and a paused window with the loop and sleep chips drawing no frames (the sleep chip counts down only while playing or while its popover is open). With `HELIKODU_PLAYMODE_SHOTS=DIR` it saves the deck with modes and chips, the sleep popover, the play button's menu and the Playback menu with its submenus, dark and light, to `DIR`; `HELIKODU_ICON_THEME` picks the icon theme for them:

```sh
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test play_modes
```

`tests/optional.rs` covers File info with the track analysis (analysed, not analysed yet, could not be read; no frames while idle), the Tracks view grouped by a field, one of the user's fields and an expression (the groups' order, closing and opening them by activation and with Left and Right, a group row's context menu acting on its tracks) and the offer to import from another player, once, after the first scan. With `HELIKODU_OPTIONAL_SHOTS=DIR` it saves those screens to `DIR`:

```sh
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test optional
```

The shuffle and repeat modes used last and "Follow the playing track" (View menu; it scrolls the queue panel to the track that starts) last for the session only: the window state file has no place for them yet.

`tests/interchange.rs` covers the Import window (the plan read in the background, the figures and the entries left out, an import with its toast, undone only by its own Undo and newest first, never by Ctrl+Z; folders that moved; a Helikodu data file; files that cannot be read), the "Replaced" section of Missing files and "Your data" in Settings › Advanced. With `HELIKODU_IMPORT_SHOTS=DIR` it saves the screens to `DIR`:

```sh
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test interchange
`tests/fields.rs` covers Your fields (a computed field with its example values, a field for a tag, a name and an expression that cannot be used, a field saved and removed, the question about unsaved changes) and the Insert popover of a pattern entry and of an expression. With `HELIKODU_FIELDS_SHOTS=DIR` it saves the window in three states and the popover to `DIR`:

```sh
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test fields
```

`tests/views.rs` covers the library views: the track table loading lazily and sorting by several keys, Artists, Genres and Folders, playlist edits with Undo, the context menu's actions, search with and without results, the menus, the empty library, and the frame times of the track table scrolling 100,000 tracks (slowly, fast, and jumping at random), which it prints:

```sh
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test views
crates/ui-gtk/tests/headless.sh cargo test --release -p helikodu-ui-gtk --test views -- frame_times
```

## Screenshots

`examples/screenshot.rs` renders the main window at 1440 × 900 to PNG files, dark and light, with the Albums view and an album page, for comparison with the design:

```sh
crates/ui-gtk/tests/headless.sh cargo run -p helikodu-ui-gtk --example screenshot -- --out target/screenshots
# a real library instead of the sample one; the player plays to the null output
crates/ui-gtk/tests/headless.sh cargo run -p helikodu-ui-gtk --example screenshot -- --database library.db
```

`examples/dialogs.rs` renders the dialogs the same way over the sample backend (`--dsp` renders only DSP and EQ with each editor and the pages with sound-processing controls; `--copies` only Settings › Library, the track table with its copies badges, the copies window with its confirmation and results, and Find duplicates in three ways of matching): each Settings page, DSP and EQ with each editor, the full-output confirmation, the output switcher, the tag editor (Tags, Cover, File info), the first run, a scan in progress, Supported formats and About:

```sh
crates/ui-gtk/tests/headless.sh cargo run -p helikodu-ui-gtk --example dialogs -- --out target/screenshots
```

`examples/views.rs` renders the library views, the empty library and the main, context and play button menus, dark and light:

```sh
crates/ui-gtk/tests/headless.sh cargo run -p helikodu-ui-gtk --example views -- --out target/views
```
