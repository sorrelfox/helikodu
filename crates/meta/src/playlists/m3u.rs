// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! M3U and M3U8.
//!
//! Reading: one location per line; blank lines and `#` comments are skipped.
//! `#EXTINF:<seconds>[ attributes],<artist> - <title>` describes the next entry, as do
//! `#EXTART:` and `#EXTALB:`; `#PLAYLIST:` is the title. `#EXTM3U` is not required. An
//! M3U file may be in a legacy charset; an M3U8 file is UTF-8.
//!
//! Writing: `#EXTM3U`, `#PLAYLIST:` when the playlist has a title, `#EXTINF` for every
//! entry with a title or a length, and the location, with LF line ends. M3U8 writes a
//! path that is not UTF-8 as decoded text and warns that it may not resolve; M3U
//! writes the path's own bytes. A path holding a line break cannot be written.

use std::os::unix::ffi::OsStrExt;
use std::time::Duration;

use super::entry::{EntryHints, EntryLocation, EntryReader, Playlist};
use super::location::{Syntax, foreign_for_writing, path_for_writing};
use super::text::{self, Charset};
use super::warning::{PlaylistWarningKind, Warnings, WriteWarning, WriteWarningKind};
use super::{
    ParseOptions, ParsedPlaylist, PlaylistError, PlaylistFormat, PlaylistKind, WriteOptions,
    WrittenPlaylist, check_input,
};

/// M3U: a legacy charset or UTF-8.
#[derive(Debug, Clone, Copy, Default)]
pub struct M3u;

/// M3U8: UTF-8.
#[derive(Debug, Clone, Copy, Default)]
pub struct M3u8;

impl PlaylistFormat for M3u {
    fn kind(&self) -> PlaylistKind {
        PlaylistKind::M3u
    }

    fn parse(&self, bytes: &[u8], options: &ParseOptions) -> Result<ParsedPlaylist, PlaylistError> {
        parse(bytes, options, PlaylistKind::M3u)
    }

    fn write(&self, playlist: &Playlist, options: &WriteOptions) -> WrittenPlaylist {
        write(playlist, options, false)
    }
}

impl PlaylistFormat for M3u8 {
    fn kind(&self) -> PlaylistKind {
        PlaylistKind::M3u8
    }

    fn parse(&self, bytes: &[u8], options: &ParseOptions) -> Result<ParsedPlaylist, PlaylistError> {
        parse(bytes, options, PlaylistKind::M3u8)
    }

    fn write(&self, playlist: &Playlist, options: &WriteOptions) -> WrittenPlaylist {
        write(playlist, options, true)
    }
}

/// The longest length accepted, in seconds: about four months.
const MAX_SECONDS: f64 = 1e7;

fn parse(
    bytes: &[u8],
    options: &ParseOptions,
    kind: PlaylistKind,
) -> Result<ParsedPlaylist, PlaylistError> {
    let chosen = check_input(bytes, options)?;
    let charset = match kind {
        PlaylistKind::M3u8 => Charset::Utf8,
        _ => Charset::Detect,
    };
    let mut warnings = Warnings::default();
    let (lines, encoding) = text::decode_lines(
        bytes,
        charset,
        chosen,
        options.limits.max_line_bytes,
        &mut warnings,
    );
    let mut reader = EntryReader::new(
        options.base_dir.as_deref(),
        Syntax::Path,
        encoding,
        options.limits.max_entries,
    );
    let mut title = None;
    let mut pending: Option<(usize, EntryHints)> = None;
    for line in &lines {
        let text = line.text.trim();
        if text.is_empty() {
            continue;
        }
        if let Some(directive) = text.strip_prefix('#') {
            let hints = &mut pending
                .get_or_insert_with(|| (line.number, EntryHints::default()))
                .1;
            if let Some(info) = strip_directive(directive, "EXTINF:") {
                extinf(info, line.number, hints, &mut warnings);
            } else if let Some(artist) = strip_directive(directive, "EXTART:") {
                hints.artist = non_empty(artist);
            } else if let Some(album) = strip_directive(directive, "EXTALB:") {
                hints.album = non_empty(album);
            } else if let Some(name) = strip_directive(directive, "PLAYLIST:")
                && title.is_none()
            {
                title = non_empty(name);
            }
            if pending.as_ref().is_some_and(|(_, hints)| hints.is_empty()) {
                pending = None;
            }
            continue;
        }
        if reader.is_full() {
            // Still warns once, and stops reading.
            reader.push(&[], "", line.number, EntryHints::default(), &mut warnings);
            pending = None;
            break;
        }
        let hints = pending.take().map(|(_, hints)| hints).unwrap_or_default();
        let raw = line.raw.trim_ascii();
        reader.push(raw, text, line.number, hints, &mut warnings);
    }
    if let Some((line, _)) = pending {
        warnings.at(line, PlaylistWarningKind::OrphanInformation);
    }
    Ok(ParsedPlaylist {
        playlist: Playlist {
            title,
            entries: reader.into_entries(),
        },
        kind,
        encoding,
        warnings: warnings.into_vec(),
    })
}

