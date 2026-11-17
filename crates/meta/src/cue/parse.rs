// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The line parser.
//!
//! The parser is tolerant: commands are case-insensitive, lines end in LF, CRLF or
//! CR, values may be quoted or not, a quote inside a quoted value is kept (`TITLE
//! "Say "Hi""`), and anything it does not understand is skipped with a warning.
//! It is also bounded: the sheet, its lines, and its numbers of files, tracks,
//! indexes and remarks all have limits, so hostile input costs little.

use super::sheet::{CueFile, CueFlags, CueIndex, CueSheet, CueTime, CueTrack, Remark};
use super::text::SheetEncoding;
use super::warning::{CueWarningKind as Kind, Warnings};

/// The largest sheet accepted, in bytes.
pub const MAX_SHEET_BYTES: usize = 1 << 20;
/// The longest line accepted, in bytes.
const MAX_LINE_BYTES: usize = 8 * 1024;
/// The most `FILE` lines kept.
const MAX_FILES: usize = 1000;
/// The most tracks kept.
const MAX_TRACKS: usize = 1000;
/// The most indexes kept per track (`INDEX 00` to `INDEX 99`).
const MAX_INDEXES: usize = 100;
/// The most remarks kept for the album or for one track.
const MAX_REMARKS: usize = 256;
/// The highest track number accepted.
const MAX_TRACK_NUMBER: u32 = 9999;
/// The highest index number accepted.
const MAX_INDEX_NUMBER: u32 = 99;

/// CD-TEXT keywords that some programs write as commands of their own; they are
/// kept as remarks.
const CD_TEXT_KEYWORDS: [&str; 6] = [
    "ARRANGER", "COMPOSER", "DISC_ID", "GENRE", "MESSAGE", "UPC_EAN",
];

/// Parses decoded sheet text. `text` is at most [`MAX_SHEET_BYTES`] long; the
/// caller checks.
pub(crate) fn parse_text(text: &str, encoding: SheetEncoding) -> CueSheet {
    let mut parser = Parser {
        sheet: CueSheet {
            catalog: None,
            cd_text_file: None,
            title: None,
            performer: None,
            songwriter: None,
            remarks: Vec::new(),
            files: Vec::new(),
            tracks: Vec::new(),
            encoding,
            warnings: Vec::new(),
        },
        warnings: Warnings::default(),
        file: None,
        in_track: false,
        skipping_track: false,
        remarks_overflowed: false,
    };
    if encoding.had_errors {
        parser.warnings.push(
            None,
            Kind::UndecodableText {
                encoding: encoding.name(),
            },
        );
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for (i, line) in lines(text).enumerate() {
        parser.line(i + 1, line);
    }
    let mut sheet = parser.sheet;
    sheet.warnings = parser.warnings.into_vec();
    sheet
}

/// The lines of `text`, ended by LF, CRLF or CR.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = Some(text);
    std::iter::from_fn(move || {
        let current = rest?;
        match current.find(['\n', '\r']) {
            Some(end) => {
                let skip = if current[end..].starts_with("\r\n") {
                    2
                } else {
                    1
                };
                rest = Some(&current[end + skip..]);
                Some(&current[..end])
            }
            None => {
                rest = None;
                (!current.is_empty()).then_some(current)
            }
        }
    })
}

struct Parser {
    sheet: CueSheet,
    warnings: Warnings,
    /// The current `FILE`, as an index into `sheet.files`.
    file: Option<usize>,
    /// Whether a `TRACK` has been seen.
    in_track: bool,
    /// Whether the current `TRACK` line was skipped: its commands are skipped too.
    skipping_track: bool,
    /// Whether the album or the current track has more remarks than are kept.
    remarks_overflowed: bool,
}

