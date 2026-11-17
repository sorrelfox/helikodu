// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The music library: database, scanner, watcher, search, query language and statistics.
//!
//! See the crate's README for its place in the workspace.

#![forbid(unsafe_code)]

mod db;
mod search;

pub use db::{
    Database, DatabaseConfig, DbError, LocaleCollation, Location, MemoryName, SCHEMA_VERSION,
    WriteTicket, Writer, display_text, path_from_blob, path_to_blob,
};
/// The SQLite bindings, whose types appear in the read and write callbacks.
pub use rusqlite;
pub use search::fold;
