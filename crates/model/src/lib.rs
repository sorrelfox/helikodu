// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared types: identifiers, audio specifications, library records, settings, commands and events.
//!
//! See the crate's README for its place in the workspace.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod analysis;
pub mod audio;
pub mod eq;
pub mod expression;
pub mod library;
mod path_serde;
pub mod player;
pub mod query;
pub mod settings;
mod text_form;
pub mod user_fields;

pub use text_form::ParseError;