impl Parser {
    fn line(&mut self, n: usize, line: &str) {
        if line.len() > MAX_LINE_BYTES {
            self.warnings.at(n, Kind::LineTooLong);
            return;
        }
        let line = line.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        if line.is_empty() {
            return;
        }
        let (command, rest) = match line.split_once(char::is_whitespace) {
            Some((c, r)) => (c, r.trim()),
            None => (line, ""),
        };
        let command = command.to_ascii_uppercase();
        match command.as_str() {
            "REM" => self.rem(n, rest),
            "CATALOG" => {
                if let Some(v) = self.value(n, "CATALOG", rest) {
                    set(&mut self.warnings, n, "CATALOG", &mut self.sheet.catalog, v);
                }
            }
            "CDTEXTFILE" => {
                if let Some(v) = self.value(n, "CDTEXTFILE", rest) {
                    set(
                        &mut self.warnings,
                        n,
                        "CDTEXTFILE",
                        &mut self.sheet.cd_text_file,
                        v,
                    );
                }
            }
            "TITLE" => self.text_field(n, "TITLE", rest),
            "PERFORMER" => self.text_field(n, "PERFORMER", rest),
            "SONGWRITER" => self.text_field(n, "SONGWRITER", rest),
            "FILE" => self.file_line(n, rest),
            "TRACK" => self.track(n, rest),
            "INDEX" => self.index(n, rest),
            "PREGAP" => self.gap(n, "PREGAP", rest),
            "POSTGAP" => self.gap(n, "POSTGAP", rest),
            "FLAGS" => self.flags(n, rest),
            "ISRC" => {
                if let Some(v) = self.value(n, "ISRC", rest)
                    && let Some(t) = self.current_track(n, "ISRC")
                {
                    let slot = &mut self.sheet.tracks[t].isrc;
                    set(&mut self.warnings, n, "ISRC", slot, v);
                }
            }
            c if CD_TEXT_KEYWORDS.contains(&c) => {
                if let Some(value) = self.value(n, "REM", rest) {
                    self.push_remark(n, command, value);
                }
            }
            _ => {
                let mut shown = command;
                shown.truncate(shown.floor_char_boundary(32));
                self.warnings.at(n, Kind::UnknownCommand(shown));
            }
        }
    }

    /// The value of a command: the argument without its quotes, or `None` after a
    /// warning when there is none.
    fn value(&mut self, n: usize, command: &'static str, rest: &str) -> Option<String> {
        let value = unquote(rest, n, &mut self.warnings);
        if value.is_empty() {
            self.warnings.at(n, Kind::MissingArgument(command));
            return None;
        }
        Some(value.to_owned())
    }

    /// The index of the current track, or `None` after a warning before the first
    /// `TRACK` or silently inside a skipped track.
    fn current_track(&mut self, n: usize, command: &'static str) -> Option<usize> {
        if !self.in_track {
            self.warnings.at(n, Kind::OutsideTrack(command));
            return None;
        }
        if self.skipping_track {
            return None;
        }
        self.sheet.tracks.len().checked_sub(1)
    }

    fn rem(&mut self, n: usize, rest: &str) {
        let (key, value) = match rest.split_once(char::is_whitespace) {
            Some((k, v)) => (k, v),
            None => (rest, ""),
        };
        // A free-form comment (`REM ripped by me`) has no key in upper case.
        let is_key = !key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
            && key.bytes().any(|b| b.is_ascii_uppercase());
        if !is_key {
            return;
        }
        let value = unquote(value, n, &mut self.warnings);
        if value.is_empty() {
            return;
        }
        self.push_remark(n, key.to_owned(), value.to_owned());
    }

    fn push_remark(&mut self, n: usize, key: String, value: String) {
        let remarks = if self.in_track {
            if self.skipping_track {
                return;
            }
            match self.sheet.tracks.last_mut() {
                Some(track) => &mut track.remarks,
                None => return,
            }
        } else {
            &mut self.sheet.remarks
        };
        if remarks.len() >= MAX_REMARKS {
            if !self.remarks_overflowed {
                self.remarks_overflowed = true;
                self.warnings.at(n, Kind::TooMany("remarks"));
            }
            return;
        }
        remarks.push(Remark { key, value });
    }

    fn text_field(&mut self, n: usize, command: &'static str, rest: &str) {
        let Some(value) = self.value(n, command, rest) else {
            return;
        };
        let slot = if self.in_track {
            if self.skipping_track {
                return;
            }
            let Some(track) = self.sheet.tracks.last_mut() else {
                return;
            };
            match command {
                "TITLE" => &mut track.title,
                "PERFORMER" => &mut track.performer,
                _ => &mut track.songwriter,
            }
        } else {
            match command {
                "TITLE" => &mut self.sheet.title,
                "PERFORMER" => &mut self.sheet.performer,
                _ => &mut self.sheet.songwriter,
            }
        };
        set(&mut self.warnings, n, command, slot, value);
    }

