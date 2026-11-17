# Spike: large lists, covers, light/dark and the window shell in GTK 4

## Question

Can the player's interface be built on GTK 4 (gtk-rs) **without libadwaita**, or does the project have to fall back to a Qt/QML front end? In detail:

1. Does a `GtkColumnView` over 200,000 tracks, fed by a lazy model that holds only IDs, scroll at the display's refresh rate, with no frame over 50 ms? Also with 12 columns instead of the default 8?
2. Does a `GtkGridView` of album tiles, with covers decoded and scaled off the main thread and visible tiles served first, do the same?
3. Can the application follow the system's light/dark preference and accent colour without libadwaita, and switch its own stylesheet live?
4. What does the design's stylesheet cost? (It is applied in every measurement unless stated.)
5. Does a window without a title bar work: the top strips as drag areas, and the desktop's own window buttons, on KDE Plasma and GNOME?

## Method

`ui-lists-spike` is a standalone Cargo project (GTK 4.24.1, gtk4 crate 0.11.5, `image` 0.25 with JPEG only, `async-channel`; `#![forbid(unsafe_code)]`). It builds the design's window shell (sidebar, three top strips, side panel, always-dark player deck) at 1440 × 900 and puts either the track table or the album grid in the content area. A tick callback then scrolls by script and the program exits by itself:

| Phase | Table | Grid |
|---|---|---|
| warm-up (idle) | 2 s | 2 s |
| slow scroll | 8 s at 1,500 px/s (50 rows/s) | 8 s at 900 px/s |
| fast scroll | 6 s at 15,000 px/s (500 rows/s) | 6 s at 9,000 px/s, then 2 s idle |
| random jumps | 5 s, a random position **every frame** | same, then 2 s idle |
| theme switch | 4 s, light/dark every 0.5 s | same |

Random jumps are a worst case: every visible row is rebound in every frame, as when the scrollbar is dragged across the whole library.

- **Lazy model.** `IdModel` implements `GListModel` over a `Vec<u32>` of track IDs (a permutation, as a sorted query would return). An item object is created only when GTK asks for a position; its row data is built only when a cell is bound (synthetic and deterministic, standing in for a read-only database fetch). Cells are labels, plus a cairo-drawn rating cell in the 12-column table. Row striping is set from the item position at bind time.
- **Grid.** 20,000 albums; 4,000 synthetic 600 × 600 JPEG covers (gradients, shapes and grain, about 58 KB each) are written to `target/covers` on first run. Four worker threads decode and scale to the tile's device-pixel size (160 px at scale 1). The request queue is a stack (newest first), capped at 256; recycled tiles cancel their request; once per frame, bound tiles that intersect the viewport and still show a placeholder are moved to the front. Textures go into a 600-entry LRU.
- **Frame times** come from the `GdkFrameClock`: *interval* is the difference between consecutive frame times (what the user sees; it equals the refresh interval when nothing is dropped); *work* is main-thread time from `before-paint` to `after-paint` (layout, snapshot, GL render submission). "Late" counts intervals longer than 1.5 × the refresh interval. Memory is VmRSS from `/proc/self/status`.
- **Light/dark and accent** are read from two sources: `GtkSettings` (`gtk-interface-color-scheme`, `gtk-accent-color`) and the Settings portal directly over D-Bus (`color-scheme`, `accent-color`). A switch swaps the `hk-dark`/`hk-light` class on the window and reloads a small second CSS provider with the accent variants. To script a live change without touching the desktop's settings, the program can act as a stand-in Settings portal on a private session bus (`ui-lists-spike fake-portal <ms> <button-layout>`), which flips the scheme and accent like the real one.
- **Window shell.** The title bar is a hidden empty widget, the three top strips are `GtkWindowHandle`s, and `GtkWindowControls` sit in the brand strip (start) and the panel strip (end). On headless mutter and GNOME Shell, `--input-test` drives the real pointer through mutter's remote-desktop D-Bus API: double-click and drag on the strips, then the maximize, close and minimize buttons.

