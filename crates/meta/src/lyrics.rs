// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lyrics: LRC files, lyrics in the tags, and the choice between them.
//!
//! - [`parse_lrc`] reads LRC text, tolerant of the files found in the wild and
//!   bounded in what it keeps (see [`lrc`]).
//! - [`read_embedded`] reads the lyrics in the tags of a file: `SYLT` and
//!   `USLT`, `LYRICS` and `UNSYNCEDLYRICS`, `©lyr`, APEv2 `Lyrics`.
//! - [`sidecar_paths`] and [`read_sidecar`] find and read a `.lrc` or `.txt`
//!   file with the base name of the track, in its folder.
//! - [`LyricsProvider`] is where lyrics come from; [`LocalLyrics`] looks only
//!   at the track's own files.
//!
//! When a track has several lyrics, synced ones win over plain ones; of two of
//! the same kind, the file beside the track wins over the tags.

pub mod lrc;

mod embedded;
mod sidecar;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use encoding_rs::Encoding;
use thiserror::Error;

pub use embedded::read_embedded;
pub use lrc::{LrcMetadata, ParsedLrc, parse_lrc};
pub use sidecar::{MAX_SIDECAR_BYTES, read_sidecar, sidecar_paths};

/// One line of lyrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    /// When the line is sung; `None` in plain lyrics.
    pub time: Option<Duration>,
    /// The text, without time or word tags.
    pub text: String,
    /// The times of the words, from enhanced LRC or a `SYLT` frame with one
    /// entry per word; kept for a later word-by-word display.
    pub words: Vec<WordTiming>,
    /// Whether the line starts a verse: a blank line or a pause came before.
    pub verse_start: bool,
}

/// When a word of a line is sung.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordTiming {
    /// The time.
    pub time: Duration,
    /// The byte offset in the line's text where the word starts.
    pub start: usize,
}

/// The kind of a lyrics file beside the track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SidecarKind {
    /// A `.lrc` file.
    Lrc,
    /// A `.txt` file.
    Text,
}

/// The tag lyrics came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddedKind {
    /// An ID3v2 `SYLT` frame.
    Id3Synced,
    /// An ID3v2 `USLT` frame.
    Id3Unsynced,
    /// The Vorbis comment `LYRICS` or `UNSYNCEDLYRICS`.
    VorbisComment,
    /// The MP4 `©lyr` atom.
    Mp4,
    /// The APEv2 `Lyrics` item.
    Ape,
    /// A tag read through FFmpeg.
    Other,
}

/// Where lyrics came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LyricsSource {
    /// A file beside the track.
    Sidecar {
        /// The file.
        path: PathBuf,
        /// `.lrc` or `.txt`.
        kind: SidecarKind,
        /// The charset its text was decoded from (WHATWG name).
        charset: &'static str,
    },
    /// The tags of the track's file.
    Embedded(EmbeddedKind),
}

impl LyricsSource {
    /// The lyrics file, when they came from one.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Sidecar { path, .. } => Some(path),
            Self::Embedded(_) => None,
        }
    }
}

/// The lyrics of a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lyrics {
    /// The lines, by time when synced, in text order otherwise.
    pub lines: Vec<LyricLine>,
    /// Whether every line has a time.
    pub synced: bool,
    /// Where they came from.
    pub source: LyricsSource,
    /// The LRC metadata tags, when there were any.
    pub metadata: LrcMetadata,
}

impl Lyrics {
    /// Lyrics from `text`: synced when it is LRC with at least one timed line,
    /// otherwise its lines as plain lyrics. `None` for text without lines.
    pub fn from_text(text: &str, source: LyricsSource) -> Option<Self> {
        let parsed = parse_lrc(text);
        let (lines, synced) = if parsed.timed.is_empty() {
            (parsed.plain, false)
        } else {
            (parsed.timed, true)
        };
        (!lines.is_empty()).then_some(Self {
            lines,
            synced,
            source,
            metadata: parsed.metadata,
        })
    }

    /// The index of the line sung at `position`, with the lines moved by
    /// `offset_ms` (positive shows them sooner, as LRC's `[offset:]`). `None`
    /// before the first line and for plain lyrics.
    pub fn line_at(&self, position: Duration, offset_ms: i64) -> Option<usize> {
        if !self.synced {
            return None;
        }
        let at = shifted(position, offset_ms);
        let after = self
            .lines
            .partition_point(|l| l.time.is_some_and(|t| t <= at));
        after.checked_sub(1)
    }

