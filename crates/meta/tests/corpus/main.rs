// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tags, pictures, lyrics and CUE sheets of the generated test corpus
//! (`cargo xtask corpus`, see `tests/corpus/README.md`): every file of the tag
//! and script sets reads as its manifest entry expects, and every sheet of the
//! CUE set splits its audio into the tracks the entry lists.
//!
//! The corpus is read from `target/corpus/` (or `$HELIKODU_CORPUS`). When it has
//! not been generated, every test prints `SKIPPED:` and passes.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]

mod cue;
mod manifest;
mod tags;
