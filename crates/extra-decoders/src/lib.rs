// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoders for the long tail of formats: tracker modules, game music, SID, MIDI and DSD files.
//!
//! See the crate's README for its place in the workspace.

#![warn(missing_docs)]

#[cfg(feature = "dsd")]
pub mod dsd;
