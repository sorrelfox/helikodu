// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! PLS.
//!
//! Reading: the keys of the `[playlist]` section, in any case: `File<n>`, `Title<n>`,
//! `Length<n>` (seconds; -1 for unknown; a title `Artist - Title` is split as in M3U), `NumberOfEntries` and `Version`. Entries are
//! taken in the order of their numbers, which need not be consecutive. Other sections
//! are skipped; keys before any section are read with a warning. The charsets are those
//! of M3U.
//!
//! Writing: `[playlist]`, the three keys of each entry, `NumberOfEntries` and
//! `Version=2`, with LF line ends; paths as their own bytes.

use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt;
use std::time::Duration;

use super::entry::{EntryHints, EntryLocation, EntryReader, Playlist};
use super::location::{Syntax, foreign_for_writing, path_for_writing};
use super::m3u::{display_hints, one_line};
use super::text::{self, Charset};
use super::warning::{PlaylistWarningKind, Warnings, WriteWarning, WriteWarningKind};
use super::{
    ParseOptions, ParsedPlaylist, PlaylistError, PlaylistFormat, PlaylistKind, WriteOptions,
    WrittenPlaylist, check_input,
};

/// PLS.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pls;

impl PlaylistFormat for Pls {
    fn kind(&self) -> PlaylistKind {
        PlaylistKind::Pls
    }

    fn parse(&self, bytes: &[u8], options: &ParseOptions) -> Result<ParsedPlaylist, PlaylistError> {
        parse(bytes, options)
    }

    fn write(&self, playlist: &Playlist, options: &WriteOptions) -> WrittenPlaylist {
        write(playlist, options)
    }
}

/// What a file says about the entry with one number.
#[derive(Debug, Default)]
struct Slot {
    /// The line, the bytes and the text of `File<n>`.
    file: Option<(usize, Vec<u8>, String)>,
    title: Option<String>,
    duration: Option<Duration>,
    /// The first line that mentioned the entry.
    line: usize,
}

/// The section a line is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    Playlist,
    Other,
}

