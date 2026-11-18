// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Integration tests through the public API.
//!
//! The test media are generated at test time: a reference signal written by the
//! tests themselves, encoded with the `ffmpeg` command-line tool (and `lame` and
//! `opusenc` where present) into Cargo's temporary directory. A test whose tool or
//! encoder is missing is skipped with a message, not failed.

mod codec_probe;
mod decoding;
mod encoding;
mod gapless;
mod lockdown;
mod media;
mod metadata;
mod seeking;
mod streams;