    fn file_line(&mut self, n: usize, rest: &str) {
        let (name, file_type) = split_file(rest, n, &mut self.warnings);
        if name.is_empty() {
            self.warnings.at(n, Kind::MissingArgument("FILE"));
            self.file = None;
            return;
        }
        if self.sheet.files.len() >= MAX_FILES {
            if self.sheet.files.len() == MAX_FILES {
                self.warnings.at(n, Kind::TooMany("files"));
            }
            self.file = None;
            return;
        }
        let raw_name = self.sheet.encoding.encode(name);
        self.sheet.files.push(CueFile {
            name: name.to_owned(),
            raw_name,
            file_type: file_type.map(str::to_ascii_uppercase),
            line: n,
        });
        self.file = Some(self.sheet.files.len() - 1);
    }

    fn track(&mut self, n: usize, rest: &str) {
        self.in_track = true;
        self.skipping_track = true;
        self.remarks_overflowed = false;
        let mut words = rest.split_whitespace();
        let Some(number) = words.next() else {
            self.warnings.at(n, Kind::MissingArgument("TRACK"));
            return;
        };
        let Some(number) = parse_number(number, MAX_TRACK_NUMBER) else {
            self.warnings.at(n, Kind::BadNumber(shorten(number)));
            return;
        };
        if self.sheet.tracks.len() >= MAX_TRACKS {
            if self.sheet.tracks.len() == MAX_TRACKS {
                self.warnings.at(n, Kind::TooMany("tracks"));
            }
            return;
        }
        if self.sheet.tracks.iter().any(|t| t.number == number) {
            self.warnings.at(n, Kind::DuplicateTrack(number));
        }
        let data_type = words.next().unwrap_or("AUDIO").to_ascii_uppercase();
        self.sheet.tracks.push(CueTrack {
            number,
            data_type: shorten(&data_type),
            file: self.file,
            title: None,
            performer: None,
            songwriter: None,
            isrc: None,
            flags: CueFlags::default(),
            pregap: None,
            postgap: None,
            indexes: Vec::new(),
            remarks: Vec::new(),
            line: n,
        });
        self.skipping_track = false;
    }

    fn index(&mut self, n: usize, rest: &str) {
        let mut words = rest.split_whitespace();
        let (Some(number), Some(time)) = (words.next(), words.next()) else {
            self.warnings.at(n, Kind::MissingArgument("INDEX"));
            return;
        };
        let Some(number) = parse_number(number, MAX_INDEX_NUMBER) else {
            self.warnings.at(n, Kind::BadNumber(shorten(number)));
            return;
        };
        let Some(time) = CueTime::parse(time) else {
            self.warnings.at(n, Kind::BadTime(shorten(time)));
            return;
        };
        let file = self.file;
        let Some(t) = self.current_track(n, "INDEX") else {
            return;
        };
        let track = &mut self.sheet.tracks[t];
        if track.index(number).is_some() {
            self.warnings.at(n, Kind::DuplicateIndex(number));
            return;
        }
        if track.indexes.len() >= MAX_INDEXES {
            self.warnings.at(n, Kind::TooMany("indexes"));
            return;
        }
        track.indexes.push(CueIndex {
            number,
            time,
            file,
            line: n,
        });
    }

    fn gap(&mut self, n: usize, command: &'static str, rest: &str) {
        let Some(time) = rest.split_whitespace().next() else {
            self.warnings.at(n, Kind::MissingArgument(command));
            return;
        };
        let Some(time) = CueTime::parse(time) else {
            self.warnings.at(n, Kind::BadTime(shorten(time)));
            return;
        };
        let Some(t) = self.current_track(n, command) else {
            return;
        };
        let track = &mut self.sheet.tracks[t];
        let slot = if command == "PREGAP" {
            &mut track.pregap
        } else {
            &mut track.postgap
        };
        set(&mut self.warnings, n, command, slot, time);
    }

    fn flags(&mut self, n: usize, rest: &str) {
        let Some(t) = self.current_track(n, "FLAGS") else {
            return;
        };
        let track = &mut self.sheet.tracks[t];
        for flag in rest.split_whitespace() {
            match flag.to_ascii_uppercase().as_str() {
                "DCP" => track.flags.digital_copy_permitted = true,
                "4CH" => track.flags.four_channel = true,
                "PRE" => track.flags.pre_emphasis = true,
                "SCMS" => track.flags.serial_copy_management = true,
                // `DATA` and unknown flags carry nothing for playback.
                _ => {}
            }
        }
    }
}

