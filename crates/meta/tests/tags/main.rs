// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tag reading through the public API.
//!
//! The media are generated at test time with the `ffmpeg` tool (tags set with
//! `-metadata`) and edited with lofty where a test needs frames FFmpeg cannot
//! write: `POPM`, `UFID`, `TMCL`, multi-valued fields, Latin-1 frames holding
//! legacy charsets. A test whose tool or encoder is missing is skipped with a
//! message, not failed.

#![forbid(unsafe_code)]

mod charsets;
mod fallback;
mod formats;
mod media;
mod robustness;
