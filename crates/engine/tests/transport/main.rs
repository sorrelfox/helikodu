// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The transport, through the public API: an engine playing generated media to the
//! WAV and null sinks, as fast as they take it.
//!
//! The media are written by the tests (WAV) or encoded from that with the `ffmpeg`
//! command-line tool into Cargo's temporary directory. A test whose tool or encoder
//! is missing prints `SKIPPED:` and passes.

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]

mod ab_loop;
mod crossfade;
mod devices;
mod dsp;
mod errors;
mod gapless;
mod media;
mod session;
mod settings;
mod transitions;
