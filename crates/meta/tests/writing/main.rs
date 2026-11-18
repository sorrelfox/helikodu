// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tag writing through the public API.
//!
//! The media are generated at test time with the `ffmpeg` tool, the pictures
//! with the `image` crate. A test whose tool or encoder is missing is skipped
//! with a message, not failed.

#![forbid(unsafe_code)]

mod behaviour;
mod formats;
mod media;
