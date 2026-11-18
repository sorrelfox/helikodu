// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Why query text could not be read, and where.

use std::fmt;
use std::ops::Range;

/// Query text that could not be read: what is wrong and the byte range of the part
/// that fails, for the interface to underline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    span: Range<usize>,
    kind: SyntaxErrorKind,
}

impl SyntaxError {
    pub(crate) fn new(span: Range<usize>, kind: SyntaxErrorKind) -> Self {
        Self { span, kind }
    }

    /// The byte range of the text that fails. It is never empty unless the text is:
    /// a missing part at the end points at the last character.
    pub fn span(&self) -> Range<usize> {
        self.span.clone()
    }

    /// What is wrong.
    pub fn kind(&self) -> &SyntaxErrorKind {
        &self.kind
    }

    /// A short example of text that works, to show beside the message.
    pub fn example(&self) -> Option<&'static str> {
        Some(match &self.kind {
            SyntaxErrorKind::UnclosedQuote => "title:\"two words\"",
            SyntaxErrorKind::UnclosedGroup | SyntaxErrorKind::UnexpectedClose => {
                "(genre:jazz OR genre:soul) year>=1970"
            }
            SyntaxErrorKind::MissingOperand(_) => "genre:jazz OR genre:soul",
            SyntaxErrorKind::UnknownField(_) => "artist:kask",
            SyntaxErrorKind::UnknownName { prefix: "has", .. } => "has:lyrics",
            SyntaxErrorKind::UnknownName { .. } => "is:lossless",
            SyntaxErrorKind::MissingOperator(_) => "tag:mood=calm",
            SyntaxErrorKind::MissingValue(_) => "year:1975",
            SyntaxErrorKind::InvalidOperator { .. } => "year>=1970",
            SyntaxErrorKind::InvalidValue { example, .. } => example,
            SyntaxErrorKind::InvalidRegex(_) => "title~\"^(Intro|Outro)$\"",
            SyntaxErrorKind::RangeOperator => "year:1970..1979",
            SyntaxErrorKind::UnknownSortKey(_) => "sort:added desc",
            SyntaxErrorKind::InvalidLimit => "limit:50",
            SyntaxErrorKind::Duplicate(_) | SyntaxErrorKind::Misplaced(_) => {
                "genre:jazz sort:added desc limit:50"
            }
            SyntaxErrorKind::EmptyText | SyntaxErrorKind::TooDeep => return None,
        })
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl std::error::Error for SyntaxError {}

/// What is wrong with query text.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SyntaxErrorKind {
    /// A `"` without its closing `"`.
    UnclosedQuote,
    /// A `(` without its `)`.
    UnclosedGroup,
    /// A `)` without its `(`.
    UnexpectedClose,
    /// `OR`, `AND`, `NOT` or `-` with nothing to apply to.
    MissingOperand(&'static str),
    /// A name before an operator that is not a field.
    UnknownField(String),
    /// A name after `has:` or `is:` that it does not know.
    UnknownName {
        /// `has` or `is`.
        prefix: &'static str,
        /// The name.
        name: String,
    },
    /// A tag field without an operator.
    MissingOperator(String),
    /// A field and operator without a value.
    MissingValue(String),
    /// A field that cannot be compared with this operator.
    InvalidOperator {
        /// The field.
        field: String,
        /// The operator.
        op: String,
    },
    /// A value that does not fit the field.
    InvalidValue {
        /// The field.
        field: String,
        /// What it takes.
        expected: &'static str,
        /// An example.
        example: &'static str,
    },
    /// A regular expression that does not compile.
    InvalidRegex(String),
    /// A range after an operator other than `:` or `=`.
    RangeOperator,
    /// A sort key that is not a field or `random`.
    UnknownSortKey(String),
    /// A limit that is not a number with an optional unit.
    InvalidLimit,
    /// A second `sort:` or `limit:`.
    Duplicate(&'static str),
    /// `sort:` or `limit:` inside parentheses or after a negation.
    Misplaced(&'static str),
    /// Empty quotes where text is searched for.
    EmptyText,
    /// Parentheses and negations nested deeper than the parser follows.
    TooDeep,
}

impl fmt::Display for SyntaxErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnclosedQuote => f.write_str("the quotes are not closed"),
            Self::UnclosedGroup => f.write_str("the parenthesis is not closed"),
            Self::UnexpectedClose => f.write_str("this parenthesis closes nothing"),
            Self::MissingOperand(op) => write!(f, "{op} needs something after it"),
            Self::UnknownField(name) => write!(f, "there is no field called {name}"),
            Self::UnknownName { prefix, name } => {
                write!(f, "{prefix}: does not know {name:?}")
            }
            Self::MissingOperator(field) => {
                write!(f, "{field} needs an operator such as : or =")
            }
            Self::MissingValue(field) => write!(f, "{field} needs a value"),
            Self::InvalidOperator { field, op } => {
                write!(f, "{field} cannot be compared with {op}")
            }
            Self::InvalidValue {
                field, expected, ..
            } => write!(f, "{field} takes {expected}"),
            Self::InvalidRegex(reason) => {
                write!(f, "the regular expression does not work: {reason}")
            }
            Self::RangeOperator => f.write_str("a range goes after : or ="),
            Self::UnknownSortKey(name) => write!(f, "cannot sort by {name}"),
            Self::InvalidLimit => f.write_str("a limit is a number of tracks, min, h, MB or GB"),
            Self::Duplicate(what) => write!(f, "{what}: is given twice"),
            Self::Misplaced(what) => {
                write!(
                    f,
                    "{what}: belongs at the top level, not in parentheses or negated"
                )
            }
            Self::EmptyText => f.write_str("the quotes are empty"),
            Self::TooDeep => f.write_str("the parentheses are nested too deeply"),
        }
    }
}