Environments (one machine: AMD Radeon PRO WX 3200, 32 threads, Fedora; GTK picked its GL renderer everywhere):

| Name | What | Refresh |
|---|---|---|
| KDE desktop | The running Plasma Wayland session, real monitor (2560 × 1440, scale 1) | 75 Hz (13.34 ms) |
| KWin headless | `kwin_wayland --virtual`, private D-Bus session with the KDE portal | 60 Hz |
| mutter headless | mutter 51 `--headless`, private D-Bus session | 60 Hz |
| GNOME Shell headless | GNOME Shell 51 `--headless`, private D-Bus session with the GNOME portal | 60 Hz |

Headless compositors render to a virtual monitor, but GTK still renders on the GPU, so the client-side numbers are comparable. GNOME was **not** measured on a real display.

Reproduce: `./measure.sh desktop|kwin|mutter|gnome-shell|follow` (results go to `results/`). The raw output of every run, including the theme logs, is in [`results/`](results/).

## Results

### Track table, 200,000 rows (frame interval and main-thread work in ms)

8 columns, stylesheet on:

| Environment | Phase | Interval mean | p95 | p99 | Worst | Late | Work mean | p95 | p99 | Worst |
|---|---|---|---|---|---|---|---|---|---|---|
| KDE desktop (75 Hz) | slow scroll | 13.31 | 13.34 | 13.34 | 13.36 | 0 / 601 | 1.98 | 2.67 | 2.99 | 3.51 |
| | fast scroll | 13.34 | 13.34 | 13.34 | 13.35 | 0 / 449 | 2.89 | 4.19 | 4.58 | 5.67 |
| | random jumps | 19.28 | 26.67 | 26.67 | 26.67 | 115 / 258 | 15.53 | 22.71 | 23.35 | 24.41 |
| KWin headless | slow scroll | 16.62 | 16.67 | 16.68 | 16.68 | 0 | 1.61 | 2.75 | 3.14 | 3.52 |
| | fast scroll | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 3.19 | 4.19 | 4.49 | 4.69 |
| | random jumps | 20.77 | 33.31 | 33.32 | 33.33 | 59 / 240 | 14.69 | 21.37 | 21.66 | 21.81 |
| mutter headless | slow scroll | 16.30 | 16.67 | 16.67 | 16.67 | 0 | 1.45 | 2.38 | 2.71 | 2.97 |
| | fast scroll | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 2.91 | 3.58 | 4.05 | 4.85 |
| | random jumps | 16.67 | 16.69 | 16.75 | 16.95 | 0 | 12.60 | 16.94 | 17.56 | 18.30 |
| GNOME Shell headless | slow scroll | 16.27 | 16.67 | 16.67 | 17.03 | 0 | 1.49 | 2.42 | 2.68 | 3.19 |
| | fast scroll | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 2.79 | 3.46 | 3.63 | 4.11 |
| | random jumps | 16.67 | 16.70 | 16.75 | 16.79 | 0 | 12.59 | 17.03 | 17.22 | 17.63 |

12 columns (adds composer, bitrate, plays and a drawn rating cell):

