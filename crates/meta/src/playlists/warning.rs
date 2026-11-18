// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the parsers skipped or repaired, and what the writers could not write.

use std::fmt;

use super::location::LocationProblem;

/// Something in a playlist that was skipped or repaired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistWarning {
    /// The line it is about, counted from 1, or `None` for the whole file.
    pub line: Option<usize>,
    /// What happened.
    pub kind: PlaylistWarningKind,
}

impl fmt::Display for PlaylistWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.kind),
            None => self.kind.fmt(f),
        }
    }
}

/// What a [`PlaylistWarning`] is about.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlaylistWarningKind {
    /// Some bytes are not valid in the charset the file was decoded with; they were
    /// replaced by U+FFFD in the text.
    UndecodableText {
        /// The charset's name.
        encoding: &'static str,
    },
    /// The line (or an XML text) is longer than the parser accepts; it was skipped.
    LineTooLong,
    /// The playlist has more entries than the parser accepts; the rest were skipped.
    TooManyEntries,
    /// There were too many warnings; the rest were not recorded.
    TooManyWarnings,
    /// The entry is a URL. It was kept as an unsupported entry and is never fetched.
    RemoteEntry(String),
    /// The entry names another playlist, which is not followed; it was skipped.
    NestedPlaylist(String),
    /// The entry's location was rejected; the entry was skipped.
    BadLocation {
        /// The location as written.
        location: String,
        /// Why it was rejected.
        problem: LocationProblem,
    },
    /// A length could not be read; it was ignored.
    BadDuration(String),
    /// A number could not be read; it was ignored.
    BadNumber(String),
    /// The line is not part of the format; it was skipped.
    BadLine(String),
    /// Entry information (`#EXTINF`, a PLS title) with no entry for it.
    OrphanInformation,
    /// A key was given twice; the later value was kept.
    Repeated(String),
    /// A PLS file has no `[playlist]` section; its keys were read anyway.
    MissingSection,
    /// A PLS file's `NumberOfEntries` disagrees with the entries found.
    EntryCountMismatch {
        /// What the file declares.
        declared: u64,
        /// What was found.
        found: usize,
    },
    /// An XSPF track has no location; it was skipped.
    MissingLocation,
    /// The XML is not well formed from here on; the rest of the file was skipped.
    MalformedXml(String),
    /// The XML is nested deeper than the parser follows; the deeper part was skipped.
    TooDeep,
    /// The XML document is not an XSPF playlist.
    NotXspf,
}

impl fmt::Display for PlaylistWarningKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndecodableText { encoding } => {
                write!(f, "bytes not valid in {encoding} were replaced")
            }
            Self::LineTooLong => f.write_str("the line is too long and was skipped"),
            Self::TooManyEntries => f.write_str("too many entries; the rest were skipped"),
            Self::TooManyWarnings => f.write_str("too many warnings; the rest were not recorded"),
            Self::RemoteEntry(url) => {
                write!(f, "{url:?} is a URL; kept as unsupported and never fetched")
            }
            Self::NestedPlaylist(path) => {
                write!(f, "{path:?} is another playlist and is not followed")
            }
            Self::BadLocation { location, problem } => {
                write!(f, "entry {location:?} skipped: {problem}")
            }
            Self::BadDuration(text) => write!(f, "{text:?} is not a valid length"),
            Self::BadNumber(text) => write!(f, "{text:?} is not a valid number"),
            Self::BadLine(text) => write!(f, "{text:?} was not understood and was skipped"),
            Self::OrphanInformation => f.write_str("entry information without an entry"),
            Self::Repeated(key) => write!(f, "{key} is given more than once"),
            Self::MissingSection => f.write_str("the [playlist] section is missing"),
            Self::EntryCountMismatch { declared, found } => {
                write!(f, "{declared} entries declared, {found} found")
            }
            Self::MissingLocation => f.write_str("a track without a location was skipped"),
            Self::MalformedXml(error) => {
                write!(f, "malformed XML ({error}); the rest was skipped")
            }
            Self::TooDeep => f.write_str("the XML is nested too deeply"),
            Self::NotXspf => f.write_str("the document is not an XSPF playlist"),
        }
    }
}

/// The most warnings kept for one file.
const MAX_WARNINGS: usize = 256;

/// Collects warnings up to a limit.
#[derive(Debug, Default)]
pub(crate) struct Warnings {
    list: Vec<PlaylistWarning>,
}

impl Warnings {
    pub(crate) fn push(&mut self, line: Option<usize>, kind: PlaylistWarningKind) {
        match self.list.len() {
            n if n < MAX_WARNINGS => self.list.push(PlaylistWarning { line, kind }),
            MAX_WARNINGS => self.list.push(PlaylistWarning {
                line: None,
                kind: PlaylistWarningKind::TooManyWarnings,
            }),
            _ => {}
        }
    }

    pub(crate) fn at(&mut self, line: usize, kind: PlaylistWarningKind) {
        self.push(Some(line), kind);
    }

    pub(crate) fn into_vec(self) -> Vec<PlaylistWarning> {
        self.list
    }
}

/// An entry that could not be written as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteWarning {
    /// The entry's index in the playlist, from 0.
    pub entry: usize,
    /// What happened.
    pub kind: WriteWarningKind,
}

impl fmt::Display for WriteWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "entry {}: {}", self.entry + 1, self.kind)
    }
}

/// What a [`WriteWarning`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteWarningKind {
    /// The path is not valid UTF-8 and the format is; it was written as decoded text
    /// and may not resolve.
    NotUtf8,
    /// The path holds a line break, which the format cannot carry; the entry was left
    /// out.
    Unrepresentable,
}

impl fmt::Display for WriteWarningKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotUtf8 => "the path is not UTF-8; written as text that may not resolve",
            Self::Unrepresentable => "the path holds a line break and was left out",
        })
    }
}
