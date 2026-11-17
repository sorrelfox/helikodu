// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cover art through the public API: embedded pictures, folder images, format
//! sniffing, limits on hostile pictures and thumbnails.
//!
//! Pictures are encoded by the tests with the `image` crate or written byte by
//! byte; audio files are generated with the `ffmpeg` tool (pictures as
//! attached-picture streams) or given their pictures with lofty. A test whose
//! tool or encoder is missing is skipped with a message, not failed.

#![forbid(unsafe_code)]

mod decoding;
mod embedded;
mod folder;
mod media;
