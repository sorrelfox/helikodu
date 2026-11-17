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