/// Sets a single-valued field, warning when it was already set.
fn set<T>(
    warnings: &mut Warnings,
    n: usize,
    command: &'static str,
    slot: &mut Option<T>,
    value: T,
) {
    if slot.is_some() {
        warnings.at(n, Kind::Repeated(command));
    }
    *slot = Some(value);
}

/// The value of an argument: without its surrounding quotes, or the whole rest of
/// the line when it is not quoted.
fn unquote<'a>(rest: &'a str, n: usize, warnings: &mut Warnings) -> &'a str {
    let rest = rest.trim();
    match rest.strip_prefix('"') {
        Some(inner) => match inner.strip_suffix('"') {
            Some(inner) => inner.trim(),
            None => {
                warnings.at(n, Kind::UnterminatedQuote);
                inner.trim()
            }
        },
        None => rest,
    }
}

/// The name and type of a `FILE` line: `"name with spaces.wav" WAVE`, `name.wav
/// WAVE`, or a name alone.
fn split_file<'a>(rest: &'a str, n: usize, warnings: &mut Warnings) -> (&'a str, Option<&'a str>) {
    let rest = rest.trim();
    if let Some(inner) = rest.strip_prefix('"') {
        return match inner.rfind('"') {
            Some(close) => {
                let file_type = inner[close + 1..].split_whitespace().next();
                (inner[..close].trim(), file_type)
            }
            None => {
                warnings.at(n, Kind::UnterminatedQuote);
                (inner.trim(), None)
            }
        };
    }
    match rest.rsplit_once(char::is_whitespace) {
        Some((name, file_type))
            if !file_type.is_empty() && file_type.bytes().all(|b| b.is_ascii_alphanumeric()) =>
        {
            (name.trim(), Some(file_type))
        }
        _ => (rest, None),
    }
}

/// A decimal number of at most four digits, no higher than `max`.
fn parse_number(text: &str, max: u32) -> Option<u32> {
    if text.is_empty() || text.len() > 4 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|&v| v <= max)
}