fn parse(bytes: &[u8], options: &ParseOptions) -> Result<ParsedPlaylist, PlaylistError> {
    let chosen = check_input(bytes, options)?;
    let mut warnings = Warnings::default();
    let (lines, encoding) = text::decode_lines(
        bytes,
        Charset::Detect,
        chosen,
        options.limits.max_line_bytes,
        &mut warnings,
    );
    let max_entries = options.limits.max_entries;
    let mut slots: BTreeMap<u64, Slot> = BTreeMap::new();
    let mut declared = None;
    let mut section = Section::None;
    let mut warned_section = false;
    let mut full = false;
    for line in &lines {
        let text = line.text.trim();
        if text.is_empty() || text.starts_with(';') || text.starts_with('#') {
            continue;
        }
        if text.starts_with('[') && text.ends_with(']') {
            let name = text[1..text.len() - 1].trim();
            section = if name.eq_ignore_ascii_case("playlist") {
                Section::Playlist
            } else {
                Section::Other
            };
            continue;
        }
        match section {
            Section::Other => continue,
            Section::None if !warned_section => {
                warned_section = true;
                warnings.at(line.number, PlaylistWarningKind::MissingSection);
            }
            _ => {}
        }
        let Some((key, value)) = text.split_once('=') else {
            warnings.at(line.number, PlaylistWarningKind::BadLine(short(text)));
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key.eq_ignore_ascii_case("NumberOfEntries") {
            match value.parse::<u64>() {
                Ok(n) => declared = Some(n),
                Err(_) => warnings.at(line.number, PlaylistWarningKind::BadNumber(short(value))),
            }
            continue;
        }
        if key.eq_ignore_ascii_case("Version") {
            continue;
        }
        let Some((field, number)) = numbered_key(key) else {
            warnings.at(line.number, PlaylistWarningKind::BadLine(short(text)));
            continue;
        };
        if !slots.contains_key(&number) && slots.len() >= max_entries {
            if !full {
                full = true;
                warnings.at(line.number, PlaylistWarningKind::TooManyEntries);
            }
            continue;
        }
        let slot = slots.entry(number).or_insert_with(|| Slot {
            line: line.number,
            ..Slot::default()
        });
        match field {
            Field::File => {
                if slot.file.is_some() {
                    warnings.at(line.number, PlaylistWarningKind::Repeated(key.to_owned()));
                }
                // The value's own bytes: after the first `=` of the line's bytes.
                let raw = line
                    .raw
                    .iter()
                    .position(|&b| b == b'=')
                    .map_or(&[][..], |eq| &line.raw[eq + 1..]);
                slot.file = Some((line.number, raw.trim_ascii().to_vec(), value.to_owned()));
            }
            Field::Title => slot.title = (!value.is_empty()).then(|| value.to_owned()),
            Field::Length => match value.parse::<i64>() {
                Ok(s) if s > 0 => slot.duration = Some(Duration::from_secs(s.unsigned_abs())),
                Ok(_) => slot.duration = None,
                Err(_) => {
                    warnings.at(line.number, PlaylistWarningKind::BadDuration(short(value)));
                }
            },
        }
    }
    let mut reader = EntryReader::new(
        options.base_dir.as_deref(),
        Syntax::Path,
        encoding,
        max_entries,
    );
    let found = slots.values().filter(|slot| slot.file.is_some()).count();
    for slot in slots.into_values() {
        let Some((line, raw, text)) = slot.file else {
            warnings.at(slot.line, PlaylistWarningKind::OrphanInformation);
            continue;
        };
        let mut hints = EntryHints {
            duration: slot.duration,
            ..EntryHints::default()
        };
        if let Some(title) = &slot.title {
            display_hints(title, &mut hints);
        }
        reader.push(&raw, &text, line, hints, &mut warnings);
    }
    if let Some(declared) = declared
        && declared != found as u64
    {
        warnings.push(
            None,
            PlaylistWarningKind::EntryCountMismatch { declared, found },
        );
    }
    Ok(ParsedPlaylist {
        playlist: Playlist {
            title: None,
            entries: reader.into_entries(),
        },
        kind: PlaylistKind::Pls,
        encoding,
        warnings: warnings.into_vec(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    File,
    Title,
    Length,
}

/// `File12` as ([`Field::File`], 12), in any case.
fn numbered_key(key: &str) -> Option<(Field, u64)> {
    let digits = key.find(|c: char| c.is_ascii_digit())?;
    let (name, number) = key.split_at(digits);
    let field = if name.eq_ignore_ascii_case("File") {
        Field::File
    } else if name.eq_ignore_ascii_case("Title") {
        Field::Title
    } else if name.eq_ignore_ascii_case("Length") {
        Field::Length
    } else {
        return None;
    };
    Some((field, number.parse().ok()?))
}

/// The start of `text`, for a warning.
fn short(text: &str) -> String {
    text.chars().take(80).collect()
}

fn write(playlist: &Playlist, options: &WriteOptions) -> WrittenPlaylist {
    let mut out = b"[playlist]\n".to_vec();
    let mut warnings = Vec::new();
    let mut number = 0usize;
    for (index, entry) in playlist.entries.iter().enumerate() {
        let location = match &entry.location {
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
        number += 1;
        out.extend_from_slice(format!("File{number}=").as_bytes());
        out.extend_from_slice(&location);
        out.push(b'\n');
        if let Some(title) = &entry.hints.title {
            let title = match &entry.hints.artist {
                Some(artist) => format!("{} - {}", one_line(artist), one_line(title)),
                None => one_line(title),
            };
            out.extend_from_slice(format!("Title{number}={title}\n").as_bytes());
        }
        let seconds = entry
            .hints
            .duration
            .map_or(-1, |d| d.as_secs_f64().round() as i64);
        out.extend_from_slice(format!("Length{number}={seconds}\n").as_bytes());
    }
    out.extend_from_slice(format!("NumberOfEntries={number}\nVersion=2\n").as_bytes());
    WrittenPlaylist {
        bytes: out,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playlists::entry::PlaylistEntry;

    fn options() -> ParseOptions {
        ParseOptions {
            base_dir: Some("/music".into()),
            ..ParseOptions::default()
        }
    }

    #[test]
    fn winamp_style_file() {
        let file = b"[playlist]\r\nfile2=B\\02.mp3\r\nFile1=A\\01.mp3\r\nTitle1=One\r\n\
            Length1=180\r\nLength2=-1\r\nFile10=http://radio.example/stream\r\n\
            NumberOfEntries=3\r\nVersion=2\r\n[other]\r\nFile4=x.mp3\r\n";
        let parsed = Pls.parse(file, &options()).unwrap();
        let entries = &parsed.playlist.entries;
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0].location,
            EntryLocation::Local("/music/A/01.mp3".into())
        );
        assert_eq!(entries[0].hints.title.as_deref(), Some("One"));
        assert_eq!(entries[0].hints.duration, Some(Duration::from_secs(180)));
        assert_eq!(
            entries[1].location,
            EntryLocation::Local("/music/B/02.mp3".into())
        );
        assert_eq!(entries[1].hints.duration, None);
        assert!(matches!(entries[2].location, EntryLocation::Remote(_)));
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
        assert!(matches!(
            parsed.warnings[0].kind,
            PlaylistWarningKind::RemoteEntry(_)
        ));
    }

    #[test]
    fn sloppy_files_warn() {
        let file =
            b"File1=a.mp3\ngarbage\nTitle2=no file\nFile1=b.mp3\nLength1=x\nNumberOfEntries=5\n";
        let parsed = Pls.parse(file, &options()).unwrap();
        assert_eq!(parsed.playlist.entries.len(), 1);
        assert_eq!(
            parsed.playlist.entries[0].location,
            EntryLocation::Local("/music/b.mp3".into())
        );
        let kinds: Vec<_> = parsed.warnings.iter().map(|w| w.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                PlaylistWarningKind::MissingSection,
                PlaylistWarningKind::BadLine("garbage".into()),
                PlaylistWarningKind::Repeated("File1".into()),
                PlaylistWarningKind::BadDuration("x".into()),
                PlaylistWarningKind::OrphanInformation,
                PlaylistWarningKind::EntryCountMismatch {
                    declared: 5,
                    found: 1
                },
            ]
        );
    }

    #[test]
    fn huge_numbers_stay_bounded() {
        let mut file = b"[playlist]\n".to_vec();
        for n in 0..50u64 {
            file.extend_from_slice(format!("File{}=a{n}.mp3\n", u64::MAX - n).as_bytes());
        }
        file.extend_from_slice(b"File99999999999999999999999=x.mp3\n");
        let parsed = Pls
            .parse(
                &file,
                &ParseOptions {
                    limits: crate::playlists::PlaylistLimits {
                        max_entries: 10,
                        ..Default::default()
                    },
                    ..options()
                },
            )
            .unwrap();
        assert_eq!(parsed.playlist.entries.len(), 10);
    }

    #[test]
    fn writing_round_trips() {
        let mut entry = PlaylistEntry::local("/music/A/01.mp3");
        entry.hints.title = Some("One".into());
        entry.hints.duration = Some(Duration::from_secs(61));
        let playlist = Playlist {
            title: None,
            entries: vec![entry, PlaylistEntry::local("/elsewhere/02.mp3")],
        };
        let written = Pls.write(
            &playlist,
            &WriteOptions {
                base_dir: Some("/music".into()),
                ..WriteOptions::default()
            },
        );
        assert_eq!(
            String::from_utf8(written.bytes.clone()).unwrap(),
            "[playlist]\nFile1=A/01.mp3\nTitle1=One\nLength1=61\nFile2=/elsewhere/02.mp3\n\
             Length2=-1\nNumberOfEntries=2\nVersion=2\n"
        );
        let back = Pls.parse(&written.bytes, &options()).unwrap();
        assert!(back.warnings.is_empty());
        assert_eq!(
            back.playlist.entries[0].location,
            playlist.entries[0].location
        );
        assert_eq!(back.playlist.entries[0].hints, playlist.entries[0].hints);
        assert_eq!(
            back.playlist.entries[1].location,
            playlist.entries[1].location
        );
    }
}
