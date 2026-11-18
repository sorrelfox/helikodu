// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Playlist files: M3U and M3U8, PLS and XSPF.
//!
//! Each format is one [`PlaylistFormat`], reached through [`PlaylistKind`]. Parsing
//! turns a file's bytes into a [`Playlist`] of [`PlaylistEntry`]s; writing turns a
//! playlist back into bytes. The parsers are this crate's own (XSPF on top of
//! `quick-xml`): tolerant of the files found in the wild, bounded in what they accept
//! ([`PlaylistLimits`]), and never panicking. What they skip or repair is reported as
//! line-numbered [`PlaylistWarning`]s.
//!
//! An entry is a local path ([`EntryLocation::Local`], as bytes), a path written for
//! another system ([`EntryLocation::Foreign`]: a drive letter or a network share), or a
//! URL that is not a local file ([`EntryLocation::Remote`]). URLs are kept as
//! unsupported entries and never fetched. Relative paths are resolved against the
//! playlist's folder and `file://` URLs are decoded into paths. Entries that name
//! other playlists are not followed.
//!
//! Charsets: a byte order mark decides; M3U8 and XSPF are UTF-8; an M3U or PLS file
//! that is not valid UTF-8 is read in the legacy charset the user chose or else a
//! guessed one, and the original bytes of each path are kept beside the decoded path
//! ([`PlaylistEntry::undecoded_path`]).

mod entry;
mod location;
mod m3u;
mod pls;
mod text;
mod warning;
mod xspf;

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub use entry::{EntryHints, EntryLocation, Playlist, PlaylistEntry};
pub use location::{LocationProblem, PathStyle};
pub use m3u::{M3u, M3u8};
pub use pls::Pls;
pub use text::{EncodingSource, TextEncoding};
pub use warning::{PlaylistWarning, PlaylistWarningKind, WriteWarning, WriteWarningKind};
pub use xspf::Xspf;

/// The bounds a parser keeps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaylistLimits {
    /// The largest file read, in bytes.
    pub max_bytes: u64,
    /// The most entries kept; the rest are skipped with a warning.
    pub max_entries: usize,
    /// The longest line (or XML text) accepted, in bytes; longer ones are skipped.
    pub max_line_bytes: usize,
}

impl Default for PlaylistLimits {
    /// 16 MiB, 100 000 entries, 32 KiB per line.
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_entries: 100_000,
            max_line_bytes: 32 * 1024,
        }
    }
}

/// How to parse a playlist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseOptions {
    /// The folder the playlist is in. Relative entries are resolved against it; without
    /// it they stay relative.
    pub base_dir: Option<PathBuf>,
    /// The legacy charset the user chose for M3U and PLS files that are neither UTF-8
    /// nor marked by a byte order mark, as a WHATWG label such as `windows-1251`. When
    /// unset, the charset is guessed.
    pub legacy_charset: Option<String>,
    /// The bounds the parser keeps to.
    pub limits: PlaylistLimits,
}

/// How to write a playlist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteOptions {
    /// Whether local paths are written relative to `base_dir` or absolute.
    pub path_style: PathStyle,
    /// The folder the playlist will be in; relative paths are relative to it.
    pub base_dir: Option<PathBuf>,
}

/// A parsed playlist with what the parser noticed.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPlaylist {
    /// The playlist.
    pub playlist: Playlist,
    /// The format it was parsed as.
    pub kind: PlaylistKind,
    /// The charset its text was decoded with.
    pub encoding: TextEncoding,
    /// What was skipped or repaired, in order.
    pub warnings: Vec<PlaylistWarning>,
}

/// A playlist written into bytes, with what could not be written as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenPlaylist {
    /// The file's contents.
    pub bytes: Vec<u8>,
    /// The entries that were changed or left out.
    pub warnings: Vec<WriteWarning>,
}

