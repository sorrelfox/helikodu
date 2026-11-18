# Changelog

All notable changes to Helikodu are recorded in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