/// `text` cut to a length fit for a warning.
fn shorten(text: &str) -> String {
    text[..text.floor_char_boundary(32)].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cue::text::decode;

    fn parse(text: &str) -> CueSheet {
        let (text, encoding) = decode(text.as_bytes(), None);
        parse_text(&text, encoding)
    }

    #[test]
    fn line_endings() {
        let all: Vec<&str> = lines("a\r\nb\nc\rd\r\n\r\ne").collect();
        assert_eq!(all, ["a", "b", "c", "d", "", "e"]);
        assert_eq!(lines("").count(), 0);
        assert_eq!(lines("x\n").collect::<Vec<_>>(), ["x"]);
    }

    #[test]
    fn values_and_quotes() {
        let sheet = parse(
            "title \"Say \"Hi\"\"\nPERFORMER Unquoted Name\nSONGWRITER \"Open\nTRACK 1 audio\nTITLE  \"  spaced  \"  \n",
        );
        assert_eq!(sheet.title.as_deref(), Some("Say \"Hi\""));
        assert_eq!(sheet.performer.as_deref(), Some("Unquoted Name"));
        assert_eq!(sheet.songwriter.as_deref(), Some("Open"));
        assert_eq!(sheet.tracks[0].title.as_deref(), Some("spaced"));
        assert_eq!(sheet.tracks[0].data_type, "AUDIO");
        assert_eq!(sheet.warnings.len(), 1);
        assert_eq!(sheet.warnings[0].line, Some(3));
        assert_eq!(sheet.warnings[0].kind, Kind::UnterminatedQuote);
    }

    #[test]
    fn file_lines() {
        let sheet = parse(
            "FILE \"My Album (Disc 1).flac\" WAVE\nFILE plain.mp3 MP3\nFILE \"no type.wav\"\nFILE spaced name.ape wave\nFILE \"x\"y.wav\" WAVE\n",
        );
        let names: Vec<(&str, Option<&str>)> = sheet
            .files
            .iter()
            .map(|f| (f.name.as_str(), f.file_type.as_deref()))
            .collect();
        assert_eq!(
            names,
            [
                ("My Album (Disc 1).flac", Some("WAVE")),
                ("plain.mp3", Some("MP3")),
                ("no type.wav", None),
                ("spaced name.ape", Some("WAVE")),
                ("x\"y.wav", Some("WAVE")),
            ]
        );
    }

    #[test]
    fn remarks() {
        let sheet = parse(
            "REM GENRE \"Progressive Rock\"\nREM DATE 1973\nREM REPLAYGAIN_ALBUM_GAIN -7.89 dB\nREM this is a note\nREM\nCOMPOSER \"Someone\"\nTRACK 01 AUDIO\nREM REPLAYGAIN_TRACK_GAIN -6.10 dB\n",
        );
        let keys: Vec<(&str, &str)> = sheet
            .remarks
            .iter()
            .map(|r| (r.key.as_str(), r.value.as_str()))
            .collect();
        assert_eq!(
            keys,
            [
                ("GENRE", "Progressive Rock"),
                ("DATE", "1973"),
                ("REPLAYGAIN_ALBUM_GAIN", "-7.89 dB"),
                ("COMPOSER", "Someone"),
            ]
        );
        assert_eq!(
            sheet.tracks[0].remark("REPLAYGAIN_TRACK_GAIN"),
            Some("-6.10 dB")
        );
        assert!(sheet.warnings.is_empty(), "{:?}", sheet.warnings);
    }

    #[test]
    fn malformed_lines_are_reported_with_their_line() {
        let sheet = parse(
            "INDEX 01 00:00:00\nFILE \"a.wav\" WAVE\nTRACK xx AUDIO\nINDEX 01 00:00:00\nTRACK 01 AUDIO\nINDEX 01 00:61:00\nINDEX 1\nINDEX 100 00:00:00\nINDEX 01 00:00:00\nINDEX 01 00:01:00\nFROB 1\nPREGAP 00:02:00\nPREGAP 00:03:00\nTRACK 01 AUDIO\nTITLE\n",
        );
        let found: Vec<(Option<usize>, Kind)> = sheet
            .warnings
            .iter()
            .map(|w| (w.line, w.kind.clone()))
            .collect();
        assert_eq!(
            found,
            [
                (Some(1), Kind::OutsideTrack("INDEX")),
                (Some(3), Kind::BadNumber("xx".into())),
                (Some(6), Kind::BadTime("00:61:00".into())),
                (Some(7), Kind::MissingArgument("INDEX")),
                (Some(8), Kind::BadNumber("100".into())),
                (Some(10), Kind::DuplicateIndex(1)),
                (Some(11), Kind::UnknownCommand("FROB".into())),
                (Some(13), Kind::Repeated("PREGAP")),
                (Some(14), Kind::DuplicateTrack(1)),
                (Some(15), Kind::MissingArgument("TITLE")),
            ]
        );
        assert_eq!(sheet.tracks.len(), 2);
        assert_eq!(sheet.tracks[0].indexes.len(), 1);
        assert_eq!(sheet.tracks[0].pregap, CueTime::parse("00:03:00"));
    }

    #[test]
    fn commands_of_a_skipped_track_are_dropped() {
        let sheet = parse(
            "FILE a.wav WAVE\nTRACK 01 AUDIO\nTITLE One\nTRACK zz AUDIO\nTITLE Bad\nINDEX 01 00:00:00\n",
        );
        assert_eq!(sheet.tracks.len(), 1);
        assert_eq!(sheet.tracks[0].title.as_deref(), Some("One"));
        assert!(sheet.tracks[0].indexes.is_empty());
    }

    #[test]
    fn limits_hold() {
        let mut text = String::from("FILE a.wav WAVE\n");
        for i in 0..1200 {
            text.push_str(&format!("TRACK {i} AUDIO\nINDEX 01 00:00:00\n"));
        }
        text.push_str(&"x".repeat(MAX_LINE_BYTES + 1));
        text.push('\n');
        for _ in 0..400 {
            text.push_str("REM KEY value\n");
        }
        let sheet = parse(&text);
        assert_eq!(sheet.tracks.len(), MAX_TRACKS);
        assert!(
            sheet
                .warnings
                .iter()
                .any(|w| w.kind == Kind::TooMany("tracks"))
        );
        assert!(sheet.warnings.iter().any(|w| w.kind == Kind::LineTooLong));
    }

    #[test]
    fn flags_and_isrc() {
        let sheet = parse(
            "FILE a.wav WAVE\nTRACK 01 AUDIO\nFLAGS DCP PRE\nISRC USRC17607839\nINDEX 01 00:00:00\nTRACK 02 MODE1/2352\n",
        );
        let t = &sheet.tracks[0];
        assert!(t.flags.pre_emphasis && t.flags.digital_copy_permitted);
        assert!(!t.flags.four_channel);
        assert_eq!(t.isrc.as_deref(), Some("USRC17607839"));
        assert!(!sheet.tracks[1].is_audio());
    }
}
