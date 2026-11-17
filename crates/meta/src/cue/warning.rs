// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the parser and the resolution skipped or repaired.

use std::fmt;

use super::path::PathRejection;

/// Something in a sheet that was skipped or repaired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueWarning {
    /// The line it is about, counted from 1, or `None` for the whole sheet.
    pub line: Option<usize>,
    /// What happened.
    pub kind: CueWarningKind,
}

impl fmt::Display for CueWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.kind),
            None => self.kind.fmt(f),
        }
    }
}

/// What a [`CueWarning`] is about.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CueWarningKind {
    /// Some bytes are not valid in the charset the sheet was decoded with; they
    /// were replaced by U+FFFD.
    UndecodableText {
        /// The charset's name.
        encoding: &'static str,
    },
    /// The line is longer than the parser accepts; it was skipped.
    LineTooLong,
    /// The command is not a CUE command; the line was skipped.
    UnknownCommand(String),
    /// The command lacks an argument; the line was skipped.
    MissingArgument(&'static str),
    /// A number could not be read; the line was skipped.
    BadNumber(String),
    /// A time is not `mm:ss:ff` with seconds below 60 and frames below 75; the
    /// line was skipped.
    BadTime(String),
    /// A quoted value has no closing quote; the rest of the line was taken.
    UnterminatedQuote,
    /// The command belongs to a track but comes before the first `TRACK`; the line
    /// was skipped.
    OutsideTrack(&'static str),
    /// The command was given twice; the later value was kept.
    Repeated(&'static str),
    /// A track number appears twice.
    DuplicateTrack(u32),
    /// An index number appears twice in a track; the later one was skipped.
    DuplicateIndex(u32),
    /// More of something than the parser accepts; the rest was skipped.
    TooMany(&'static str),
    /// There were too many warnings; the rest were not recorded.
    TooManyWarnings,
    /// The track has no `INDEX 01`; another index was taken as its start.
    MissingIndex01(u32),
    /// The track has no index at all and was left out.
    NoIndex(u32),
    /// The track's `INDEX 00` comes after its start; the pregap was ignored.
    PregapAfterStart(u32),
    /// The track does not start after the previous track of the same file and
    /// was left out.
    OutOfOrder(u32),
    /// The track's start has no `FILE` before it; the track was left out.
    TrackWithoutFile(u32),
    /// The `FILE` path was rejected; the tracks in that file were left out.
    RejectedPath {
        /// The path as written in the sheet.
        name: String,
        /// Why it was rejected.
        reason: PathRejection,
    },
    /// The file is not on disk, under its own name or a likely substitute.
    FileNotFound(String),
    /// The file was not found under its own name; a file with the same name in
    /// another case or another audio extension was taken.
    FileSubstituted {
        /// The path as written in the sheet.
        name: String,
        /// The file name taken instead.
        found: String,
    },
    /// An embedded sheet names more than one file; only the tracks of the first
    /// were kept.
    EmbeddedExtraFile,
}

impl fmt::Display for CueWarningKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndecodableText { encoding } => {
                write!(f, "bytes not valid in {encoding} were replaced")
            }
            Self::LineTooLong => f.write_str("the line is too long and was skipped"),
            Self::UnknownCommand(c) => write!(f, "unknown command {c:?}"),
            Self::MissingArgument(c) => write!(f, "{c} lacks an argument"),
            Self::BadNumber(n) => write!(f, "{n:?} is not a valid number"),
            Self::BadTime(t) => write!(f, "{t:?} is not a valid mm:ss:ff time"),
            Self::UnterminatedQuote => f.write_str("a quoted value has no closing quote"),
            Self::OutsideTrack(c) => write!(f, "{c} comes before the first TRACK"),
            Self::Repeated(c) => write!(f, "{c} is given more than once"),
            Self::DuplicateTrack(n) => write!(f, "track {n} appears more than once"),
            Self::DuplicateIndex(n) => write!(f, "index {n:02} appears more than once"),
            Self::TooMany(what) => write!(f, "too many {what}; the rest were skipped"),
            Self::TooManyWarnings => f.write_str("too many warnings; the rest were not recorded"),
            Self::MissingIndex01(n) => write!(f, "track {n} has no INDEX 01"),
            Self::NoIndex(n) => write!(f, "track {n} has no index and was left out"),
            Self::PregapAfterStart(n) => {
                write!(f, "track {n} has INDEX 00 after its start; pregap ignored")
            }
            Self::OutOfOrder(n) => {
                write!(f, "track {n} does not start after the previous track")
            }
            Self::TrackWithoutFile(n) => write!(f, "track {n} has no FILE"),
            Self::RejectedPath { name, reason } => write!(f, "file {name:?} rejected: {reason}"),
            Self::FileNotFound(name) => write!(f, "file {name:?} not found"),
            Self::FileSubstituted { name, found } => {
                write!(f, "file {name:?} not found; {found:?} used instead")
            }
            Self::EmbeddedExtraFile => {
                f.write_str("an embedded sheet names several files; only the first was used")
            }
        }
    }
}

/// The most warnings kept for one sheet.
const MAX_WARNINGS: usize = 256;

/// Collects warnings up to a limit.
#[derive(Debug, Default)]
pub(crate) struct Warnings {
    list: Vec<CueWarning>,
}

impl Warnings {
    pub(crate) fn push(&mut self, line: Option<usize>, kind: CueWarningKind) {
        match self.list.len() {
            n if n < MAX_WARNINGS => self.list.push(CueWarning { line, kind }),
            MAX_WARNINGS => self.list.push(CueWarning {
                line: None,
                kind: CueWarningKind::TooManyWarnings,
            }),
            _ => {}
        }
    }

    pub(crate) fn at(&mut self, line: usize, kind: CueWarningKind) {
        self.push(Some(line), kind);
    }

    pub(crate) fn into_vec(self) -> Vec<CueWarning> {
        self.list
    }
}
