# Helikodu

Helikodu is an offline audio player and music library manager for Linux, written in Rust.

The name is Estonian: *heli* means "sound" and *kodu* means "home". It is a home for your own music library, kept locally on your computer. It is pronounced HE-li-ko-du.

> **Status: early development.** There is nothing to install or use yet. The repository currently contains the project's foundations: the workspace layout, the build checks and the contribution rules.

## Goals

- **Plays practically every audio format available on Linux**, including the sound track of video files. Helikodu never shows video.
- **Correct playback:** gapless joins, bit-perfect output where the hardware allows it, and loudness normalization that follows the standards (ReplayGain 2.0, EBU R128).
- **You choose the output:** pick the audio backend (PipeWire, PulseAudio, ALSA or JACK), the device and the mode in the settings, or switch them quickly while playing.
- **A library built for large collections** of 100,000 tracks and more, with fast search, one query language for searches and smart playlists, and play statistics that survive moving your files.
- **At home on every desktop:** GNOME, KDE Plasma and others, on Wayland and X11. Helikodu has its own design and integrates through freedesktop standards (MPRIS, notifications, XDG portals), so it depends on no particular desktop.
- **Entirely offline:** no internet access, no online services and no telemetry. This is enforced in the code and checked in CI.
- **Free software** under the GPL-3.0-or-later.

## Building

Helikodu needs Rust (the version is pinned in `rust-toolchain.toml`) and, as the work progresses, the development files of GTK 4, FFmpeg, PipeWire, ALSA and SQLite.

```sh
cargo build
cargo xtask ci      # every check that CI runs
```

## Repository layout

| Folder | Content |
|---|---|
| `apps/` | The programs: `helikodu` (desktop application), `helikodu-cli`, `helikodu-scan-worker` |
| `crates/` | The libraries, one folder per crate |
| `data/` | Desktop entry, AppStream metainfo, icons, D-Bus files, man pages |
| `po/` | Translations |
| `packaging/` | Flatpak, AUR, RPM and Debian packaging |
| `docs/` | Developer and user documentation, and the website |
| `tests/` | Fixtures, test corpus recipes, system and UI tests |
| `fuzz/` | Fuzz targets |
| `ci/` | Container definitions and test environments for CI |
| `xtask/` | Developer commands (`cargo xtask <command>`) |

## Contributing

Contributions are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) and follow the [Code of Conduct](CODE_OF_CONDUCT.md). To report a security problem, see [SECURITY.md](SECURITY.md).

## Licence

Helikodu is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. See [LICENSE](LICENSE).

Documentation and artwork are licensed under CC-BY-SA-4.0 and the AppStream metadata under CC0-1.0. Every file states its licence through [REUSE](https://reuse.software) metadata; the licence texts are in `LICENSES/`.
