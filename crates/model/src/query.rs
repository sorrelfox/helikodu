// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The query tree shared by search, saved searches and smart playlists.
//!
//! A [`Query`] is an optional filter [`Expr`], sort keys and a [`Limit`]. The filter
//! combines [`Comparison`]s of a [`Field`] with a [`Value`], plain text, presence tests
//! (`has:`) and flags (`is:`) with `All`, `Any` and `Not`. The library parses the text
//! form into this tree, prints it back and compiles it to SQL; the rule builder edits
//! the tree itself.
//!
//! The tree is plain data with serde support. [`Comparison::check`] tells whether a
//! field, an operator and a value fit together, so that a rule builder can refuse a
//! combination the compiler would reject.

mod expr;
mod field;
mod quick;
mod value;

pub use expr::{
    Comparison, Expr, Flag, Limit, LimitUnit, Operator, Presence, Query, SortBy, SortKey,
    TermProblem,
};
pub use field::{Field, FieldKind};
pub use quick::QuickFilter;
pub use value::{Age, AgeUnit, CalendarDate, Decimal, MediaKind, Value};
