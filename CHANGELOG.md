# Changelog

All notable changes to Helikodu are recorded in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Lyrics: synced and plain lyrics from a `.lrc` or `.txt` file beside the track (any charset) or from its tags (`SYLT`, `USLT`, `LYRICS`, `UNSYNCEDLYRICS`, `©lyr`, APEv2 `Lyrics`), in the Lyrics tab of the right panel. The line sung now stays in the middle; a click on a line plays from there; the offset of each track can be moved in steps of 0.1 s.
- Copies of the same file are recognised: byte-identical files (content and tags) share one set of play counts, ratings, history and playlist entries, and play from a main copy that another copy replaces when it disappears. The setting `[library.duplicates]` keeps copies out of the library (`on_scan = "skip"`) or shows only the main copy (`show_one`). Copies, and other duplicates found by recording or album track, can be moved to the desktop trash after safety checks; `helikodu-cli duplicates` lists them. Before play counts of copies are first combined, the library is backed up once to `library.db.backup-before-copies`.
- Smart playlists: rules built with a rule builder that shows the same rules as query text (either can be edited), groups of rules, a limit by tracks, time or size, and an order; their tracks follow the library as it changes.
- Playlist folders in the sidebar; a playlist dragged onto a folder moves into it.
- Pins: albums, artists and playlists can be pinned to the top of the sidebar.
- Playlists: undo and redo of every change (Ctrl+Z, Ctrl+Shift+Z), a lock against changes, removing duplicates and missing tracks, shuffling the order for good, and a display grouped by album.
- Playlist files (M3U, M3U8, PLS, XSPF) found in the library folders can become playlists that follow their files (`[library] import_playlists`).
- Now Playing (F11, or a click on the deck's cover): the cover, the track, the synced lyrics or the queue, and the signal path from the file to the device.
- Search understands the query language: text with fields such as `genre:jazz -genre:fusion` or `added<30d sort:added desc` lists the tracks it selects in a table, with completion of field names and values under the search field, and a notice with an example when part of the query cannot be read. The Tracks filter takes queries too.
- Saved searches: save a search under a name, and open, rename or delete it from the right column of the search page, which also shows example queries.
- The Tracks table can show **Genre** and **Added** columns.
- Play history (**Library ▸ History**): every listen by day with its time, source and listening time, skips dimmed, for today, 7 days, 30 days or all, with a filter and **Save as playlist**.
- **Library ▸ Export History…** writes the play history to a local file as ListenBrainz listens (JSON Lines) or as a `.scrobbler.log`, for submitting later; nothing is sent.
- MPRIS: the track list (the queue, with metadata and jumping to a track) and the playlists (listed in several orders, started from desktop controls, the one playing shown as active).
- Composers and Years in the sidebar: the albums of a composer, and of a year or a decade (by the original release year where the tags give one), with **Play all** and **Shuffle**.
- Audiobooks: the album page of an audiobook lists its chapters with where each starts and which have been heard, shows how much is left, continues where you stopped and can start again. Audiobooks are left out of shuffle and of **Play all** in other views. The kind of an album (music, audiobook, audio from a video) can be changed on its page and survives rescans.
- Chapters: while a chapter plays, the deck shows its number and title, and the seek bar covers the whole file with a gap at each chapter boundary.
- Folders outside the library: the Folders view also shows the home folder, connected drives and the file system, plays from them without storing anything, and adds a folder to the library with **Add to library**.

### Changed

- The player deck at the bottom of the window follows the colour scheme: light in the light scheme, with its popovers, seek bar and output readout, and dark as before in the dark scheme.

### Fixed

- With a colour scheme chosen in Helikodu that differs from the desktop's, text no longer takes the desktop theme's colours: sidebar entries, titles and other labels were light on light (or dark on dark) under themes such as Breeze, mostly while the window did not have the focus.
- Tags without a field of their own (`MOOD`, `BPM`, `GROUPING`, `LABEL`, `COMMENT` and others) are stored when files are scanned, so `tag:NAME` queries find them, and shuffling by grouping uses the `GROUPING` tag.

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
