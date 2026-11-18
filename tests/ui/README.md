# UI tests

Smoke, visual and frame-time tests of the desktop application, run in a headless compositor.

Every script re-runs itself inside `dbus-run-session`, starts `mutter --headless` (or KWin's virtual backend for the visual check when mutter is missing) and, where needed, its own accessibility bus, so nothing touches the desktop it is started from. The program under test gets a throw-away profile (XDG folders in a temporary directory) whose settings select the **null output**: nothing is ever heard.

| File | What it checks |
|---|---|
| `perf.py` | Start-up (cold and warm), memory, frame times and idle CPU over a large library, against the performance budgets |
| `a11y.py` | Accessible roles and names of every control, and a keyboard-only path through the application, over AT-SPI |
| `visual.py` | The designed screens under Adwaita and Breeze, light and dark, against the design's reference images |
| `headless.py` | Shared helpers: the private session, the compositor, the accessibility bus, the profile, page-cache eviction |
| `daily-driver.md` | The manual checklist for GNOME Wayland, KDE Plasma Wayland and X11, and Sway |

All three checks at once, over a 100,000-track synthetic library:

```sh
cargo xtask ui-checks --design DIR          # DIR: the reference images, <Screen>.png
cargo xtask ui-checks --only perf --idle-secs 10
```

Requirements: mutter (`mutter --headless`), `dbus-run-session`, Python 3 with PyGObject and `Atspi` 2.0 (`python3-gobject`, `at-spi2-core`), Pillow and NumPy. A script whose requirements are missing says so and exits successfully.

## Performance: `perf.py`

```sh
cargo xtask synth-library --tracks 100k
tests/ui/perf.py --library target/synth-library/flac-100000-seed1
tests/ui/perf.py --database library.db --warm-runs 3 --idle-secs 60 --json perf.json
```

The measured program is `crates/ui-gtk/examples/perf.rs`. It starts the front end through `helikodu_ui_gtk::run`, as the application does, over the service opened from the profile, and measures from inside the process:

| Measurement | How |
|---|---|
| Start-up | From the moment the script starts the process (`CLOCK_MONOTONIC`) to the first frame the window paints, to the first frame painted after the album grid has its first rows, and to the first low-priority idle of the main loop after that ("interactive"). One cold run (an empty cache and state folder, and the database, the program and its libraries dropped from the page cache with `posix_fadvise`; libraries other processes keep mapped stay cached), then warm runs; the warm median is compared |
| Memory | `VmRSS` after start-up, after scrolling the album grid, after scrolling the track table, and the peak (`VmHWM`); for information, each split from `/proc/self/smaps_rollup` into pages only the process maps (`Private_*`) and pages shared with other processes (`Shared_*`), PSS and anonymous memory, with the resident pages of the GL driver's libraries (Mesa, LLVM) from `/proc/self/smaps` and the RSS without them |
| Frame times | A tick callback scrolls the album grid and then the track table every frame for five seconds each: slowly (12 px a frame), fast (900 px a frame) and in random jumps (as when dragging the scroll bar). Intervals come from the frame clock's frame times, work from the update phase to the end of the paint |
| Idle | CPU time of the process (`/proc/self/stat`) over 60 s while stopped, while paused, and while playing with the window hidden and minimized, with the frames the window's frame clock painted and the main thread's wake-ups meanwhile |

Budgets: first frame ≤ 1.0 s and interactive ≤ 1.5 s (warm); steady scrolling at the refresh rate (95th percentile interval within 10 % of the 16.7 ms of the 60 Hz virtual output) and no frame over 50 ms in any mode; RSS ≤ 250 MB (10⁶ bytes) with the album grid open; stopped or paused under 0.1 % of one core with no periodic work (no frames painted); playing with the window hidden or minimized with no drawing work (no frames painted).

Headless mutter has no GPU load and a virtual 60 Hz output, so frame times here show the application's own work; check them on reference hardware before a release.

## Accessibility: `a11y.py`

```sh
tests/ui/a11y.py --database library.db [--dump tree.txt]
```

Starts the real `helikodu` program with the accessibility bus of the private session, then:

- audits every object shown in the sidebar, header and search, the album grid, the track table, the queue panel, the player deck and each Settings page: interactive objects (buttons, toggles, entries, sliders, list items, table cells…) need a non-empty accessible name, and focusable objects need a role that says what they are;
- walks a keyboard-only path, typing through mutter's remote-desktop D-Bus interface: Ctrl+F, a query, Tab and arrow keys to a track result, Return (checked over MPRIS: `PlaybackStatus` is `Playing`), F6 to the queue panel, Ctrl+comma to Settings, Space on a switch (checked in `settings.toml`), Escape, and the sidebar to Tracks.

- then audits every other window and view the same way, opening each through the application's actions on the session bus (`org.gtk.Actions`, as its menu item does): every sidebar view and the first page each opens (album, artist, genre, folder), the facet browser, the tag editor and its Tag tools, a new playlist and the smart playlist editor, History and Statistics, query search and its completion, Now Playing and the analyzer, the main menu, the output switcher and the sleep timer, Settings with its search and its Shortcuts page, Keyboard shortcuts, Supported formats, About, every tool window (Organize, Find duplicates, Convert, Sync and export, Cover manager, Verify files, Missing files) and the mini player. In each it presses Tab two dozen times: focus must stay on a named object of that window and must move;
- serves a StatusNotifierWatcher of its own, so that the tray icon registers, and reads the tray menu over D-Bus as a desktop's panel would: every item needs a label.

Also flagged: a description over 300 characters (GTK builds one from what a widget holds, such as the rows of a popover). `--views` limits the sweep to some labels (as the report prints them), `--skip-path` and `--skip-sweep` leave out either part.

Contrast is checked by the unit test `style::color::tests::text_tokens_have_their_contrast` (every text token on every surface it is drawn on, 4.5:1); reduced motion by the style tests and by step 12.29 of the checklist.

It is a stand-in for the screen-reader test, not a replacement: run sections 11 and 12 of the daily-driver checklist with Orca before a release.

## Visual: `visual.py`

```sh
tests/ui/visual.py --design DIR --out target/ui-visual
```

Renders the screens with the `screenshot`, `views` and `dialogs` examples of helikodu-ui-gtk (sample content, 1440 × 900) once per `GTK_THEME`: `Adwaita`, `Adwaita:dark`, `Breeze`, `Breeze-Dark` (Breeze only when installed). It writes the renders per theme and scheme, a side-by-side image per screen in `diff/` (render, reference, difference), and `report.md` with the mean difference, the share of pixels that differ by more than 48, SSIM, and the third of the image that differs most. The application draws its own look, so a second table shows how much each screen changes between Adwaita and Breeze; that should stay close to zero.

The reference images are dark except `Main.light.png`. The sample content is not the content drawn in the design, so the numbers point at screens to look at; the side-by-side images show what differs.

## Manual: `daily-driver.md`

The checklist a person runs on GNOME Wayland, KDE Plasma Wayland and X11, and Sway before a release, including the drag strips and window buttons on Plasma and the screen-reader pass.