| Environment | Phase | Interval mean | p95 | p99 | Worst | Late | Work mean | p95 | p99 | Worst |
|---|---|---|---|---|---|---|---|---|---|---|
| KDE desktop (75 Hz) | slow scroll | 13.33 | 13.34 | 13.34 | 13.35 | 0 | 2.61 | 3.69 | 4.18 | 4.68 |
| | fast scroll | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 3.63 | 5.23 | 6.12 | 6.37 |
| | random jumps | 24.76 | 39.75 | 39.85 | 39.96 | 137 / 201 | 22.13 | 30.95 | 31.63 | 32.16 |
| KWin headless | fast scroll | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 3.54 | 4.35 | 4.43 | 4.57 |
| | random jumps | 25.43 | 33.34 | 33.34 | 33.34 | 103 / 196 | 17.76 | 23.49 | 24.62 | 29.29 |
| mutter headless | fast scroll | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 3.67 | 4.69 | 5.52 | 7.42 |
| | random jumps | 20.04 | 30.76 | 31.03 | 31.34 | 52 / 249 | 17.64 | 23.16 | 23.98 | 24.15 |
| GNOME Shell headless | fast scroll | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 3.33 | 4.41 | 4.81 | 5.34 |
| | random jumps | 19.99 | 30.72 | 30.84 | 31.01 | 50 / 250 | 17.49 | 22.87 | 23.27 | 23.63 |

- Normal scrolling, slow or fast, runs at the refresh rate everywhere, with 8 or 12 columns: **no late frame at all**, and main-thread work stays below 7.5 ms even at the 99th percentile.
- **No frame over 50 ms in any scrolling phase, in any environment.** The only intervals over 50 ms are the first frames of the warm-up phase (window mapping, 50–83 ms).
- Random jumps on every frame cost 12–16 ms of work with 8 columns (enough at 60 Hz, not quite at 75 Hz) and 17–22 ms with 12 columns, so frames are shown every second refresh (26–40 ms).
- GTK kept 205 rows of widgets (1,640 cells at 8 columns, 2,460 at 12), independent of the row count.
- The model with 200,000 rows was built in 21–44 ms; the first frame was painted 95–120 ms after `present()`.

### Album grid, 20,000 albums

`max-columns 12` (the grid shows 4 columns at this window width):

| Environment | Phase | Interval mean | p95 | p99 | Worst | Late | Work mean | p95 | p99 | Worst |
|---|---|---|---|---|---|---|---|---|---|---|
| KDE desktop (75 Hz) | slow scroll | 13.33 | 13.34 | 13.34 | 13.34 | 0 | 0.55 | 0.87 | 1.10 | 4.23 |
| | fast scroll | 13.34 | 13.34 | 13.34 | 13.34 | 0 | 3.30 | 4.17 | 5.43 | 6.00 |
| | random jumps | 20.06 | 26.43 | 26.54 | 26.68 | 125 / 248 | 16.50 | 17.52 | 20.08 | 20.90 |
| KWin headless | fast scroll | 16.67 | 16.67 | 16.67 | 16.68 | 0 | 3.45 | 5.49 | 7.35 | 8.01 |
| | random jumps | 22.08 | 33.26 | 33.36 | 33.65 | 73 / 225 | 17.52 | 19.09 | 22.58 | 25.58 |
| mutter headless | fast scroll | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 3.54 | 5.86 | 6.38 | 9.07 |
| | random jumps | 17.41 | 19.04 | 29.33 | 30.78 | 4 / 287 | 16.75 | 19.17 | 20.97 | 27.42 |
| GNOME Shell headless | fast scroll | 16.67 | 16.67 | 16.67 | 16.67 | 0 | 3.24 | 5.21 | 5.78 | 6.58 |
| | random jumps | 16.80 | 16.98 | 18.25 | 32.62 | 1 / 297 | 16.05 | 16.65 | 19.55 | 20.76 |

Covers (all environments alike):

- Decoding and scaling one 600 × 600 JPEG to 160 × 160 on a worker: 1.4 ms mean, 1.7 ms p95, under 4 ms worst.
- From binding a tile to its cover on screen: 15–16 ms median, 18–29 ms p95 (`max-columns 12`).
- After scrolling stops, every **visible** tile has its cover 13–19 ms later (after random jumps); after the fast scroll, the visible covers were already in place. The multi-second worst latencies in the raw results belong to off-screen tiles that are deliberately served last.
- **`max-columns` matters.** GTK keeps about `max-columns × 30` tiles bound: 385 tiles at 12, 641 at 20, although only about 16 are visible. With 20, a random jump costs 26–29 ms of work (frames every 27–40 ms instead of 17–27 ms), and RSS is 5–10 MiB higher.

### Memory (RSS at the end of the run, MiB)

| Run | KDE desktop | KWin | mutter | GNOME Shell |
|---|---|---|---|---|
| Shell with a 100-row table (baseline) | 145.6 | 142.8 | 144.6 | 143.9 |
| 200k tracks, 8 columns | 156.6 | 151.6 | 153.3 | 153.4 |
| 200k tracks, 12 columns | 165.4 | 157.5 | 159.5 | 159.3 |
| 200k tracks, 8 columns, no stylesheet | 155.2 | 150.2 | 151.4 | 151.8 |
| 20k albums, max-columns 12 | 229.1 | 223.1 | 227.9 | 224.5 |
| 20k albums, max-columns 20 (peak) | 237.5 (256.2) | 233.4 (253.4) | 233.1 (253.1) | 231.7 (253.1) |

About 43 MiB is GTK itself before any window; the GL driver and the first frames bring the empty shell to about 145 MiB. 200,000 tracks add 9–11 MiB over the baseline (8 columns), 12 columns another 6–9 MiB. The grid adds about 80 MiB, most of it the 600 cached 160 × 160 textures (about 60 MiB of pixel data). The cache size is the lever here, not the library size.

### Stylesheet cost

Same table (200k rows, 8 columns) with and without the stylesheet:

| Environment | Work mean, slow scroll | Work mean, fast scroll | Work mean, random jumps | Theme switch to end of frame |
|---|---|---|---|---|
| KDE desktop, stylesheet | 1.98 | 2.89 | 15.53 | 23.4 ms (worst 26.5) |
| KDE desktop, none | 1.42 | 3.00 | 11.78 | 16.8 ms (worst 18.2) |
| mutter, stylesheet | 1.45 | 2.91 | 12.60 | 22.7 ms (worst 24.0) |
| mutter, none | 1.52 | 3.12 | 11.45 | 17.3 ms (worst 18.6) |

The stylesheet costs little in normal scrolling (within noise to 0.6 ms), 1–4 ms in random jumps, and about 6 ms in a full light/dark switch. A switch with 12 columns takes 27–29 ms, with the 100-row table 15–17 ms; most of it is GTK reloading its own theme variant, which the spike forces for a manual override. Either way a switch drops one or two frames, once.

### Light/dark and accent without libadwaita

- **It works.** On Wayland, GTK 4.24 receives `color-scheme` and `accent-color` from the Settings portal into `GtkSettings` (`gtk-interface-color-scheme`, `gtk-accent-color`), and notifies on change. The spike swaps its class and accent provider in the same main-loop turn (within 1 ms of the notification). With the stand-in portal flipping every 1.5 s, every one of the changes arrived, through both paths, and the new scheme was painted in the next frame (16–21 ms from change to end of frame).
- Real values seen: the KDE portal reported dark and the Breeze accent `#3DAEE9`; the GNOME portal (headless GNOME Shell) reported dark and `#3584E4`.
- Each portal change produces two notifications (scheme, then accent), so the spike restyled twice per change. The real code should coalesce them (apply once, from an idle callback).
- `gtk-application-prefer-dark-theme` is deprecated since GTK 4.20. Setting it changes `gtk-interface-color-scheme` and sends a notify back, which caused a feedback loop in the first version. The spike now writes `gtk-interface-color-scheme` only for a manual Light/Dark override and leaves GTK alone when following the system.
- The KDE portal on this machine also supplies the GTK theme (Breeze), icon theme (breeze-dark), font (Noto Sans 10) and button layout to GTK.

### Theme leaks found and fixed in the stylesheet

The design's draft stylesheet had never been loaded in GTK. Under Breeze and GTK's built-in Adwaita it showed three problems, now fixed in `style/helikodu.css` (sections 9 and 10):