/// Why a playlist could not be read at all.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlaylistError {
    /// The file is larger than [`PlaylistLimits::max_bytes`].
    #[error("the playlist is larger than {limit} bytes")]
    TooLarge {
        /// The limit.
        limit: u64,
    },
    /// The chosen legacy charset is not a known WHATWG label.
    #[error("unknown charset {0:?}")]
    UnknownCharset(String),
    /// The file could not be read.
    #[error("cannot read {}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        #[source]
        source: io::Error,
    },
}

/// A playlist file format: the seam new formats plug into.
pub trait PlaylistFormat: fmt::Debug + Sync {
    /// Which format this is.
    fn kind(&self) -> PlaylistKind;

    /// Parses `bytes`. Never panics; only a file over the size limit or an unknown
    /// charset label fails, and anything else malformed is skipped with a warning.
    fn parse(&self, bytes: &[u8], options: &ParseOptions) -> Result<ParsedPlaylist, PlaylistError>;

    /// Writes `playlist`.
    fn write(&self, playlist: &Playlist, options: &WriteOptions) -> WrittenPlaylist;
}

/// The playlist formats.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PlaylistKind {
    /// M3U in a legacy charset or UTF-8, with or without `#EXTM3U`.
    M3u,
    /// M3U in UTF-8: the default for writing.
    #[default]
    M3u8,
    /// PLS (`[playlist]`, `File1=`).
    Pls,
    /// XSPF, the XML Shareable Playlist Format.
    Xspf,
}

impl PlaylistKind {
    /// Every format.
    pub const ALL: [Self; 4] = [Self::M3u, Self::M3u8, Self::Pls, Self::Xspf];

    /// The implementation of the format.
    pub fn format(self) -> &'static dyn PlaylistFormat {
        match self {
            Self::M3u => &M3u,
            Self::M3u8 => &M3u8,
            Self::Pls => &Pls,
            Self::Xspf => &Xspf,
        }
    }

    /// The file name extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::M3u => "m3u",
            Self::M3u8 => "m3u8",
            Self::Pls => "pls",
            Self::Xspf => "xspf",
        }
    }

    /// The format a file name's extension stands for, ignoring case.
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.as_encoded_bytes();
        Self::ALL
            .into_iter()
            .find(|kind| extension.eq_ignore_ascii_case(kind.extension().as_bytes()))
    }

    /// The format `bytes` look like: XML is XSPF, a `[playlist]` section is PLS, and
    /// anything else is M3U.
    pub fn sniff(bytes: &[u8]) -> Self {
        let head = &bytes[..bytes.len().min(4096)];
        let (text, _) = text::decode_head(head);
        let start = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
        if start.starts_with('<') {
            return Self::Xspf;
        }
        let first = start.lines().next().unwrap_or("").trim();
        if first.eq_ignore_ascii_case("[playlist]") {
            Self::Pls
        } else {
            Self::M3u
        }
    }

    /// Whether `path`'s extension is that of a playlist format, including those not
    /// read yet (JSPF, ASX, WPL). Entries naming them are not followed.
    pub fn is_playlist_path(path: &[u8]) -> bool {
        let Some(dot) = path.iter().rposition(|&b| b == b'.') else {
            return false;
        };
        if path[dot..].contains(&b'/') {
            return false;
        }
        let extension = &path[dot + 1..];
        ["m3u", "m3u8", "pls", "xspf", "jspf", "asx", "wpl", "wax"]
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known.as_bytes()))
    }
}

impl fmt::Display for PlaylistKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::M3u => "M3U",
            Self::M3u8 => "M3U8",
            Self::Pls => "PLS",
            Self::Xspf => "XSPF",
        })
    }
}

/// Parses `bytes` as `kind`, or as the format they look like.
pub fn parse_playlist(
    bytes: &[u8],
    kind: Option<PlaylistKind>,
    options: &ParseOptions,
) -> Result<ParsedPlaylist, PlaylistError> {
    let kind = kind.unwrap_or_else(|| PlaylistKind::sniff(bytes));
    kind.format().parse(bytes, options)
}

