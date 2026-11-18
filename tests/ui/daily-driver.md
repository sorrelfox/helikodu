<!--
SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Daily-driver checklist

A manual pass that a person runs before a release, once per desktop:

| Session | How to start it |
|---|---|
| GNOME, Wayland | GNOME Shell login session (default) |
| KDE Plasma, Wayland | "Plasma (Wayland)" at the login screen |
| KDE Plasma, X11 | "Plasma (X11)" at the login screen |
| Sway | `sway` from a TTY or the login screen |

Copy the result table at the end into the release issue, one column per session. Mark each step **pass**, **fail** (with a note and, if possible, a screenshot) or **n/a**. Work with your own library if you can; otherwise use a synthetic one (see [Preparation](#preparation)).

Nothing in this checklist needs a network connection. Turn networking off for the run if you can: the player must not notice.

## Preparation

1. Install the build under test (package, Flatpak or `cargo build --release` with `target/release/helikodu`).
2. Start from a clean profile so the first-run assistant shows up. Either use a fresh user account, or move these aside and restore them afterwards: `~/.config/helikodu`, `~/.local/share/helikodu`, `~/.local/state/helikodu`, `~/.cache/helikodu`.
3. Have a music folder ready: your own, or a synthetic one made with `cargo xtask synth-library --tracks 100k` (silent files; good for size, useless for listening).
4. Have two output devices if you can (for example speakers and a USB DAC or headphones), so that device switching can be checked.
5. Note the session type: `echo $XDG_SESSION_TYPE $XDG_CURRENT_DESKTOP`.

## 1. First run and library

| # | Step | Expected |
|---|---|---|
| 1.1 | Start Helikodu from the application menu (not a terminal) | The window opens within about a second; the first-run assistant is shown |
| 1.2 | Add the music folder in the assistant and finish it | Scanning starts; the footer shows progress; the window stays responsive while it scans (scroll, open a menu) |
| 1.3 | Wait for the scan to end | Albums, Artists, Tracks and Genres show counts in the sidebar; covers appear in the album grid |
| 1.4 | Scroll the album grid quickly from top to bottom, then drag the scroll bar | No visible stutter; covers fill in shortly after scrolling stops |
| 1.5 | Open Tracks; scroll; click a column header, then a second one with Shift held | Rows scroll smoothly; sorting by one and by two columns works |
| 1.6 | Open Artists, then one artist; open Genres, then one genre; open Folders | Each view shows its content; Back (Alt+Left) returns along the history |
| 1.7 | Right-click a track, choose "Open containing folder" | The file manager opens the folder with the file selected |

## 2. Playing

| # | Step | Expected |
|---|---|---|
| 2.1 | Double-click an album tile | The album plays from its first track; the deck shows title, artist and the cover; the queue panel lists the tracks |
| 2.2 | Press Space, then Space again | Playback pauses and resumes within a moment, without a click |
| 2.3 | Ctrl+Right, Ctrl+Left | Next and previous track |
| 2.4 | Click and drag in the seek bar; Ctrl+Shift+Right | Playback continues from the new position quickly |
| 2.5 | Change the volume with the slider and with Ctrl+Up/Down | Volume changes smoothly |
| 2.6 | Turn shuffle and repeat on and off with the deck buttons | The buttons show their state; the queue order follows |
| 2.7 | Play an album whose tracks run into each other (a live or classical album) | No gap or click between tracks |
| 2.8 | Use the desktop's media keys and its media controls (GNOME: the notification area player; Plasma: the media player applet; Sway: `playerctl play-pause`) | Play/pause, next and previous work; the controls show the current title and cover |
| 2.9 | Start playing, then lock the screen for a minute and unlock | Playback continued; the system did not suspend while playing |

## 3. Search

| # | Step | Expected |
|---|---|---|
| 3.1 | Press Ctrl+F and type part of an artist name, letter by letter | Results update while typing, with no visible delay |
| 3.2 | Play a result with the keyboard only (arrow keys or Tab, then Return) | It plays |
| 3.3 | Type a word that matches nothing | The empty result page is shown, with a hint |
| 3.4 | Press Escape | Search is cleared; the previous view is back |

## 4. Queue

| # | Step | Expected |
|---|---|---|
| 4.1 | Right-click a track, "Play next"; on another, "Add to queue" | They appear in the queue panel at the right places |
| 4.2 | Drag a queue entry to another position | The order changes and playback follows it |
| 4.3 | Remove an entry; then "Clear" | The entry goes; Clear keeps only the playing track |
| 4.4 | Hide the panel with its button and show it again; press F6 several times | The panel toggles; F6 moves focus between sidebar, content, panel and deck |
| 4.5 | "Save as playlist" | A new playlist appears in the sidebar with the queue's tracks |

## 5. Playlists

| # | Step | Expected |
|---|---|---|
| 5.1 | Ctrl+N, name the playlist | It appears in the sidebar |
| 5.2 | Drag tracks from Tracks or an album onto the playlist in the sidebar | They are added; the count updates |
| 5.3 | Open the playlist; reorder by dragging; remove one track; Ctrl+Z | The order changes; the removal is undone |
| 5.4 | Rename and delete a playlist from its context menu | Both work; deleting asks first or offers Undo |
| 5.5 | Export the playlist as M3U8 and import it again | The imported playlist has the same tracks |

## 6. Tags

| # | Step | Expected |
|---|---|---|
| 6.1 | Select one track, Alt+Return | The tag editor opens with the track's tags, cover and file info |
| 6.2 | Change the title and save | The new title shows in the table and the deck. (Where the build cannot write tags yet, the editor says so and opens read-only: note which) |
| 6.3 | Set a rating with the stars | It is kept after a restart |
| 6.4 | Select several tracks of one album and open the tag editor | Shared values are shown; differing values are marked as mixed |

## 7. Output devices

| # | Step | Expected |
|---|---|---|
| 7.1 | While playing, open the output button in the deck and pick the other device | Sound moves to the other device within about a second, without a loud click |
| 7.2 | Unplug the device that is playing (USB or Bluetooth) | Playback pauses or moves as the settings say; a toast explains it; the lamp shows the device is gone |
| 7.3 | Plug it back in and choose it again | Playback resumes on it |
| 7.4 | Settings → Audio output: choose a device with fixed full volume, then play | The full-output confirmation is shown before any sound |

## 8. Restart and resume

| # | Step | Expected |
|---|---|---|
| 8.1 | Play a track, seek to the middle, pause; quit with Ctrl+Q; start again | The queue, the track and the position are restored, paused |
| 8.2 | Resize the panes and the window, hide the sidebar; restart | The layout is kept |
| 8.3 | While playing, kill the program (`pkill -9 helikodu`); start again | The session is restored paused; no data is lost; no crash loop |
| 8.4 | Start Helikodu a second time while it runs, with a file argument (`helikodu song.flac`) | The running window comes to the front and plays the file; no second window |

## 9. Settings

| # | Step | Expected |
|---|---|---|
| 9.1 | Ctrl+comma; walk every page | Every page opens; nothing is cut off at the default size |
| 9.2 | Appearance: switch Light, Dark and System; change the accent | The window follows at once, without a restart |
| 9.3 | Change the desktop's light/dark setting and accent colour while Helikodu is set to System | The window follows the desktop |
| 9.4 | Library: add and remove a folder | The library follows after a scan |

## 10. Window and desktop integration

The window has no title bar of its own in the integrated style: the strips at the top of the content and of the panel are drag areas, and the window buttons sit in the top right corner (or where the desktop's button layout puts them).

| # | Step | Expected |
|---|---|---|
| 10.1 | Drag the window by the top strip of the content area (left of the search field, or the empty part of the strip) | The window moves with the pointer |
| 10.2 | Drag the window by the top strip of the queue panel (beside the tabs) | The window moves |
| 10.3 | Double-click either strip | The window maximizes; double-click again restores it |
| 10.4 | Right-click a strip | The desktop's window menu opens (GNOME, Plasma) or nothing happens (Sway) |
| 10.5 | Click Minimize, Maximize, Close | Each works; Close quits or hides as the settings say |
| 10.6 | Change the desktop's window button layout (GNOME: `gsettings set org.gnome.desktop.wm.preferences button-layout 'close,minimize,maximize:'`; Plasma: System Settings → Colors & Themes → Window Decorations → Titlebar Buttons) | The buttons follow the new layout and side; restore the layout afterwards |
| 10.7 | Drag each window edge and corner | The window resizes from all of them |
| 10.8 | Switch to the classic window style (Settings → Appearance), restart if asked | The desktop's own title bar is used; Ctrl+M shows the menu bar |
| 10.9 | HiDPI: set the display scale to 125 %, 150 % and 200 % (fractional scaling where available) | Text and covers are sharp; nothing is cut off |
| 10.10 | Check the application menu entry, its icon and the window icon in the task switcher (Alt+Tab) | The Helikodu icon is shown everywhere |

### KDE Plasma (Wayland and X11): drag strips and window buttons

Do this part with particular care on both Plasma sessions: it is the check that cannot be automated.

| # | Step | Expected |
|---|---|---|
| K.1 | With the default Breeze decoration, look at the top right corner | Minimize, maximize and close are drawn like Breeze's and in Plasma's order |
| K.2 | Drag by the content strip, then by the panel strip; also with the panel hidden | The window moves in each case; dragging from a button, the search field or a tab does not move it |
| K.3 | Double-click a strip | It does what Plasma's "Titlebar double-click" setting says (maximize by default) |
| K.4 | Middle-click and right-click a strip | Plasma's titlebar actions for those buttons run (window menu on right click) |
| K.5 | Drag a strip to the top screen edge, and to the left and right edges | Plasma's quick tiling and maximize-on-drag work |
| K.6 | Alt+drag anywhere in the window | The window moves (Plasma's own modifier drag) |
| K.7 | System Settings → Window Decorations → Titlebar Buttons: put close on the left, add "Keep above" | The window buttons follow (buttons Plasma has and GTK does not, such as "Keep above", may be left out) |
| K.8 | Repeat K.1-K.3 with the Breeze GTK theme and with the Adwaita GTK theme (System Settings → Application Style → GNOME/GTK Application Style) | The window looks the same apart from the buttons |

### Sway

| # | Step | Expected |
|---|---|---|
| S.1 | Start Helikodu in a tiled workspace | It tiles like any window; no shadow or empty border around it |
| S.2 | Look for window buttons | Minimize is hidden or harmless; close works; maximize does not leave the window in a broken state |
| S.3 | `floating toggle`, then drag by the content strip | The floating window moves |
| S.4 | Media keys bound in the Sway config to `playerctl` | Play/pause, next, previous work through MPRIS |
| S.5 | Light/dark: change `gsettings set org.gnome.desktop.interface color-scheme prefer-dark` (or the portal's setting) | The window follows when set to System |

## 11. Screen reader (Orca)

Start Orca (Super+Alt+S on GNOME; `orca` elsewhere) and use the keyboard only.

| # | Step | Expected |
|---|---|---|
| 11.1 | Tab and F6 through the window | Orca names every control and its kind (button, toggle, slider, list item…) and the album tiles by album and artist |
| 11.2 | Browse: open Albums, move through the grid with the arrow keys, open an album with Return | Orca reads each album and the album page's tracks |
| 11.3 | Search: Ctrl+F, type, move to a result, Return | Orca reads the results; playback starts |
| 11.4 | Queue: F6 to the panel, move through the entries | Orca reads each entry and which one plays |
| 11.5 | Settings: Ctrl+comma, move through a page, change a switch | Orca reads the labels and the new state |

## 12. Keyboard only, every feature (with Orca)

Leave the mouse alone for the whole section and keep Orca running. Before it, run the automated check, which covers the names, roles and Tab order of every window listed here: `cargo xtask ui-checks --only a11y` (or `tests/ui/a11y.py`, see `tests/ui/README.md`). This section checks what it cannot: that what Orca says makes sense, in order, and that every feature can be used without a pointer.

For every step: the focus is always visible (a ring or a highlight); Orca says the name, the kind (button, toggle, list item, slider, menu item…) and the state (checked, selected, expanded) of what has the focus; Escape closes a menu, popover or window and puts the focus back where it was; no step needs the mouse. Note anything Orca reads wrongly, twice or not at all.

General keys: **F6** moves between the sidebar, the content, the right panel and the player bar; **Tab** and **Shift+Tab** move between controls and leave a list after its focused row; the arrow keys move inside lists, grids and tables; **Return** plays or opens; **Menu** or **Shift+F10** opens the context menu; **F10** opens the main menu; **Alt+Left** goes back.

| # | Feature | Keys | Expected |
|---|---|---|---|
| 12.1 | Main menu and menu bar | F10, the arrow keys into each submenu, Escape; Ctrl+M (Classic style), F10, Left and Right through the menu bar | Every item is read by its name, with its shortcut; submenus say they open |
| 12.2 | Help | F1 | The user guide opens in the browser from the installed copy (no network); its headings and links are reachable with Tab and the browser's heading keys |
| 12.3 | Home | F6 to the sidebar, Home, Return; Tab through the sections | Each album tile is read as "album by artist", a button; Return plays it |
| 12.4 | Albums, Artists, Genres, Composers, Years, Folders | Down to each in the sidebar, Return; F6 to the content; arrow keys; Return; Alt+Left | Grid and list entries are read with their names; the page opened is announced by its heading |
| 12.5 | Album page | Return on an album; Tab to Play, Shuffle and the track table; Down through the tracks; Return on one | The tracks are read with number, title and artist; Return plays |
| 12.6 | Artist page and its photo | Open an artist; Tab to the more button, Return; Down to "Set artist photo…", Return; choose a picture in the file chooser with the keyboard | The menu items are read; the file chooser can be used without a mouse; the photo shows |
| 12.7 | Tracks, facet browser and columns | Open Tracks; Tab to the facet browser button, Space; Tab through the three panes, arrows, Space; Tab to the columns button | The panes are read with their field names; choices narrow the table; Tab leaves the table after the focused row |
| 12.8 | Editing in the table | F2 on a track's cell, type, Return; Ctrl+Z | The cell editor is read as an entry; Undo is announced in a toast |
| 12.9 | Search and query search | Ctrl+F, type `genre:` and pick a completion with Down and Return, type a value, Return; Tab to a result; Return | The completion list is read; the results table is read; "Save search" can be reached and used |
| 12.10 | Queue, lyrics and signal path | F6 to the panel; Tab to the Queue, Lyrics and Signal path tabs, Space on each; Down through the queue; Delete | Each entry says which one plays; the lyrics' current line is read; the signal path stages are read in order |
| 12.11 | Player bar | F6 to the player bar; Tab through play, previous, next, the seek bar, shuffle and repeat, volume and the output button; Up and Down on the sliders | Sliders say their value; the output button says the device and the signal state, not a list of devices |
| 12.12 | Output switcher | Tab to the output button, Return; Down through the devices; Return | Each device is read with its kind; the chosen one is announced |
| 12.13 | Sleep timer and play modes | Open the sleep timer from the Playback menu; Tab through the choices and the minutes field; Return | Choices are read as buttons; the timer's chip in the player bar is reachable and can be cleared |
| 12.14 | Now Playing and the analyzer | F11; Tab through the segments (Lyrics, Queue, Analyzer); Space on Analyzer; F11 again | The segments are read as toggles; nothing moves while paused; with reduced motion on, nothing glides |
| 12.15 | Mini player | Ctrl+Shift+M; Tab through it; Escape or Ctrl+Shift+M | Controls are read; the main window comes back with the focus where it was |
| 12.16 | Playlists | Ctrl+N, type a name, Return; add tracks from their context menu (Menu key); in the playlist, Down, Delete, Ctrl+Z | Each entry is read with number, title and artist; Undo restores it |
| 12.17 | Smart playlist editor | "New playlist" row in the sidebar, Return, Down to "Smart playlist", Return; Tab through the rule builder: field, operator, value, add a rule, a group, the limit and the order; edit the query text | Every control of a rule says what it is; changes are announced through the track count |
| 12.18 | History and statistics | Library ▸ History; Tab to the range and the filter; Down through the days; Library ▸ Statistics | Day headings and listens are read with time, title and artist |
| 12.19 | Tag editor and tag tools | Select tracks, Alt+Return; Tab through the files and the fields; type; Tab to "Tag tools", Return, choose a tool; Tab through its options and table; apply; Tab to Save, Return | The files list reads number, title and file name; the tools list and the preview table are read |
| 12.20 | Organize files | Tools ▸ Organize Files…; Tab through the pattern, the insert menu and the preview; run it; Ctrl+Z | The preview rows read old and new paths |
| 12.21 | Find duplicates, copies | Tools ▸ Find Duplicates…; Tab through the kinds, the groups and the buttons; a copies badge's window from a track's menu | Groups are read with title, artist and count |
| 12.22 | Convert | Tools ▸ Convert…; Down through the presets (no preset is made by moving onto "New preset"); Return on "New preset"; Tab through format, quality, pattern and folder; Start, Stop | The new preset appears only after Return; progress is announced |
| 12.23 | Sync and export | Tools ▸ Sync and Export…; Down through the destinations (the folder chooser opens only on Return on "New destination"); Tab through the sources and the options; sync | Destinations are read by name; the plan table is read |
| 12.24 | Cover manager | Tools ▸ Cover Manager…; Down through the filters and albums; choose a cover with the keyboard | The albums are read with their cover state |
| 12.25 | Verify files, missing files | Tools ▸ Verify Files…, start, move through the results; Library ▸ Missing Files…, move through them | Results are read with path, problem and format |
| 12.26 | Settings, settings search and shortcuts | Ctrl+comma; Down through the pages; Tab through each page; type in the search field; on the Shortcuts page, Return on an action, press new keys, Escape | Every setting is read with its label and value; search results are read in groups; a shortcut row is read as "action: keys", a button |
| 12.27 | Keyboard shortcuts, supported formats, about | Ctrl+?; type in "Find an action or its keys"; Help ▸ Supported Formats, Tab to the filter and the families; Help ▸ About | The filter fields are read by their labels; each format family is read with formats, decoder and status |
| 12.28 | Tray menu | With the tray icon on, open its menu from the desktop's tray with the keyboard (on GNOME with the AppIndicator extension, or Plasma's system tray) | Every item is read: the playing track, Play/Pause, Next, Previous, Stop after this track, Show or Hide the window, Mini player, Quit |
| 12.29 | Contrast and motion | Turn on the desktop's high contrast and reduced motion (GNOME: Accessibility settings; Plasma: Colours and Animations) | Lines and secondary text get stronger; nothing slides, glides or fades; text stays readable on every surface in light and dark |

## Results

| Step | GNOME Wayland | Plasma Wayland | Plasma X11 | Sway |
|---|---|---|---|---|
| 1 First run and library | | | | |
| 2 Playing | | | | |
| 3 Search | | | | |
| 4 Queue | | | | |
| 5 Playlists | | | | |
| 6 Tags | | | | |
| 7 Output devices | | | | |
| 8 Restart and resume | | | | |
| 9 Settings | | | | |
| 10 Window and desktop | | | | |
| K Plasma drag strips and buttons | n/a | | | n/a |
| S Sway | n/a | n/a | n/a | |
| 11 Orca | | | | |
| 12 Keyboard only, every feature | | | | |

Tester, date, build (version and commit), distribution, GTK version (`helikodu --version` or the About window), screen scale:
