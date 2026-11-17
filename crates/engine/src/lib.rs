// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The playback engine: decoding, processing, output backends and the transport.
//!
//! See the crate's README for its place in the workspace.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod decode;
pub mod output;
pub mod pipeline;