/// Reads the playlist file at `path`, in the format its extension names or else the
/// one its contents look like. Relative entries are resolved against its folder
/// (`options.base_dir` is replaced).
pub fn read_playlist(path: &Path, options: &ParseOptions) -> Result<ParsedPlaylist, PlaylistError> {
    let io_error = |source| PlaylistError::Io {
        path: path.to_owned(),
        source,
    };
    let limit = options.limits.max_bytes;
    let file = File::open(path).map_err(io_error)?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > limit {
        return Err(PlaylistError::TooLarge { limit });
    }
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let options = ParseOptions {
        base_dir: absolute.parent().map(Path::to_owned),
        ..options.clone()
    };
    parse_playlist(&bytes, PlaylistKind::from_path(path), &options)
}

/// Writes `playlist` in the format `kind`.
pub fn write_playlist(
    playlist: &Playlist,
    kind: PlaylistKind,
    options: &WriteOptions,
) -> WrittenPlaylist {
    kind.format().write(playlist, options)
}

/// Checks the size limit and the charset label shared by every parser.
pub(crate) fn check_input(
    bytes: &[u8],
    options: &ParseOptions,
) -> Result<Option<&'static encoding_rs::Encoding>, PlaylistError> {
    if bytes.len() as u64 > options.limits.max_bytes {
        return Err(PlaylistError::TooLarge {
            limit: options.limits.max_bytes,
        });
    }
    match &options.legacy_charset {
        None => Ok(None),
        Some(label) => encoding_rs::Encoding::for_label(label.trim().as_bytes())
            .map(Some)
            .ok_or_else(|| PlaylistError::UnknownCharset(label.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_come_from_extensions() {
        assert_eq!(
            PlaylistKind::from_path(Path::new("/a/B.M3U")),
            Some(PlaylistKind::M3u)
        );
        assert_eq!(
            PlaylistKind::from_path(Path::new("x.m3u8")),
            Some(PlaylistKind::M3u8)
        );
        assert_eq!(
            PlaylistKind::from_path(Path::new("x.Pls")),
            Some(PlaylistKind::Pls)
        );
        assert_eq!(
            PlaylistKind::from_path(Path::new("x.xspf")),
            Some(PlaylistKind::Xspf)
        );
        assert_eq!(PlaylistKind::from_path(Path::new("x.txt")), None);
    }

    #[test]
    fn kinds_are_sniffed() {
        assert_eq!(
            PlaylistKind::sniff(b"\xef\xbb\xbf<?xml version=\"1.0\"?><playlist/>"),
            PlaylistKind::Xspf
        );
        assert_eq!(
            PlaylistKind::sniff(b"\r\n[Playlist]\r\nFile1=a.mp3"),
            PlaylistKind::Pls
        );
        assert_eq!(PlaylistKind::sniff(b"#EXTM3U\n"), PlaylistKind::M3u);
        assert_eq!(PlaylistKind::sniff(b""), PlaylistKind::M3u);
    }

    #[test]
    fn playlist_paths_are_recognized() {
        assert!(PlaylistKind::is_playlist_path(b"/a/b.M3U8"));
        assert!(PlaylistKind::is_playlist_path(b"b.wpl"));
        assert!(!PlaylistKind::is_playlist_path(b"/a.m3u/b.flac"));
        assert!(!PlaylistKind::is_playlist_path(b"m3u"));
    }

    #[test]
    fn oversized_input_is_refused() {
        let options = ParseOptions {
            limits: PlaylistLimits {
                max_bytes: 4,
                ..PlaylistLimits::default()
            },
            ..ParseOptions::default()
        };
        for kind in PlaylistKind::ALL {
            assert!(matches!(
                kind.format().parse(b"12345", &options),
                Err(PlaylistError::TooLarge { limit: 4 })
            ));
        }
    }

    #[test]
    fn unknown_charsets_are_refused() {
        let options = ParseOptions {
            legacy_charset: Some("klingon".into()),
            ..ParseOptions::default()
        };
        assert!(matches!(
            parse_playlist(b"a.mp3", None, &options),
            Err(PlaylistError::UnknownCharset(_))
        ));
    }
}
