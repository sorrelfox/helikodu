# Changelog

All notable changes to Helikodu are recorded in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- On an album page, the facts of the selected track come first in the details column, above the album's.

### Fixed

- The **Most played** lists of artist pages and of Home had no context menu; a right click (or the Menu key) now opens the menu of tracks.
- The Labels list opened from **Labels…** closed again at once when it had only a few rows, as before the first label exists. The list of completions in the search filter could close the same way.

## [0.9.4] - 2026-11-20

### Added

- Helikodu analyses your library tracks in the background after a scan: where each track's sound starts and ends, its silent stretches and how its loudness changes. It works on one track at a time at the lowest priority, slows down while music plays from the same disk, pauses during scans and on battery, and carries on where it stopped after a restart. Nothing is written to the files. Tracks without any loudness tags are measured in the same pass, albums as a whole, so they are evened out like tagged ones. Playback uses it (see below). `helikodu-cli analyze` runs it from the command line.
- Naming patterns are now written in an expression language: the braces can hold functions, conditions, comparisons and names bound with `let`, such as `{if(composer, composer, artist)}`, `{initial(noarticle(albumartist))}` or `{num(raw(track), 3)}`. Every pattern written before reads and names files exactly as it did. The organizer, the converter and folder sync all use it, and a new page of the user guide, Expressions, describes it.
- Playback uses the track analysis. Crossfades between analysed tracks follow the music: they start where the ending track fades out and skip the silence between the two, never longer than the length you set. Settings can also leave out the silence before and after tracks, jump over long silence inside a track (before a hidden track, for example), and even out tracks without loudness values as they play with a leveller. Tracks of one album that follow each other are never trimmed, and bit-perfect playback turns all of this off.
- Moving from another player: `helikodu-cli import` takes over play counts, ratings, loved tracks and playlists from Rhythmbox (automatic playlists as smart playlists where their rules fit), Strawberry, Clementine, Lollypop and an iTunes or Music `Library.xml`. It only reads the other player's files, shows what would change before `--apply`, matches by path (with `--map` for moved or Windows folders) and then by tags, adds only new plays when the same library is imported again, and `undo-import` undoes it. See the new user guide page "Moving from another player".
- `helikodu-cli carry-stats` carries the plays, rating, history and playlist entries of a file that was replaced by another copy of the same recording (for example an MP3 by a FLAC) to the new file.
- `helikodu-cli export-user-data` and `import-user-data` move ratings, play counts, history, playlists, smart playlists, Favourites, saved searches, labels and computed and custom fields between installations as a documented JSON file. An import adds labels and fields without taking any away, keeps a field the library defines differently, and is undone with `undo-import`.
- Play counts can be written into the files (**Settings › Advanced › Statistics in files**), as the `POPM` counter and `FMPS_PLAYCOUNT`, in the background a few minutes after plays rather than after every play (never into the file that is playing, never during a scan, and briefly when Helikodu closes); `helikodu-cli write-stats` writes the changed play counts and ratings now.
- A managed media folder, off by default: `helikodu-cli managed-folder` sets a folder of the library and a naming pattern, imports files and folders from elsewhere (a downloads folder, a memory stick) into it by copying or moving them, with their covers, booklets, rip logs, lyrics and CUE sheets (whose `FILE` lines are changed to the new names), and consolidates the library files that are elsewhere into it, keeping their ratings, play counts and history. `--dry-run` shows every file and where it goes, names that are taken, and the space needed and free. Nothing is ever replaced, a copy is checked before an original is removed, and a run cut short continues or rolls back without losing a file.
- The managed folder in the window: its group in Settings ▸ Library (the folder, the names, copy or move, the characters in names), **Add Music to the Managed Folder…** in the main menu and **Gather into the Managed Folder…** in the Library menu, and files dropped on the window while it is on. A window shows the plan with its figures and every file before anything changes, and a notice when the files do not fit.
- A run of the managed folder works in the background with its progress and **Stop**, in the window, the sidebar and Settings; afterwards **Show in the library** opens Tracks at the folders the music went to. A finished run has no undo.
- A run of the managed folder that was cut short is announced by a notice with **Details…**, which opens a dialog to continue it or roll it back.
- Labels: free words such as `late night` or `to sort` given to tracks, albums and artists, kept in the library and never written to the files. A label of an album or an artist counts for all of its tracks, and labels stay through moves, renames and rescans. `label:jazz`, `label=jazz` and `has:label` find them, and plain search text does too. `helikodu-cli labels` lists, gives, takes, renames and merges them.
- Labels in the window: **Labels…** in the menu of tracks, albums and artists and **Add label** on album and artist pages give and take labels, with one **Undo** for everything changed while the list was open. A Labels view (in the sidebar once a label exists, and in **Library ▸ Labels**) lists them with what each was given to, plays them, and renames, merges and removes them, each with **Undo**. Album and artist pages show their labels.
- Your own fields: a computed field is a name for an expression (`decade` = `{left(year,3)}0s`), a custom field a name for a tag (`mood` = `MOOD`, also written `TXXX:MOOD` or `----:com.apple.iTunes:MOOD`). Both can be named in searches, smart playlists and naming patterns, such as `decade=1990s` or `{decade}/{album}`. `helikodu-cli fields` defines them, and a new page of the user guide, Labels and your own fields, describes both.
- Settings for the track analysis and what uses it: **Library › Track analysis** turns the analysis in the background on or off and shows how many tracks are analysed and what it does now; **Playback** has **Start the crossfade where the track fades out** and a **Silence** group (leave out silence at the start and end of tracks, skip long silence inside a track); **Loudness** has the **Leveller**, and a box at the bottom of the page shows the signal path of the playing track; **Advanced › Statistics in files** can also write play counts into the files. The settings search finds them all.
- The **Tracks** table can show a **Labels** column (a track's own labels and those of its album and artists) and a column for each of your own fields, from **Columns ▸ Your fields**. A computed field's column cannot be sorted; a custom field's column sorts like any text column.
- The **Browse** lists of **Tracks** can list labels: choose **Label** in a list's head.
- The search field's completion offers `label` with your labels after `label:`, and your own fields among the others, with the values of a custom field when they are few.
- The Import window (**Library → Import from Another Player…**) shows what importing another player's library or a Helikodu data file would change, with the folders that moved, the entries left out and the playlists, and imports it; an import is undone from the window or its message, not with Ctrl+Z.
- Missing files has a "Replaced" section that carries the plays, history and playlist places of missing tracks to their new copies.
- **Library → Export Your Data…** and the "Your data" row of Settings → Advanced save and import your data file.
- **Library ▸ Your Fields…** defines your computed and custom fields in a window: the name, the kind, the expression or the tag, and example values for the selected tracks or a search that follow the expression as it is typed. A problem is underlined and explained with its place, and a saved field is in use at once.
- Every pattern and expression entry has an **Insert** button: a searchable list of the fields, your own fields and the functions with an example each, which puts the choice at the cursor and selects a function's first value. It replaces **Insert field** in Organize files, and Convert and Sync and export have it too.
- The tag editor's File info tab shows what the track analysis found: a curve of the track's loudness with its silences and the start of its fade-out, where the sound starts and ends, the silences inside it and where it fades out, or that the track is not analysed yet or could not be read. The tags of the file are now a row of the File group.
- The Tracks view can be grouped with **Group by**: by album artist, album, genre, year, format, label, one of your own fields (computed fields too) or an expression. Group rows show the value and the number of tracks, open and close with a double click, **Enter**, **Left** and **Right**, and their context menu acts on all of their tracks.
- After the first scan, when the library of another player Helikodu can import from is found on the computer, a notice offers to bring its plays and ratings over, once.

### Changed

- The album page's details show the record label of the files' tags as **Record label**.
- With **Restore the last session**, music that was playing when you closed or quit Helikodu now continues where it stopped when you start it again, with a short fade in; music that was paused or stopped stays paused. The track is selected in the queue panel and scrolled into view. Files you open with Helikodu play instead, and a track whose file is missing or that was playing when Helikodu ended abnormally stays paused.
- The memory budget with 100,000 tracks is 270 MB RSS instead of 250 MB. The last measurement, 264 MB after scrolling through the whole track table, is within it, so this is no longer a known issue.
- The **Analysis data** limit under **Settings › Library › Storage** is now called **Waveforms**: it holds the pictures of the seek bar.

### Fixed

- WAV files whose RIFF INFO tags are in a legacy charset, such as Windows-1251 as old Windows taggers wrote them, no longer seem untagged: their title, artist and album are read and the charset is repaired like that of other old tags.

## [0.9.3] - 2026-11-19

The release candidate with Helikodu's own Flatpak repository and fixes for narrow windows and the tray menu.

### Added

- Helikodu's own Flatpak repository, published with the website: **Install with Flatpak** on the website (or `flatpak install --user --from https://sorrelfox.github.io/helikodu/helikodu.flatpakref`) installs Helikodu from it, and new versions then come with the other Flatpak updates. The repository is signed. The `.flatpak` file of a release adds the same repository when it is installed, so software centres no longer say that it comes without a software repository. An installation from the file of 0.9.2 or earlier is switched to the repository as the user guide's installing page describes.

### Changed

- Helikodu uses less memory: Now Playing is built when it is first opened, large sorts of the track table use temporary files instead of memory, and artists' photos are looked up only on the Artists page.

### Fixed

- The tray menu in KDE Plasma could show items in the wrong form after it changed (another track, Play turned into Pause): a check box on Previous, entries drawn as headings, Quit twice. Plasma keeps menu items by their number and updates only what it is sent again; each change of the menu now numbers its items afresh.
- The smart playlist editor fits the room it has: in a narrower window, or with the queue panel open in German and French, its right edge (Done editing, the menu button and the matching tracks) was cut off. The rules pane gives way a little first, and when the page is too narrow for both, the matching tracks go under the rules.
- Library statistics no longer cut short the large figures (Size, Playing time, listened) in a narrower window: the tiles and the listening figures continue on further rows, and Formats and Quality, and the most played artists and albums, go one under the other.
- Page titles such as History, Results, Tracks and Library statistics are no longer cut short in a narrower window: the count beside the title gives way first, the Tracks filter gets narrower, and the controls of History and of query results continue on a second line.
- With the sidebar hidden, the queue panel stays beside the content as long as there is room for both, instead of opening over it as if the sidebar were still there; showing or hiding the sidebar lays the panes out again.
- Search results no longer run off the page in a narrow window: the query syntax and saved searches go under the results when there is no room beside them, and the table of a query fits a narrow page in every language (its Length column was cut off in a 1440-pixel window with the queue panel open).
- Artists, Genres, Composers, Years and Folders fit a narrow window: the list gets narrower down to 200 pixels, and the buttons of the page's header go under the title instead of cutting off its right side or shortening the name to a few letters. A long artist name continues on a second line.
- The album and audiobook pages fit a narrow window: the format badges continue on a second line, the Details column goes under the tracks, and the cover no longer stretches into an empty frame when the buttons take several lines.
- Identical copies found in one scan get their main copy by the documented rule, the shorter path and then the lower one, instead of by whichever file the scan happened to write first.
- Albums fits a narrow window in French: its quick filter and sort button continue on a second line. A playlist's name is no longer shortened to a few letters in a narrow window: its buttons go under it.

## [0.9.2] - 2026-11-19

The release candidate with fixes found while preparing the Flathub submission, and screenshots for Flathub and the software centres.

### Added

- Screenshots in the AppStream metainfo, with captions in every language, for Flathub and the software centres. `docs/site/shots.py --store` draws them over the sample library, in the default integrated window style.
- Helikodu says which permissions the Flatpak asks for and what each one is for: in the Flatpak on the first-run page, before anything is scanned, and under About ▸ Permissions; in its description in software centres and on Flathub; on the website's download section, in the README and on the Installing page of the user guide. The website's download section and the README also list every way Helikodu is published, since 0.9.1 more than the archives.

### Fixed

- The ring around the cover of the album that plays is drawn as designed, a gap in the background colour and then the accent ring, and it is no longer cut off along the top of the first row of the Albums page.
- The Favourite button's heart is Helikodu's own icon. GNOME's icon theme has no heart, so on GNOME and in the Flatpak the button showed a stand-in picture instead of the outline heart seen on KDE Plasma.
- With Now Playing open, the window could not be made shorter than about 740 pixels, and opening Now Playing made a shorter window taller: the cover and the analyzer kept their full height. The cover now shrinks with the window, and the analyzer scrolls like the lyrics and the queue.

## [0.9.1] - 2026-11-18

The same release candidate as 0.9.0, published in more ways: a Flatpak, an AppImage, packages for Fedora, Debian, Ubuntu and Arch Linux, and archives for any distribution. The known issues of 0.9.0 still apply, except that the Flatpak, the AppImage and the distribution packages no longer need FFmpeg 7.1.

### Added

- An AppImage for x86_64 and aarch64 on the Releases page: one file that runs without installing, with its own minimal FFmpeg, GTK and the decoders for tracker modules, game music, SID tunes and game audio streams (vgmstream). `--cli` starts the command-line tool. It never looks for updates.
- The Flatpak as a file to install directly (`helikodu-<version>-x86_64.flatpak` and `-aarch64.flatpak`); it brings the GNOME runtime from Flathub with it.
- A Fedora 45 RPM package for x86_64 and aarch64, built against Fedora's own libraries.
- Packages for Debian 13 and Ubuntu 26.04 LTS (`.deb`, for x86_64 and 64-bit ARM), installed with `sudo apt install ./helikodu_<version>-1.debian13_amd64.deb` or the Ubuntu file.
- An Arch Linux package (`helikodu-<version>-1-x86_64.pkg.tar.zst`), built with the libraries of the Arch repositories.
- The ready-built programs are also published as `.tar.gz` archives, next to the `.tar.xz` ones.
- The Installing chapter of the user guide says which file to use on which system.
- `helikodu --version` prints the version.
- The `helperdir` Meson option chooses the folder the scan worker is installed to, and the programs look for it there too. The default is `<libexecdir>/helikodu`; until now it was always `<prefix>/libexec/helikodu`, whatever `libexecdir` said, and `bindir` had to be `<prefix>/bin`.

## [0.9.0] - 2026-11-18

The release candidate. This version brings everything planned for 1.0: smart playlists and a query language, play modes, lyrics and a play history, tools that convert, organize, sync and repair files, the interface in eleven more languages, and the work of making Helikodu robust, fast with large libraries and accessible. With it the features are frozen for 1.0; until then only fixes, translations and documentation follow.

### Added

#### Playback
- Play modes in the **Playback** menu: shuffle by tracks, albums, folders or groupings, or within each album, with **Previous** retracing the order that played; repeat this track, this album or the whole queue; stop after this track, this album or the queue; play only this artist or this album; remove tracks from the queue after they play; continue from the selected track; previous, next and random album. The deck shows the active modes.
- An A-B loop that repeats a section of a track without a gap, and a sleep timer that pauses after a time (fading out) or at the end of the track, each with a chip on the deck.
- Per-track settings in the tag editor's **File info**: leave out when shuffling, play only when started by hand, and a volume of its own.
- Lyrics: synced and plain lyrics from a `.lrc` or `.txt` file beside the track (any charset) or from its tags (`SYLT`, `USLT`, `LYRICS`, `UNSYNCEDLYRICS`, `©lyr`, APEv2 `Lyrics`, and the `TXXX` frames FFmpeg writes), in the Lyrics tab of the right panel. The line sung stays in the middle; a click on a line plays from there; each track's offset can be moved in steps of 0.1 s.
- Now Playing (F11, or a click on the deck's cover): the cover, the track, the synced lyrics or the queue, the signal path from the file to the device, and an analyzer with a spectrum and a peak and RMS meter for each channel.
- A waveform seek bar: its bars follow the loudness of the track. Peaks are computed in the background, never while the library is scanned, and kept within the **Analysis data** limit.
- Chapters: while a chapter plays, the deck shows its number and title, and the seek bar has a gap at each chapter boundary.
- Audiobooks: an audiobook's page lists its chapters with where each starts and which have been heard, shows how much is left, continues where you stopped and can start again. Audiobooks are left out of shuffle and of **Play all** in other views. The kind of an album (music, audiobook, audio from a video) can be changed on its page.
- The limiter's editor in **Settings ▸ DSP and EQ** shows its activity while music plays.
- Playback pauses before the computer goes to sleep (on by default) and, if you choose, when the screen locks.

#### Library and browsing
- Home: the albums added and played lately, the most played tracks, and albums to rediscover.
- Composers and Years in the sidebar: the albums of a composer, and of a year or a decade (by the original release year where the tags give one), with **Play all** and **Shuffle**.
- A facet browser in Tracks (**Browse**): linked lists of genres, album artists and albums (each list's field can be changed) that narrow each other and the table.
- The Tracks table can show **Genre** and **Added** columns, and tags can be edited in it: F2 edits a cell, for all selected rows at once.
- Album page: a selected track shows its own facts (composer, format, gain, plays, lyrics, copies, size, file and more) below the album's details.
- Artist photos: an `artist.jpg` in the artist's folder, or a picture you choose or drop onto the artist page. Only local files are used.
- Play history (**Library ▸ History**): every listen by day with its time, source and listening time, skips dimmed, with a filter and **Save as playlist**. **Library ▸ Export History…** writes it to a local file as ListenBrainz listens (JSON Lines) or a `.scrobbler.log`; nothing is sent.
- Library statistics (**Library ▸ Statistics**): counts, playing time and size, formats and their quality, listening this year, and the most played artists and albums, with **Copy as text**.
- Copies of the same file are recognised: byte-identical files share one set of play counts, ratings, history and playlist entries, and play from a main copy. `[library.duplicates]` keeps copies out of the library or shows only the main copy. Copies and other duplicates can be moved to the desktop trash after safety checks; `helikodu-cli duplicates` lists them. Before play counts of copies are first combined, the library is backed up once to `library.db.backup-before-copies`.
- Folders outside the library: the Folders view also shows the home folder, connected drives and the file system, plays from them without storing anything, and adds a folder with **Add to library**.
- Tags without a field of their own (`MOOD`, `BPM`, `GROUPING`, `LABEL`, `COMMENT` and others) are stored when files are scanned, so `tag:NAME` queries find them; files scanned by 0.2.0 get them the next time they are read.

#### Playlists
- Smart playlists: rules built with a rule builder that shows the same rules as query text (either can be edited), groups of rules, a limit by tracks, time or size, and an order; their tracks follow the library as it changes.
- Favourites: the heart on the deck adds the playing track to **Favourites**, a playlist always first under **Playlists**. A playlist you named "Favourites" before becomes it.
- Playlist folders, and pins for albums, artists and playlists at the top of the sidebar.
- Undo and redo of every playlist change (Ctrl+Z, Ctrl+Shift+Z), a lock against changes, removing duplicates and missing tracks, shuffling the order for good, and a display grouped by album.
- Playlist files (M3U, M3U8, PLS, XSPF) in the library folders can become playlists that follow their files (`[library] import_playlists`).

#### Search
- A query language: text with fields such as `genre:jazz -genre:fusion`, `tag:mood=calm` or `added<30d sort:added desc` lists the tracks it selects, with completion of field names and values and a notice with an example when part of a query cannot be read. The Tracks filter takes queries too.
- Saved searches, with example queries, in the right column of the search page.

#### Tools
- **Tools ▸ Convert…**: converts tracks to Opus, MP3, AAC, Vorbis, FLAC, ALAC or WAV with presets of your own and naming patterns, copying tags and covers, several files at a time in the background.
- **Tools ▸ Sync and Export…**: keeps a folder (a phone, a memory stick) in step with chosen playlists, searches, albums and artists, converting lossless files on the way, writing the playlists as `.m3u8` and removing only files Helikodu put there. A playlist's **Export** copies its files to a folder.
- **Tools ▸ Organize Files…**: moves and renames files by a naming pattern, with saved patterns and a preview of every file before anything moves; files are never replaced. Every run is journaled, so it can be undone and redone even after a restart, and a run cut short by a crash is finished at the next start.
- **Tools ▸ Cover Manager…**: finds albums without a cover, with small covers or with covers that differ between files, and puts pictures already on the computer into the files or beside them as `cover.jpg`.
- **Tools ▸ Verify Files…**: decodes files from start to end and reports damaged parts, files that stop early, wrong lengths and FLAC audio that does not match its checksum.
- **Library ▸ Missing Files…**: finds the files of tracks that are gone elsewhere, by size and content, and relinks them with their ratings, play counts and playlist places; tracks not found can be removed, with Undo.
- Tag tools in the tag editor: tags from file names, track numbering, capitalisation, find and replace (also with regular expressions), repair of garbled text from old code pages, and sort names.
- Tag edits can be undone and done again (Ctrl+Z, Ctrl+Shift+Z, or Undo in the toast): each file gets back its tags, cover and lyrics, unless it was changed since. Outside a playlist, Ctrl+Z steps through one history of tag edits and file moves, with a notice saying what was undone.
- **Rename file…** and **Move to trash…** in the context menu of tracks; a rename can be undone.
- The organizer, the converter and folder sync read naming patterns the same way, so one pattern names a track the same in each.
- The tag editor's Cover tab takes a dropped picture, saves the front cover as a file, and can put a new cover as `cover.jpg` in the album folder.
- Ratings can be written into the files (**Settings ▸ Advanced**, off by default).

#### Desktop integration
- A mini player (Ctrl+Shift+M): the cover, the track, the seek bar and the controls, with a drawer for the queue and the lyrics.
- A tray icon (StatusNotifierItem) with the playing track, the controls, the window, the mini player and Quit. It is off by default; closing the window quits unless **Closing the window** asks to keep playing in the tray and a tray shows the icon.
- Keyboard shortcuts can be changed in **Settings ▸ Shortcuts**, with a warning when keys are in use, and global shortcuts work in other applications through the desktop's GlobalShortcuts portal.
- Settings can be searched.
- A narrow window: below the widths that leave the content 560 pixels, the queue panel and then the sidebar open over the content; below 720 pixels the deck is compact.
- MPRIS: the track list (the queue) and the playlists.
- `helikodu-cli` controls the running application: `status`, `queue`, `playlists`, `play-pause`, `pause`, `resume`, `stop`, `next`, `previous`, `seek`, `volume`, `shuffle`, `repeat`, `enqueue`, `play-query`, `rate` and `export-playlist`, with `--json` where they print, through a documented D-Bus interface, `io.github.sorrelfox.Helikodu.Control1` (`docs/dev/dbus-api.md`).
- Backups of the library: one a day, the last 7 kept, with **Back up now** and **Restore…** in **Settings ▸ Advanced**. A damaged library database is replaced with the newest good backup at start; one that cannot be written opens read-only; one that cannot be opened leaves Helikodu playing files without a library. The window says what happened.
- Local crash reports: after a crash, the next start says so, with the report behind **Details**. Nothing is sent anywhere.
- A log on disk with **Normal detail** or **Full detail**, rotated within the Logs storage limit (**No logs** writes nothing), and **Copy diagnostics** with the build, the output devices and the state of the library database.

#### Help and docs
- **Help ▸ User Guide** (F1) opens the user guide installed with the application, every chapter and the FAQ on one local page, so it opens without a network.
- A manual page for `helikodu-scan-worker`, and the new commands in the manual page of `helikodu-cli`.
- Ready-built programs for 64-bit ARM (aarch64) on the GitHub Releases page, beside the x86_64 build.
- A [website](https://sorrelfox.github.io/helikodu/) that shows what Helikodu does, with screenshots in both colour schemes, and carries the user guide and these release notes.

#### Languages
- Translations into German, Spanish, French, Italian, Japanese, Korean, Dutch, Brazilian Portuguese, Russian, Ukrainian and Simplified Chinese.
- **Settings ▸ Appearance ▸ Language** chooses the language of the interface, or keeps the desktop's. A new language is used from the next start (**Restart now**); the library is sorted for it at once, in the background, and `helikodu-cli` sorts the same way. Numbers are written with the language's decimal separator.

#### Packaging

- A Flatpak manifest (`packaging/flatpak/`) to build Helikodu as a Flatpak yourself, with vgmstream, libopenmpt, libgme and libsidplayfp built in and no network access; see **Installing** in the user guide.

### Changed

- The player deck follows the colour scheme: light in the light scheme, with its popovers, seek bar and output readout, and dark as before in the dark scheme.
- The **Follow the playing track** setting of the View menu is remembered between runs.
- HES (PC Engine) game music is no longer played: libgme's HES emulator reads outside its memory on damaged files.
- Large libraries are faster: the album grid, the Tracks filter and long lists no longer read their rows page by page (at 500,000 tracks start-up with albums went from 2.4 s to 1.0 s), rows scrolled past are no longer loaded, and a first scan uses far less memory (1.4 GB to 0.34 GB at 500,000 files). Pages and menus are built when first opened, which takes some 2,600 widgets out of the window after start-up.

### Fixed

#### Playback and formats
- DSD played as DoP no longer makes the DAC leave DSD mode or click when pausing, seeking, waiting for the next track or at the end: the device gets DSD silence instead of PCM silence.
- Damaged files no longer stop the program: ADX files with a header FFmpeg cannot play, Opus, MP4 and other files whose headers made the tag reader fail, SID tunes whose header points past the end of the file, MIDI files without tracks and DSDIFF files with an impossible property size are refused or read safely.
- NSF tunes whose header asks for the play routine thousands of times a second no longer make playback and seeking take many seconds per second of music.

#### Library and interface
- The playing track is marked the same way in every track list, wherever playback was started, with the playing indicator in the accent.
- With a colour scheme chosen in Helikodu that differs from the desktop's, text no longer takes the desktop theme's colours (light on light or dark on dark under themes such as Breeze).
- Album covers in the album grids no longer stay placeholders when the grid gains a column.
- The album page's **Details** column wraps its text instead of growing wider; its width can be dragged and is kept between runs.
- Invisible padding around tag values (spaces, no-break spaces, tabs, zero-width characters) is trimmed when files are scanned and when titles are shown.
- In the Tracks table, a column that was shown no longer takes the padding of the table's edge from the first and last columns.
- Supported formats, opened from About, can be closed while About is open.
- A library database on a read-only file is opened read-only instead of failing on each change, and temporary files of settings saves cut short are cleared.

#### Accessibility
- Screen readers: the items of every menu have their names (with GTK 4.24 they were read without one); the rows of Supported formats, the tag editor's files, the playlists and the Folders tree have names; the search fields of Keyboard shortcuts and Supported formats have labels; the output button is no longer described with every device's capabilities.
- Keyboard: Tab leaves the track table, the album grid and the other long lists after the focused row instead of visiting every row; Shift+Tab goes back the same way.
- Reduced motion also stops the slide of the notices in Settings and the tag editor and the other remaining animations when the desktop asks for it through its settings portal.

### Known issues

- With a library of 100,000 tracks, memory use reaches about 264 MB after scrolling through the whole track table in the headless measurement, above the 250 MB budget (some 61 MB of it is the software GL driver's, which a GPU driver replaces). Start-up and the album grid stay under 250 MB. See `docs/dev/performance.md`.
- Some narrow column headers (Plays in the copy tables, Length in the folder table) are shortened in the longer translations; their tooltips show the whole word.
- vgmstream (game audio streams) is off in the default build and in the ready-built programs.
- No SoundFont is included; MIDI files play only with a SoundFont you choose or one installed on the system.
- HES (PC Engine) game music is not played (see Changed).
- A screen-reader walkthrough with Orca and the manual checks on real GNOME, KDE Plasma and Sway sessions (`tests/ui/daily-driver.md`) have not yet been done by people; only the automated interface and accessibility checks have run.
- The review of the smart playlist editor by outside testers is still to come; the editor may change before 1.0.
- The bit-perfect check through an ALSA loopback device has not been run; bit-perfect output is verified through the WAV file output only.
- The facet browser takes about 0.6 s to list the values of the whole library at 500,000 tracks.
- The ready-built programs need the FFmpeg 7.1 libraries (`libavformat.so.61`, `libavcodec.so.61`, `libavutil.so.59`); on a system with only a newer FFmpeg, build from the source tarball instead.
- The ready-built programs also need libopenmpt, libgme and libsidplayfp from the system (`libopenmpt.so.0`, `libgme.so.0`, `libsidplayfp.so.6`) and do not start where one of them is not installed. The installing notes now name them; those of 0.2.0 did not.

## [0.2.0] - 2026-11-18

The first beta. This version is about the sound: an equalizer and other processing, crossfades, loudness measurement, a view of the signal path, more output backends and the long tail of formats.

### Added

#### Sound processing
- A DSP and EQ page in the settings with a chain of stages, each switched on separately and applied while music plays without a click: graphic equalizers of 10 and 31 bands with presets, a parametric equalizer of up to 64 filters with import and export of AutoEQ's `ParametricEQ.txt`, crossfeed for headphones with three presets, a channel mix (mono, balance, swapped channels, the LFE channel of surround files) and a true-peak limiter. Dither is added automatically where needed. With every stage off the samples pass untouched.
- The limiter can be chosen to prevent clipping instead of lowering the gain.

#### Transitions
- Crossfades between tracks and when you skip, with equal-power gains and exact to the frame. Tracks of one album stay gapless, and so does bit-perfect playback in exclusive mode.
- Adjustable fades for pause and resume, stop, seek and skip.

#### Loudness
- Tracks and albums are measured following EBU R128 (loudness, loudness range, sample and true peak), CUE tracks included, in the background with progress and a stop button, from the Loudness settings or a track's or album's menu, and with `helikodu-cli loudness`. Measured values take precedence over tags and are measured again when a file changes; writing them to the tags (ReplayGain 2.0, and R128 in Opus files) is an option.

#### Output
- The Signal path tab of the right panel shows a lamp (bit-perfect, DSP active, converted), every stage from the file to the device and live playback counters, and copies them for a bug report. The output button shows the format the device was actually opened with, and the output switcher lists each device's rates, formats and exclusive support.
- 16-, 24- and 32-bit sources play bit-perfect: 32-bit integer samples reach the device without passing through floating point.
- PulseAudio and JACK output.
- A DAC warm-up setting, a resampler quality setting, the time after which a paused exclusive device is released, and per-device settings applied when a device opens.
- Playback can pause when headphones are unplugged.

#### Formats
- Tracker modules (libopenmpt), game console music (libgme) and Commodore 64 SID tunes (libsidplayfp, with the HVSC song-length database when you point to it) play, and MIDI files with a SoundFont you choose or one your distribution installs. Game audio streams play through vgmstream in builds made with it. Files are recognised by their content, and the subsongs of a module, game or tune become tracks of their own in the library.
- Looping game music plays its loop twice and fades out; the number of loops, the fade and the length of tunes without one are settings.
- DSD files can go to a DAC as DoP when nothing on the way changes the samples; otherwise they are converted to PCM as before.
- Files with several audio streams can play another one.
- Tracks can be read into memory whole before they play, for slow or sleeping drives (off by default, never above 512 MiB).

#### Other
- Logs can be switched off, which is the default in release builds.
- A user guide page on the equalizer and sound processing; the guide covers crossfades, loudness measurement, the signal path and the new formats.

### Changed
- With the window style left on Auto, KDE Plasma gets the integrated window style like GNOME.

### Fixed
- Context menus open at their full height instead of scrolling, and scrollbars no longer cover the counts in lists.
- Album grids outside the Albums view, such as the Genres page, show their covers.
- A window narrower than a page scrolls the page sideways and keeps the sidebar and queue panel at their width.
- Rebuilding a list while its context menu was attached could hang the application.
- Ratings update every visible row and the now-playing panel at once.
- Every track list plays on double-click or Enter and selects on a single click.
- Popovers and drop-down lists no longer draw an opaque frame.
- The artist page's table fits its headers, and the playing indicator moves while music plays.

### Known issues

- The bit-perfect check through an ALSA loopback device has not been run for this release; bit-perfect output is verified through the WAV file output only.
- vgmstream (game audio streams) is off in the default build and in the ready-built programs.
- No SoundFont is included; MIDI files play only with a SoundFont you choose or one installed on the system.
- The limiter's editor does not show its activity yet.
- Logs are not written to disk yet.
- With a library of 100,000 tracks, memory use reaches about 260 MB after scrolling through the whole track table, above the 250 MB budget. Start-up and the album grid stay under 250 MB.
- The manual checks on real GNOME, KDE Plasma and Sway sessions and a screen-reader pass with Orca (`tests/ui/daily-driver.md`) have not yet been done by people; only the automated interface checks have run.
- The ready-built programs need the FFmpeg 7.1 libraries (`libavformat.so.61`, `libavcodec.so.61`, `libavutil.so.59`); on a system with only a newer FFmpeg, build from the source tarball instead.

## [0.1.0] - 2026-11-18

The first alpha. Helikodu plays and organizes the music on your own computer and never connects to the internet.

### Added

#### Playback
- Plays practically every audio format FFmpeg decodes, and the sound of video files, with every file opened under a lock-down that refuses network access, nested files and video decoders. Files are recognised by their content, not their name. DSD (DSF and DFF) plays through a built-in converter.
- Gapless playback: tracks that share a format join sample-exactly; seeking lands on the exact frame wherever the format allows it.
- Output through PipeWire or ALSA to a chosen device, shared or exclusive, bit-perfect where nothing needs to change the signal. The output device, sample-rate policy, sample format and buffer can be changed while playing. Device lists mark the system default; a lost device switches the output to "Not connected".
- Exclusive ALSA output takes the sound card from PipeWire through the device reservation protocol and gives it back afterwards.
- ReplayGain 2.0 and Opus R128 loudness normalization (off, track, album or automatic), with preamp and clipping prevention.
- CUE sheets (separate, embedded or in a FLAC CUESHEET block) and container chapters play as tracks of their own.
- A queue that remembers where it was started from, with play next, add to queue, reorder and remove; shuffle, repeat one or all, and stop after the current track. The queue, position and modes are restored when Helikodu starts again.
- A confirmation before playing at full scale to a device whose volume Helikodu cannot control.

#### Library
- Library folders are scanned by isolated worker processes, so a damaged file cannot take the player down, and watched for changes; folders that cannot be watched (network shares, FUSE mounts) are rescanned every 30 minutes. A folder whose drive is gone is hidden, not forgotten.
- Tags are read from every common tag format, with multiple artists and genres, credits by role, sort names and MusicBrainz identifiers; text in legacy charsets is repaired.
- Tracks are grouped into albums (with multi-disc albums and compilations) and artists; covers are found in the files and folders, stored once and cached as thumbnails.
- Search across the library, including scripts written without spaces.
- Play counts, skips, ratings and resume positions survive moving and renaming files.
- Static playlists that survive moved files, with import and export of M3U, M3U8, PLS and XSPF.
- A tag editor for one or many tracks, with mixed values, cover, lyrics and ratings. Every write goes to a checked copy first, so a failed write never damages the original.

#### Desktop application
- The `helikodu` window in the project's own design, light and dark, following the desktop's colour scheme, accent colour, high contrast and reduced-motion settings, under GNOME's and KDE's themes alike.
- Views for albums, artists, genres, folders and all tracks (a table with sortable, movable and resizable columns), playlists and search; a queue panel; and a player deck with transport, volume and an output switcher.
- Settings applied live, a first-run assistant that picks the music folders, a supported-formats page, a keyboard shortcuts window and keyboard access throughout, with accessible names for screen readers.
- Media controls and media keys through MPRIS, track notifications, sleep inhibition while playing, and files opened from the file manager.

#### Command line
- `helikodu-cli play` plays files gaplessly to a chosen backend and device, with keys for pause, seek, skip, volume and switching the output device, and `--replaygain` and `--preamp`; `list-outputs` lists the output devices.
- `helikodu-cli scan`, `search`, `info` and `stats` scan folders, search tracks, show a file's or track's tags and loudness, and show the library's totals and most played tracks. Every command can print JSON.

#### Documentation and releases
- A user guide, an FAQ and the man pages `helikodu(1)` and `helikodu-cli(1)`.
- Releases on GitHub: a source tarball with every Rust dependency included, which builds without network access through Meson, and ready-built programs for x86_64.

### Known issues

- With a library of 100,000 tracks, memory use reaches about 270–290 MB after scrolling through the whole track table. Start-up and the album grid stay under 250 MB.
- The manual checks on real GNOME, KDE Plasma and Sway sessions and a screen-reader pass with Orca (`tests/ui/daily-driver.md`) have not yet been done by people; only the automated interface checks have run.
- The waveform seek bar and the Lyrics and Signal path panels come in a later version.
- The ready-built programs need the FFmpeg 7.1 libraries (`libavformat.so.61`, `libavcodec.so.61`, `libavutil.so.59`). On a system that has moved to a newer FFmpeg (for example FFmpeg 8) and no longer has `libavformat.so.61`, they do not start; build from the source tarball instead, as the user guide's installing page describes.