1. The list inside a column view and the grid view keep the **theme's view background**, so the table stayed dark after switching to light. Fixed by resetting `columnview.hk-table > listview` and `gridview.hk-grid`.
2. **Adwaita paints buttons, slider knobs and switches with `background-image`**, which a `background-color` rule does not replace: the play button lost its accent fill and the output button took Adwaita's text colour. Fixed by `background-image: none` and an explicit `color` on every restyled stock widget.
3. The stock window buttons and the search entry were not styled at all. Added rules for `windowcontrols > button` and gave the search entry the input classes.

After the fixes, the application's surfaces look the same under Adwaita and Breeze, in light and dark (checked by rendering the window to PNG headless). One leftover: under Breeze, GTK warns that the seek slider's knob reports a negative minimum size (from the negative margins); the slider rules need tuning in the real crate.

### Window without a title bar

Results are in [`results/input.md`](results/input.md) (headless GNOME) and the `shell:` sections of the other results.

| Check | KDE Plasma (desktop) | mutter / GNOME Shell (headless) |
|---|---|---|
| No title bar from GTK or the compositor | Yes. KWin reports frame geometry equal to client geometry (1440 × 900), so it adds no server-side decoration. GTK draws client-side (`csd`), with shadow and resize borders | Yes |
| Window buttons shown | Minimize, maximize, close on the right, from KDE's layout `icon:minimize,maximize,close` (the `icon` item shows nothing; the spike sets no window icon) | Follows the layout: `appmenu:close` gives only close on the right; `close,minimize,maximize:` puts all three in the brand strip on the left, the end controls stay empty |
| Double-click on the content header strip | **Not tested with input** | Maximizes, and restores on the second double-click |
| Drag on the panel strip | **Not tested with input** | The window moves with the pointer |
| Maximize button | **Not tested with input** | Maximizes and restores |
| Close button | **Not tested with input** | Sends close-request |
| Minimize button | **Not tested with input** | The window loses focus. Wayland tells the client nothing more, as expected |

On KDE the behaviour of the drag areas and buttons could not be driven by script: KWin offers no input injection that does not need the user's approval or a privileged client, and moving the user's real pointer was out of the question. The pieces involved are the same GTK code paths that work on mutter (`GtkWindowHandle` asks the compositor to start a move; `GtkWindowControls` sends maximize/minimize/close requests through xdg-shell), and KWin supports those requests. A manual check on Plasma is still needed: `./target/release/ui-lists-spike --view table --follow-only --time-scale 30` keeps the window open for five minutes.

Note also: GTK rebuilds the window-control buttons whenever the window state changes (for example after maximizing), so code must not hold on to those button widgets.

## Problems and limits

- Row data is synthetic and in memory; the real player fetches it from SQLite on another thread, which adds latency but no main-thread work if batched. Covers are decoded from 600 px JPEGs; the real disk thumbnail cache at the right size will be cheaper.
- Scale 1 only. Fractional scaling and HiDPI were not measured (the code requests covers at the surface scale). X11 was not measured.
- GNOME was measured on headless mutter and headless GNOME Shell, not on a real GNOME session with a monitor. Both run on the same machine as the KDE session, which was in use during the measurements.
- Drag areas and window buttons on KDE were not tested with input (see above).
- Accessibility (AT-SPI) was not examined; headless runs disable it (`GTK_A11Y=none`).
- Striping needs the row widget, which `GtkColumnViewRow` does not expose; the spike reaches it as the parent of the first cell's parent. It works on GTK 4.24 but depends on the widget tree.
- Without an icon theme that has the media icons (the stand-in portal case), the deck showed "missing image" icons: the bundled symbolic fallbacks are needed.

## Recommendation

**Keep GTK 4 without libadwaita. Do not trigger the Qt/QML fallback.**

Every question was answered positively on both desktops' compositors:

- 200,000 tracks scroll at the display's refresh rate (75 Hz on the real KDE desktop, 60 Hz headless) with 8 and with 12 columns, with no late frame in slow or fast scrolling, and main-thread work under 7.5 ms at the 99th percentile. No scrolling frame anywhere exceeded 50 ms; even the deliberately pathological random jumps stayed below 41 ms.
- The album grid with asynchronous covers does the same, and visible covers appear within about 20 ms after scrolling stops.
- Memory is dominated by GTK and the GL driver (about 145 MiB for the shell); the library size adds about 10 MiB for 200,000 tracks.
- Light/dark and accent follow the system live through GtkSettings and the Settings portal, without libadwaita; the design's own stylesheet works under Adwaita and Breeze once theme leaks are reset.
- The window without a title bar works on mutter/GNOME Shell with real input, and on KWin it gets no extra server-side title bar and shows the desktop's buttons.

The single-widget-per-row alternative for the table is not needed. It becomes interesting only if more than 12 columns or much slower cells are wanted, because the one scenario that misses frames (a jump to a new position on every frame) scales with the number of cell widgets.

## Lessons for the `ui-gtk` crate

1. **Lazy models work as planned.** A `GListModel` over an ID vector, item objects created on demand, row data built at bind time and cached on the item. GTK binds about 200 rows; never put a generic sort or filter model over the whole library.
2. **Set `max-columns` on the album grid from the widest expected layout** (about 12 for 160 px tiles), not higher: GridView keeps about `max-columns × 30` tiles bound, which is memory and bind cost.
3. **A newest-first cover queue needs a visibility pass.** With a capped stack, tiles that are still bound lose their requests; once per frame, move bound, visible, empty tiles to the front, and requeue them if dropped. Cancel on unbind.
4. **Coalesce theme changes** (scheme and accent arrive as separate notifications) and apply them once per main-loop turn.
5. **Do not set `gtk-application-prefer-dark-theme`** on GTK 4.20+. Follow `gtk-interface-color-scheme` and `gtk-accent-color`; write `gtk-interface-color-scheme` only for a manual override. Keep the portal path for GTK 4.18 and X11.
6. **Every restyled stock widget resets `background-image` and sets `color`**, and stock lists inside column and grid views reset their background. Test every screen under Adwaita and Breeze, light and dark; rendering the window to PNG headless (`WidgetPaintable` plus the window's renderer) makes this scriptable in CI.
7. **Headless compositors are good test beds.** `mutter --headless`, `gnome-shell --headless` and `kwin_wayland --virtual` on a private D-Bus session give stable frame timing, and mutter's remote-desktop API makes window-shell input tests possible in CI.
8. **Do not keep references to `GtkWindowControls` buttons**; they are rebuilt on state changes.
9. Striping via the cell's grandparent is fragile; consider a custom cell widget that paints the stripe from the position, or watch for a row-widget accessor in a future GTK.
10. Keep cells cheap: plain labels, one custom-drawn rating cell. 12 columns cost 30–40 % more main-thread work than 8.

## Files

- `src/main.rs`: arguments, window, scroll script, report.
- `src/model.rs`: the lazy ID model and its item objects.
- `src/catalog.rs`: synthetic tracks and albums.
- `src/table.rs`: the column view.
- `src/grid.rs`, `src/covers.rs`: the grid view, the synthetic covers and the threaded thumbnail loader.
- `src/theme.rs`: stylesheet providers, scheme and accent following, the stand-in Settings portal.
- `src/shell.rs`: the window without a title bar.
- `src/inputtest.rs`: pointer input through mutter's remote-desktop API.
- `src/stats.rs`: frame-clock recording, percentiles, RSS.
- `style/helikodu.css`: the design's stylesheet with the fixes from this spike.
- `measure.sh`: runs the measurements; `results/`: raw output.
