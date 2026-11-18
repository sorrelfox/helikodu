// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A playlist and its entries, independent of the file format.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::PlaylistKind;
use super::location::{Syntax, resolve_location};
use super::text::TextEncoding;
use super::warning::{PlaylistWarningKind, Warnings};

/// A playlist: an optional title and its entries in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Playlist {
    /// The title the file gives (`#PLAYLIST:` in M3U, `<title>` in XSPF).
    pub title: Option<String>,
    /// The entries, in order.
    pub entries: Vec<PlaylistEntry>,
}

/// What an entry points at.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntryLocation {
    /// A path on this system, as bytes. Absolute when the playlist's folder was known
    /// or the entry was absolute.
    Local(PathBuf),
    /// A path written for another system: a drive letter (`C:/Music/a.mp3`), a
    /// network share (`//server/share/a.mp3`) or a path rooted at the current drive
    /// (`/Music/a.mp3` written as `\Music\a.mp3`). Bytes with `/` as the separator.
    /// It cannot be opened here; a library can match it by its last components.
    Foreign(Vec<u8>),
    /// A URL that is not a local file (`http:`, `https:`, `smb:`, a `file:` URL on
    /// another host). Kept as an unsupported entry; never fetched.
    Remote(String),
}

impl EntryLocation {
    /// The local path, if this is one.
    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Self::Local(path) => Some(path),
            Self::Foreign(_) | Self::Remote(_) => None,
        }
    }
}

/// What a playlist file says about an entry besides its location. A library uses it
/// to find a track whose path changed, and to show an entry it cannot find.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryHints {
    /// The title.
    pub title: Option<String>,
    /// The artist, as credited.
    pub artist: Option<String>,
    /// The album.
    pub album: Option<String>,
    /// The length.
    pub duration: Option<Duration>,
    /// The track number.
    pub track_number: Option<u32>,
}

impl EntryHints {
    /// Whether nothing is known.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// One entry of a playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistEntry {
    /// What the entry points at.
    pub location: EntryLocation,
    /// For a local path read from a file in a legacy charset: the path made of the
    /// original bytes, when they differ from the decoded path in `location`. Old
    /// collections may still have file names in that charset.
    pub undecoded_path: Option<PathBuf>,
    /// The title, artist and length the file gives.
    pub hints: EntryHints,
    /// The line it was read from, counted from 1; `None` for an entry not read from
    /// a file.
    pub line: Option<usize>,
}

impl PlaylistEntry {
    /// An entry pointing at `location`, with no hints.
    pub fn new(location: EntryLocation) -> Self {
        Self {
            location,
            undecoded_path: None,
            hints: EntryHints::default(),
            line: None,
        }
    }

    /// An entry for the local file at `path`.
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::new(EntryLocation::Local(path.into()))
    }

    /// The local paths this entry may stand for, the decoded one first.
    pub fn local_paths(&self) -> impl Iterator<Item = &Path> {
        self.location
            .local_path()
            .into_iter()
            .chain(self.undecoded_path.as_deref())
    }
}

/// Turns the locations read from a file into entries, within the entry limit.
#[derive(Debug)]
pub(crate) struct EntryReader<'a> {
    base: Option<&'a Path>,
    syntax: Syntax,
    encoding: TextEncoding,
    max_entries: usize,
    entries: Vec<PlaylistEntry>,
    full: bool,
}

impl<'a> EntryReader<'a> {
    pub(crate) fn new(
        base: Option<&'a Path>,
        syntax: Syntax,
        encoding: TextEncoding,
        max_entries: usize,
    ) -> Self {
        Self {
            base,
            syntax,
            encoding,
            max_entries,
            entries: Vec::new(),
            full: false,
        }
    }

    /// Whether no more entries are taken.
    pub(crate) fn is_full(&self) -> bool {
        self.entries.len() >= self.max_entries
    }

    /// Adds the entry written as `raw` (the bytes in the file) and `text` (decoded) on
    /// `line`. A rejected location or another playlist is skipped with a warning.
    pub(crate) fn push(
        &mut self,
        raw: &[u8],
        text: &str,
        line: usize,
        hints: EntryHints,
        warnings: &mut Warnings,
    ) {
        if self.is_full() {
            if !self.full {
                self.full = true;
                warnings.at(line, PlaylistWarningKind::TooManyEntries);
            }
            return;
        }
        // In a legacy charset the text is the likely name today and the bytes the
        // name as it was; in UTF-8 the bytes are the name, even where they are not
        // valid UTF-8.
        let legacy = self.encoding.is_legacy();
        let primary = if legacy { text.as_bytes() } else { raw };
        let location = match resolve_location(primary, self.base, self.syntax) {
            Ok(location) => location,
            Err(problem) => {
                warnings.at(
                    line,
                    PlaylistWarningKind::BadLocation {
                        location: text.trim().to_owned(),
                        problem,
                    },
                );
                return;
            }
        };
        let named = match &location {
            EntryLocation::Local(path) => Some(path.as_os_str().as_bytes()),
            EntryLocation::Foreign(path) => Some(path.as_slice()),
            EntryLocation::Remote(_) => None,
        };
        if named.is_some_and(PlaylistKind::is_playlist_path) {
            warnings.at(
                line,
                PlaylistWarningKind::NestedPlaylist(text.trim().to_owned()),
            );
            return;
        }
        if let EntryLocation::Remote(url) = &location {
            warnings.at(line, PlaylistWarningKind::RemoteEntry(url.clone()));
        }
        let undecoded_path = if legacy && raw != text.as_bytes() {
            match resolve_location(raw, self.base, self.syntax) {
                Ok(EntryLocation::Local(path)) if location.local_path() != Some(&path) => {
                    Some(path)
                }
                _ => None,
            }
        } else {
            None
        };
        self.entries.push(PlaylistEntry {
            location,
            undecoded_path,
            hints,
            line: Some(line),
        });
    }

    pub(crate) fn into_entries(self) -> Vec<PlaylistEntry> {
        self.entries
    }
}