/// `directive` without the `name` it starts with, in any case.
fn strip_directive<'a>(directive: &'a str, name: &str) -> Option<&'a str> {
    let head = directive.get(..name.len())?;
    head.eq_ignore_ascii_case(name)
        .then(|| directive[name.len()..].trim())
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Reads `#EXTINF:<seconds>[ key="value" …],<display>` into `hints`.
fn extinf(info: &str, line: usize, hints: &mut EntryHints, warnings: &mut Warnings) {
    // The display text follows the first comma outside quotes.
    let mut quoted = false;
    let comma = info.char_indices().find_map(|(i, c)| match c {
        '"' => {
            quoted = !quoted;
            None
        }
        ',' if !quoted => Some(i),
        _ => None,
    });
    let (head, display) = match comma {
        Some(i) => (&info[..i], &info[i + 1..]),
        None => (info, ""),
    };
    let seconds = head.split_whitespace().next().unwrap_or("");
    if !seconds.is_empty() {
        match seconds.parse::<f64>() {
            Ok(s) if s.is_finite() && s <= MAX_SECONDS => {
                // -1 (and 0) mean "unknown".
                hints.duration = (s > 0.0).then(|| Duration::from_secs_f64(s));
            }
            _ => warnings.at(line, PlaylistWarningKind::BadDuration(seconds.to_owned())),
        }
    }
    display_hints(display, hints);
}

/// Reads `Artist - Title`, or a title alone, into `hints`.
pub(crate) fn display_hints(display: &str, hints: &mut EntryHints) {
    let display = display.trim();
    if display.is_empty() {
        return;
    }
    match display.split_once(" - ") {
        Some((artist, title)) if !artist.trim().is_empty() && !title.trim().is_empty() => {
            hints.artist = Some(artist.trim().to_owned());
            hints.title = Some(title.trim().to_owned());
        }
        _ => hints.title = Some(display.to_owned()),
    }
}

fn write(playlist: &Playlist, options: &WriteOptions, utf8: bool) -> WrittenPlaylist {
    let mut out = b"#EXTM3U\n".to_vec();
    let mut warnings = Vec::new();
    if let Some(title) = &playlist.title {
        out.extend_from_slice(b"#PLAYLIST:");
        out.extend_from_slice(one_line(title).as_bytes());
        out.push(b'\n');
    }
    for (index, entry) in playlist.entries.iter().enumerate() {
        let mut location = match &entry.location {
            EntryLocation::Local(path) => path_for_writing(path, options)
                .as_os_str()
                .as_bytes()
                .to_vec(),
            EntryLocation::Foreign(path) => foreign_for_writing(path),
            EntryLocation::Remote(url) => url.as_bytes().to_vec(),
        };
        if location.iter().any(|&b| b == b'\n' || b == b'\r') {
            warnings.push(WriteWarning {
                entry: index,
                kind: WriteWarningKind::Unrepresentable,
            });
            continue;
        }
        if utf8 && let Err(_) = std::str::from_utf8(&location) {
            location = String::from_utf8_lossy(&location).into_owned().into_bytes();
            warnings.push(WriteWarning {
                entry: index,
                kind: WriteWarningKind::NotUtf8,
            });
        }
        // A relative name starting with `#` would read as a comment.
        if location.first() == Some(&b'#') {
            location.splice(0..0, *b"./");
        }
        let hints = &entry.hints;
        if hints.title.is_some() || hints.duration.is_some() {
            let seconds = hints
                .duration
                .map_or(-1, |d| d.as_secs_f64().round() as i64);
            let display = match (&hints.artist, &hints.title) {
                (Some(artist), Some(title)) => {
                    format!("{} - {}", one_line(artist), one_line(title))
                }
                (None, Some(title)) => one_line(title),
                (_, None) => String::new(),
            };
            out.extend_from_slice(format!("#EXTINF:{seconds},{display}\n").as_bytes());
        }
        if let Some(album) = &hints.album {
            out.extend_from_slice(format!("#EXTALB:{}\n", one_line(album)).as_bytes());
        }
        out.extend_from_slice(&location);
        out.push(b'\n');
    }
    WrittenPlaylist {
        bytes: out,
        warnings,
    }
}

