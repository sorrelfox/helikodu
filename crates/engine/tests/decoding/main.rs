// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The decoder registry, the FFmpeg and DSD backends and sections, through the
//! public API.
//!
//! The test media are generated at test time: a reference signal written as WAV by
//! the tests themselves, encoded with the `ffmpeg` command-line tool into Cargo's
//! temporary directory, and DSF/DFF files of a sigma-delta modulated sine. A test
//! whose tool or encoder is missing prints `SKIPPED:` and passes.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]

#[cfg(feature = "dsd")]
mod dsd;
mod errors;
mod media;
mod registry;
mod sections;
mod seeking;
