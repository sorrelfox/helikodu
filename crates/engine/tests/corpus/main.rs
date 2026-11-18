// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine against the generated test corpus (`cargo xtask corpus`, see
//! `tests/corpus/README.md`): every sample of the formats set decodes to the
//! length, rate and channels its manifest entry expects; the gapless sets join
//! through the transport bit-exact (lossless) or with the exact total length
//! (lossy); seeks land exactly (lossless) or aligned (lossy); video files play
//! their audio only; broken files fail cleanly without stopping playback; file
//! names that are not UTF-8 play; the long-tail files play with their own backends.
//!
//! The corpus is read from `target/corpus/` (or `$HELIKODU_CORPUS`). When it has
//! not been generated, every test prints `SKIPPED:` and passes. Each test prints a
//! table of its results (`-- --nocapture`) and writes it to
//! `target/tmp/corpus-report/`.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]

mod broken;
mod decoding;
mod formats;
mod gapless;
mod longtail;
mod manifest;
// The transport tests' helpers: the WAV reader and the engine driven as the
// service drives it. Not every helper is used here.
#[allow(dead_code)]
#[path = "../transport/media.rs"]
mod media;
mod names;
mod report;
mod seeking;
#[allow(dead_code)]
#[path = "../transport/session.rs"]
mod session;
mod video;