/// `text` with line breaks and other control characters as spaces.
pub(crate) fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::playlists::entry::PlaylistEntry;

    fn options() -> ParseOptions {
        ParseOptions {
            base_dir: Some("/music/lists".into()),
            ..ParseOptions::default()
        }
    }

    #[test]
    fn extended_m3u() {
        let file = "\u{feff}#EXTM3U\r\n#PLAYLIST:Road trip\r\n#EXTINF:215,Kino - Gruppa krovi\r\n\
            ../Kino/01.flac\r\n\r\n# a comment\r\n#EXTINF:-1 tvg-id=\"a,b\",Only a title\r\n\
            /abs/02.mp3\r\nplain.ogg\r\n";
        let parsed = M3u8.parse(file.as_bytes(), &options()).unwrap();
        assert_eq!(parsed.playlist.title.as_deref(), Some("Road trip"));
        let entries = &parsed.playlist.entries;
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0].location,
            EntryLocation::Local("/music/Kino/01.flac".into())
        );
        assert_eq!(entries[0].hints.artist.as_deref(), Some("Kino"));
        assert_eq!(entries[0].hints.title.as_deref(), Some("Gruppa krovi"));
        assert_eq!(entries[0].hints.duration, Some(Duration::from_secs(215)));
        assert_eq!(entries[0].line, Some(4));
        assert_eq!(entries[1].hints.title.as_deref(), Some("Only a title"));
        assert_eq!(entries[1].hints.duration, None);
        assert!(entries[2].hints.is_empty());
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    }

    #[test]
    fn bad_lengths_and_orphans_warn() {
        let parsed = M3u8
            .parse(
                b"#EXTINF:abc,X\na.mp3\n#EXTINF:1e999,Y\nb.mp3\n#EXTINF:3,Z\n",
                &options(),
            )
            .unwrap();
        assert_eq!(parsed.playlist.entries.len(), 2);
        let kinds: Vec<_> = parsed
            .warnings
            .iter()
            .map(|w| (w.line, w.kind.clone()))
            .collect();
        assert_eq!(
            kinds,
            [
                (Some(1), PlaylistWarningKind::BadDuration("abc".into())),
                (Some(3), PlaylistWarningKind::BadDuration("1e999".into())),
                (Some(5), PlaylistWarningKind::OrphanInformation),
            ]
        );
    }

    #[test]
    fn writing_round_trips() {
        let mut first = PlaylistEntry::local("/music/A/01 #1.flac");
        first.hints = EntryHints {
            title: Some("Title\nwith break".into()),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            duration: Some(Duration::from_millis(61_400)),
            track_number: None,
        };
        let playlist = Playlist {
            title: Some("Mix".into()),
            entries: vec![
                first,
                PlaylistEntry::local("/music/lists/#hash.mp3"),
                PlaylistEntry::new(EntryLocation::Remote("http://x/y.mp3".into())),
                PlaylistEntry::new(EntryLocation::Foreign(b"C:/M/a.mp3".to_vec())),
                PlaylistEntry::local("/music/new\nline.mp3"),
            ],
        };
        let written = M3u8.write(
            &playlist,
            &WriteOptions {
                base_dir: Some("/music/lists".into()),
                ..WriteOptions::default()
            },
        );
        let text = String::from_utf8(written.bytes.clone()).unwrap();
        assert_eq!(
            text,
            "#EXTM3U\n#PLAYLIST:Mix\n#EXTINF:61,Artist - Title with break\n#EXTALB:Album\n\
             ../A/01 #1.flac\n./#hash.mp3\nhttp://x/y.mp3\nC:\\M\\a.mp3\n"
        );
        assert_eq!(
            written.warnings,
            [WriteWarning {
                entry: 4,
                kind: WriteWarningKind::Unrepresentable
            }]
        );
        let back = M3u8.parse(&written.bytes, &options()).unwrap();
        assert_eq!(back.playlist.title.as_deref(), Some("Mix"));
        let locations: Vec<_> = back
            .playlist
            .entries
            .iter()
            .map(|e| e.location.clone())
            .collect();
        assert_eq!(
            locations,
            [
                EntryLocation::Local("/music/A/01 #1.flac".into()),
                EntryLocation::Local("/music/lists/#hash.mp3".into()),
                EntryLocation::Remote("http://x/y.mp3".into()),
                EntryLocation::Foreign(b"C:/M/a.mp3".to_vec()),
            ]
        );
        assert_eq!(
            back.playlist.entries[0].hints.album.as_deref(),
            Some("Album")
        );
    }

    #[test]
    fn non_utf8_paths() {
        use std::ffi::OsStr;
        let path = PathBuf::from(OsStr::from_bytes(b"/m/Caf\xe9.mp3"));
        let playlist = Playlist {
            title: None,
            entries: vec![PlaylistEntry::local(path.clone())],
        };
        let absolute = WriteOptions::default();
        let utf8 = M3u8.write(&playlist, &absolute);
        assert_eq!(utf8.warnings[0].kind, WriteWarningKind::NotUtf8);
        let legacy = M3u.write(&playlist, &absolute);
        assert!(legacy.warnings.is_empty());
        assert!(legacy.bytes.ends_with(b"/m/Caf\xe9.mp3\n"));
        // Read back as M3U8, the bytes are the name.
        let back = M3u8.parse(&legacy.bytes, &ParseOptions::default()).unwrap();
        assert_eq!(
            back.playlist.entries[0].location.local_path(),
            Some(path.as_path())
        );
        // Read back as M3U, the guessed charset decodes it and the bytes stay beside.
        let back = M3u.parse(&legacy.bytes, &ParseOptions::default()).unwrap();
        let paths: Vec<&Path> = back.playlist.entries[0].local_paths().collect();
        assert!(paths.contains(&path.as_path()), "{paths:?}");
    }
}