    /// How long after `position` the line after the one at `position` begins,
    /// with the same offset as [`line_at`](Self::line_at); `None` after the
    /// last line and for plain lyrics.
    pub fn next_change(&self, position: Duration, offset_ms: i64) -> Option<Duration> {
        if !self.synced {
            return None;
        }
        let at = shifted(position, offset_ms);
        let next = self
            .lines
            .partition_point(|l| l.time.is_some_and(|t| t <= at));
        self.lines
            .get(next)
            .and_then(|l| l.time)
            .map(|t| t.saturating_sub(at))
    }
}

/// `position` as the lines see it with `offset_ms`.
fn shifted(position: Duration, offset_ms: i64) -> Duration {
    let ms = i64::try_from(position.as_millis()).unwrap_or(i64::MAX);
    Duration::from_millis(u64::try_from(ms.saturating_add(offset_ms)).unwrap_or(0))
}

/// Why lyrics could not be read.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LyricsError {
    /// A file could not be read.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The tags could not be read.
    #[error("cannot read the tags: {0}")]
    Tags(String),
    /// A lyrics file is larger than [`MAX_SIDECAR_BYTES`].
    #[error("{} is too large for a lyrics file", .0.display())]
    TooLarge(PathBuf),
}

/// What lyrics are looked for.
#[derive(Debug, Clone, Copy)]
pub struct LyricsQuery<'a> {
    /// The track's file.
    pub path: &'a Path,
}

/// A source of lyrics. Only local ones exist: there is no network access.
pub trait LyricsProvider: Send + Sync + fmt::Debug {
    /// Every lyrics found for `query`, best first.
    fn find_all(&self, query: &LyricsQuery<'_>) -> Result<Vec<Lyrics>, LyricsError>;

    /// The best lyrics for `query`, if any.
    fn find(&self, query: &LyricsQuery<'_>) -> Result<Option<Lyrics>, LyricsError> {
        Ok(self.find_all(query)?.into_iter().next())
    }
}

/// Lyrics from the track's own files: the `.lrc` or `.txt` file beside it and
/// its tags.
#[derive(Debug, Clone, Default)]
pub struct LocalLyrics {
    legacy_charset: Option<&'static Encoding>,
}

impl LocalLyrics {
    /// Local lyrics; text files that are not UTF-8 are read in `legacy_charset`
    /// (a label such as `windows-1251`) or else in a guessed charset.
    pub fn new(legacy_charset: Option<&str>) -> Self {
        Self {
            legacy_charset: legacy_charset.and_then(|l| Encoding::for_label(l.as_bytes())),
        }
    }
}

impl LyricsProvider for LocalLyrics {
    fn find_all(&self, query: &LyricsQuery<'_>) -> Result<Vec<Lyrics>, LyricsError> {
        let mut found = Vec::new();
        let mut error = None;
        for (path, kind) in sidecar_paths(query.path) {
            match read_sidecar(&path, kind, self.legacy_charset) {
                Ok(Some(lyrics)) => found.push(lyrics),
                Ok(None) => {}
                Err(e) => error = Some(e),
            }
        }
        match read_embedded(query.path) {
            Ok(embedded) => found.extend(embedded),
            Err(e) => error = Some(e),
        }
        if found.is_empty()
            && let Some(error) = error
        {
            return Err(error);
        }
        // Synced first; within each kind the order found: files, then tags.
        found.sort_by_key(|l| !l.synced);
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn the_line_at_a_position() {
        let lyrics = Lyrics::from_text(
            "[00:01.00]a\n[00:03.00]b\n[00:05.00]c\n",
            LyricsSource::Embedded(EmbeddedKind::Other),
        )
        .expect("lyrics");
        assert!(lyrics.synced);
        assert_eq!(lyrics.line_at(ms(500), 0), None);
        assert_eq!(lyrics.line_at(ms(1000), 0), Some(0));
        assert_eq!(lyrics.line_at(ms(2999), 0), Some(0));
        assert_eq!(lyrics.line_at(ms(3000), 0), Some(1));
        assert_eq!(lyrics.line_at(ms(60_000), 0), Some(2));
        // Shown 300 ms sooner, or later.
        assert_eq!(lyrics.line_at(ms(2700), 300), Some(1));
        assert_eq!(lyrics.line_at(ms(3200), -300), Some(0));
        assert_eq!(lyrics.next_change(ms(1500), 0), Some(ms(1500)));
        assert_eq!(lyrics.next_change(ms(6000), 0), None);
    }

    #[test]
    fn plain_text_has_no_current_line() {
        let lyrics = Lyrics::from_text("one\ntwo", LyricsSource::Embedded(EmbeddedKind::Mp4))
            .expect("lyrics");
        assert!(!lyrics.synced);
        assert_eq!(lyrics.line_at(ms(1000), 0), None);
        assert!(Lyrics::from_text("  \n\n", LyricsSource::Embedded(EmbeddedKind::Mp4)).is_none());
    }
}
