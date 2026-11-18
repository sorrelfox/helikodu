// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Search: the folded form of text that sorting, grouping and search share, the two
//! search indexes and plain text search over them.
//!
//! A query is folded and split into terms at white space. A track matches when every
//! term matches one of its fields:
//!
//! - A term of three characters or more is looked up in the trigram index (a substring
//!   of the title, artists or album, in any script) and in the word index (a word
//!   prefix in any indexed field, including album artist, genre, composer and path).
//! - A term of one or two characters is too short for trigrams. It is found by
//!   scanning the folded title, artist, album and genre columns.
//!
//! Results are ranked by how the whole query matches the title, an artist or the album:
//! exactly, as a prefix, as the start of a word, inside a word, and last the tracks
//! that only match term by term. Within a rank they are in title order.

mod fold;
pub(crate) mod index;
mod query;
mod rank;
pub(crate) mod terms;

pub use fold::fold;
pub(crate) use query::search;
pub(crate) use query::term_condition;
pub use query::{SearchHit, SearchRequest, SearchResults};
pub use rank::MatchRank;
pub(crate) use rank::match_rank;
