# Changelog

All notable changes to Helikodu are recorded in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Cargo workspace with the programs in `apps/` and the libraries in `crates/`.
- Developer commands: `cargo xtask ci` and `cargo xtask check-deps`.
- Dependency policy with `cargo deny`, including a ban on network, TLS and DNS crates.
- REUSE licensing metadata and the project's contribution documents.
- `helikodu-cli play` plays audio files in order, without gaps, to a PipeWire or ALSA device chosen with `--backend` and `--device`, with keys to pause, seek, skip, change the volume and move to another output device while playing; `helikodu-cli list-outputs` lists the output devices, also as JSON.
- `helikodu-cli play --replaygain off|track|album|auto` normalizes loudness by the files' ReplayGain tags or Opus R128 tags, with `--preamp` and clipping prevention by the peak; `auto` uses the album gain for files whose neighbours share their album.
- `helikodu-cli scan`, `search`, `info` and `stats` work on the music library from the command line: scan folders with exclusions and a chosen number of workers, search tracks, show the tags, loudness values and statistics of a file or track, and show the library's totals and most played tracks, each also as JSON. The database is `$XDG_DATA_HOME/helikodu/library.db` unless `--database` names another.
