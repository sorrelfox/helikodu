// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! CUE sheets and chapters through the public API.
//!
//! The sheets are written by the tests, shaped like those found in the wild. The
//! audio for embedded sheets and chapters is generated at test time with the
//! `ffmpeg` and `metaflac` tools; a test whose tool is missing is skipped with a
//! message, not failed.

#![forbid(unsafe_code)]

mod chapters;
mod embedded;
mod media;
mod sheets;
