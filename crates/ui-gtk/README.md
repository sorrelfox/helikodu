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
| `src/window.rs`, `src/window/` | The main window: sidebar, content header and view history, the panel with its tabs, resizable panes, toasts |
| `src/deck.rs` | The player deck |
| `src/views/` | Albums (the grid), the album page, and stand-ins for the views that come later |
| `src/models/` | ID-backed list models: the album list fetches its rows in batches on worker threads; cover thumbnails load newest first into a texture cache |
| `src/widgets/` | Drawn widgets: cover art with the generated placeholder, the waveform seek bar, the signal lamp, rating stars, the playing indicator, the brand mark |
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
- **Covers:** `LibraryService::thumbnail` (the artwork pool and thumbnail cache, newest request first). Files are decoded on worker threads into a texture cache; the grid's visibility pass asks again for visible covers once per frame after scrolling and drops the requests of recycled tiles.
- **Player:** commands, and the service's events read on a thread of the adapter, which also feeds MPRIS (`helikodu-desktop`) and keeps the system from suspending while playing.
- **Window state:** pane widths, visible panes and the window size in `ui-state.toml`.

When the service's interface changes, only `service.rs` follows it.

## Tests

```sh
cargo test -p helikodu-ui-gtk
```

The unit tests check the accent maths against the design's worked examples, the theme rules, the formats and the sample backend. The widget tests in `tests/widgets.rs` build the main window over the sample backend; they need a display and run only under the headless compositor script, which starts `mutter --headless` (or KWin) on a private D-Bus session:

```sh
crates/ui-gtk/tests/headless.sh cargo test -p helikodu-ui-gtk --test widgets
```

Without it they report that they were skipped.

## Screenshots

`examples/screenshot.rs` renders the main window at 1440 × 900 to PNG files, dark and light, with the Albums view and an album page, for comparison with the design:

```sh
crates/ui-gtk/tests/headless.sh cargo run -p helikodu-ui-gtk --example screenshot -- --out target/screenshots
# a real library instead of the sample one; the player plays to the null output
crates/ui-gtk/tests/headless.sh cargo run -p helikodu-ui-gtk --example screenshot -- --database library.db
```
