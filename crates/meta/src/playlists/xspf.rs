// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! XSPF, the XML Shareable Playlist Format.
//!
//! Reading, with `quick-xml`: `playlist/title`, and for each `playlist/trackList/track`
//! its `location` (the first local one when there are several), `title`, `creator`,
//! `album`, `duration` (milliseconds) and `trackNum`. Element names are matched without
//! their namespace prefix and in any case. Locations are URI references: `file:` URLs
//! and relative references are percent-decoded. Only the predefined entities and
//! character references are expanded; a document type's entities are never expanded,
//! so entity expansion cannot blow up. Malformed XML ends the reading with a warning
//! and keeps the tracks read before it. Nesting is limited to 64 levels.
//!
//! Writing: UTF-8, `file:` URIs (or relative references) that percent-encode every byte
//! outside the unreserved set, so names that are not UTF-8 survive.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::time::Duration;

use quick_xml::Reader;
use quick_xml::escape::{escape, resolve_predefined_entity};
use quick_xml::events::Event;

#[cfg(test)]
use super::entry::EntryLocation;
use super::entry::{EntryHints, EntryReader, Playlist};
use super::location::{Syntax, uri_for_writing};
use super::text::{self, Charset};
use super::warning::{PlaylistWarningKind, Warnings};
use super::{
    ParseOptions, ParsedPlaylist, PlaylistError, PlaylistFormat, PlaylistKind, WriteOptions,
    WrittenPlaylist, check_input,
};

/// XSPF.
#[derive(Debug, Clone, Copy, Default)]
pub struct Xspf;

impl PlaylistFormat for Xspf {
    fn kind(&self) -> PlaylistKind {
        PlaylistKind::Xspf
    }

    fn parse(&self, bytes: &[u8], options: &ParseOptions) -> Result<ParsedPlaylist, PlaylistError> {
        parse(bytes, options)
    }

    fn write(&self, playlist: &Playlist, options: &WriteOptions) -> WrittenPlaylist {
        write(playlist, options)
    }
}

/// The deepest nesting followed.
const MAX_DEPTH: usize = 64;

/// The XSPF namespace.
const NAMESPACE: &str = "http://xspf.org/ns/0/";

/// The elements whose text is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    PlaylistTitle,
    Location,
    Title,
    Creator,
    Album,
    Duration,
    TrackNum,
}

/// A track being read.
#[derive(Debug, Default)]
struct Track {
    line: usize,
    locations: Vec<(usize, String)>,
    hints: EntryHints,
}

/// Counts lines up to a byte position, moving forward only.
#[derive(Debug)]
struct LineCounter<'a> {
    text: &'a str,
    position: usize,
    line: usize,
}

impl LineCounter<'_> {
    fn at(&mut self, position: usize) -> usize {
        let position = position.min(self.text.len());
        if position > self.position {
            self.line += self.text.as_bytes()[self.position..position]
                .iter()
                .filter(|&&b| b == b'\n')
                .count();
            self.position = position;
        }
        self.line
    }
}

fn parse(bytes: &[u8], options: &ParseOptions) -> Result<ParsedPlaylist, PlaylistError> {
    check_input(bytes, options)?;
    let (document, encoding) = text::decode_document(bytes, Charset::Utf8, None);
    let mut warnings = Warnings::default();
    if encoding.had_errors {
        warnings.push(
            None,
            PlaylistWarningKind::UndecodableText {
                encoding: encoding.name(),
            },
        );
    }
    let mut reader = Reader::from_str(&document);
    {
        let config = reader.config_mut();
        config.check_end_names = false;
        config.allow_unmatched_ends = true;
        config.expand_empty_elements = false;
    }
    let mut lines = LineCounter {
        text: &document,
        position: 0,
        line: 1,
    };
    let max_text = options.limits.max_line_bytes;
    let mut entries = EntryReader::new(
        options.base_dir.as_deref(),
        Syntax::Uri,
        encoding,
        options.limits.max_entries,
    );
    let mut title = None;
    // The local names of the open elements.
    let mut stack: Vec<String> = Vec::new();
    let mut too_deep = 0usize;
    let mut track: Option<Track> = None;
    let mut field: Option<(Field, String, bool)> = None;
    let mut seen_root = false;
    loop {
        let position = usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX);
        let event = match reader.read_event() {
            Ok(event) => event,
            Err(error) => {
                let line = lines.at(position);
                let message: String = error.to_string().chars().take(200).collect();
                warnings.at(line, PlaylistWarningKind::MalformedXml(message));
                break;
            }
        };
        match event {
            Event::Start(start) => {
                if too_deep > 0 || stack.len() >= MAX_DEPTH {
                    if too_deep == 0 {
                        warnings.at(lines.at(position), PlaylistWarningKind::TooDeep);
                    }
                    too_deep += 1;
                    continue;
                }
                let name = start.local_name().as_ref().to_owned();
                if stack.is_empty() {
                    seen_root = true;
                    if !name.eq_ignore_ascii_case("playlist") {
                        warnings.at(lines.at(position), PlaylistWarningKind::NotXspf);
                        break;
                    }
                }
                stack.push(name);
                if path_is(&stack, &["playlist", "trackList", "track"]) {
                    track = Some(Track {
                        line: lines.at(position),
                        ..Track::default()
                    });
                } else if let Some(f) = field_of(&stack, track.is_some()) {
                    field = Some((f, String::new(), false));
                }
            }
            Event::Empty(start) => {
                if stack.is_empty() {
                    seen_root = true;
                    let name = start.local_name();
                    if !name.as_ref().eq_ignore_ascii_case("playlist") {
                        warnings.at(lines.at(position), PlaylistWarningKind::NotXspf);
                    }
                    break;
                }
            }
            Event::End(_) => {
                if too_deep > 0 {
                    too_deep -= 1;
                    continue;
                }
                if let Some((f, text, too_long)) = field.take() {
                    if too_long {
                        warnings.at(lines.at(position), PlaylistWarningKind::LineTooLong);
                    } else {
                        store(
                            f,
                            text,
                            lines.at(position),
                            &mut title,
                            track.as_mut(),
                            &mut warnings,
                        );
                    }
                }
                if path_is(&stack, &["playlist", "trackList", "track"])
                    && let Some(done) = track.take()
                {
                    finish(done, &mut entries, &mut warnings);
                }
                stack.pop();
                if stack.is_empty() {
                    break;
                }
            }
            Event::Text(text) => {
                if too_deep == 0 {
                    append(&mut field, &text.xml10_content(), max_text);
                }
            }
            Event::CData(data) => {
                if too_deep == 0 {
                    append(&mut field, &data.xml10_content(), max_text);
                }
            }
            Event::GeneralRef(reference) => {
                if too_deep > 0 {
                    continue;
                }
                let resolved = match reference.resolve_char_ref() {
                    Ok(Some(c)) => Some(Cow::Owned(c.to_string())),
                    _ => resolve_predefined_entity(&reference.xml10_content()).map(Cow::Borrowed),
                };
                if let Some(text) = resolved {
                    append(&mut field, &text, max_text);
                }
            }
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
    if !seen_root {
        warnings.push(None, PlaylistWarningKind::NotXspf);
    }
    Ok(ParsedPlaylist {
        playlist: Playlist {
            title,
            entries: entries.into_entries(),
        },
        kind: PlaylistKind::Xspf,
        encoding,
        warnings: warnings.into_vec(),
    })
}

/// Whether the open elements are exactly `path`, ignoring case.
fn path_is(stack: &[String], path: &[&str]) -> bool {
    stack.len() == path.len()
        && stack
            .iter()
            .zip(path)
            .all(|(open, want)| open.eq_ignore_ascii_case(want))
}

/// The field the innermost open element is, if its text is read.
fn field_of(stack: &[String], in_track: bool) -> Option<Field> {
    if in_track && stack.len() == 4 && path_is(&stack[..3], &["playlist", "trackList", "track"]) {
        let name = stack[3].as_str();
        let fields = [
            ("location", Field::Location),
            ("title", Field::Title),
            ("creator", Field::Creator),
            ("album", Field::Album),
            ("duration", Field::Duration),
            ("trackNum", Field::TrackNum),
        ];
        return fields
            .iter()
            .find(|(n, _)| name.eq_ignore_ascii_case(n))
            .map(|&(_, f)| f);
    }
    path_is(stack, &["playlist", "title"]).then_some(Field::PlaylistTitle)
}

/// Adds `more` to the text of the open field, unless it grows past `max`.
fn append(field: &mut Option<(Field, String, bool)>, more: &str, max: usize) {
    if let Some((_, text, too_long)) = field {
        if text.len() + more.len() > max {
            *too_long = true;
        } else if !*too_long {
            text.push_str(more);
        }
    }
}

/// Stores the finished field `f` with its `text`.
fn store(
    f: Field,
    text: String,
    line: usize,
    title: &mut Option<String>,
    track: Option<&mut Track>,
    warnings: &mut Warnings,
) {
    let value = text.trim();
    if value.is_empty() {
        return;
    }
    if f == Field::PlaylistTitle {
        title.get_or_insert_with(|| value.to_owned());
        return;
    }
    let Some(track) = track else {
        return;
    };
    let hints = &mut track.hints;
    match f {
        Field::PlaylistTitle => {}
        Field::Location => track.locations.push((line, value.to_owned())),
        Field::Title => hints.title = Some(value.to_owned()),
        Field::Creator => hints.artist = Some(value.to_owned()),
        Field::Album => hints.album = Some(value.to_owned()),
        Field::Duration => match value.parse::<u64>() {
            Ok(0) => {}
            Ok(ms) => hints.duration = Some(Duration::from_millis(ms)),
            Err(_) => warnings.at(line, PlaylistWarningKind::BadDuration(short(value))),
        },
        Field::TrackNum => match value.parse::<u32>() {
            Ok(n) => hints.track_number = Some(n),
            Err(_) => warnings.at(line, PlaylistWarningKind::BadNumber(short(value))),
        },
    }
}

fn short(text: &str) -> String {
    text.chars().take(80).collect()
}

/// Adds the finished `track` as an entry: its first local location, or else its first.
fn finish(track: Track, entries: &mut EntryReader<'_>, warnings: &mut Warnings) {
    let Track {
        line,
        locations,
        hints,
    } = track;
    let chosen = locations
        .iter()
        .find(|(_, location)| {
            let lower = location.trim_start().to_ascii_lowercase();
            lower.starts_with("file:") || !lower.contains("://")
        })
        .or_else(|| locations.first());
    match chosen {
        Some((line, location)) => {
            entries.push(location.as_bytes(), location, *line, hints, warnings);
        }
        None => warnings.at(line, PlaylistWarningKind::MissingLocation),
    }
}

fn write(playlist: &Playlist, options: &WriteOptions) -> WrittenPlaylist {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(out, "<playlist version=\"1\" xmlns=\"{NAMESPACE}\">");
    if let Some(title) = &playlist.title {
        let _ = writeln!(out, "  <title>{}</title>", xml_text(title));
    }
    out.push_str("  <trackList>\n");
    for entry in &playlist.entries {
        out.push_str("    <track>\n");
        let location = uri_for_writing(&entry.location, options);
        let _ = writeln!(out, "      <location>{}</location>", xml_text(&location));
        let hints = &entry.hints;
        for (name, value) in [
            ("title", &hints.title),
            ("creator", &hints.artist),
            ("album", &hints.album),
        ] {
            if let Some(value) = value {
                let _ = writeln!(out, "      <{name}>{}</{name}>", xml_text(value));
            }
        }
        if let Some(number) = hints.track_number {
            let _ = writeln!(out, "      <trackNum>{number}</trackNum>");
        }
        if let Some(duration) = hints.duration {
            let _ = writeln!(out, "      <duration>{}</duration>", duration.as_millis());
        }
        out.push_str("    </track>\n");
    }
    out.push_str("  </trackList>\n</playlist>\n");
    WrittenPlaylist {
        bytes: out.into_bytes(),
        warnings: Vec::new(),
    }
}

/// `text` escaped for XML, without the control characters XML 1.0 cannot hold.
fn xml_text(text: &str) -> String {
    let clean: String = text
        .chars()
        .filter(|&c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
        .filter(|&c| c != '\u{fffe}' && c != '\u{ffff}')
        .collect();
    escape(clean.as_str()).into_owned()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

    use super::*;
    use crate::playlists::entry::PlaylistEntry;
    use crate::playlists::{PathStyle, PlaylistWarning};

    fn options() -> ParseOptions {
        ParseOptions {
            base_dir: Some("/music/lists".into()),
            ..ParseOptions::default()
        }
    }

    #[test]
    fn a_typical_document() {
        let file = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- made by hand -->
<playlist version="1" xmlns="http://xspf.org/ns/0/">
  <title>Rock &amp; roll</title>
  <trackList>
    <track>
      <location>http://example.org/a.mp3</location>
      <location>file:///music/A/01%20One.flac</location>
      <title>One &#x26; only</title>
      <creator>Band</creator>
      <album><![CDATA[Album <1>]]></album>
      <duration>201000</duration>
      <trackNum>1</trackNum>
      <extension application="x"><location>ignored</location></extension>
    </track>
    <track><location>../B/02%20Two.mp3</location></track>
    <track><title>no location</title></track>
    <track><location>https://example.org/stream</location></track>
  </trackList>
</playlist>"#;
        let parsed = Xspf.parse(file.as_bytes(), &options()).unwrap();
        assert_eq!(parsed.playlist.title.as_deref(), Some("Rock & roll"));
        let entries = &parsed.playlist.entries;
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0].location,
            EntryLocation::Local("/music/A/01 One.flac".into())
        );
        assert_eq!(entries[0].hints.title.as_deref(), Some("One & only"));
        assert_eq!(entries[0].hints.artist.as_deref(), Some("Band"));
        assert_eq!(entries[0].hints.album.as_deref(), Some("Album <1>"));
        assert_eq!(entries[0].hints.duration, Some(Duration::from_secs(201)));
        assert_eq!(entries[0].hints.track_number, Some(1));
        assert_eq!(entries[0].line, Some(8));
        assert_eq!(
            entries[1].location,
            EntryLocation::Local("/music/B/02 Two.mp3".into())
        );
        assert!(matches!(entries[2].location, EntryLocation::Remote(_)));
        let kinds: Vec<_> = parsed
            .warnings
            .iter()
            .map(|w| (w.line, w.kind.clone()))
            .collect();
        assert_eq!(kinds[0], (Some(17), PlaylistWarningKind::MissingLocation));
        assert!(matches!(kinds[1].1, PlaylistWarningKind::RemoteEntry(_)));
    }

    #[test]
    fn malformed_xml_keeps_what_came_before() {
        let file = b"<playlist><trackList><track><location>a.mp3</location></track>\n<track><location>b.mp3</loc";
        let parsed = Xspf.parse(file, &options()).unwrap();
        assert_eq!(parsed.playlist.entries.len(), 1);
        assert!(matches!(
            parsed.warnings[0],
            PlaylistWarning {
                line: Some(2),
                kind: PlaylistWarningKind::MalformedXml(_)
            }
        ));
        let file = b"<playlist><trackList><track><location>a.mp3</location></track><!-- open";
        let parsed = Xspf.parse(file, &options()).unwrap();
        assert_eq!(parsed.playlist.entries.len(), 1);
        assert!(matches!(
            parsed.warnings[0].kind,
            PlaylistWarningKind::MalformedXml(_)
        ));
    }

    #[test]
    fn entities_are_not_expanded() {
        let file = br#"<?xml version="1.0"?>
<!DOCTYPE playlist [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;">]>
<playlist><trackList><track><location>x&b;.mp3</location></track></trackList></playlist>"#;
        let parsed = Xspf.parse(file, &options()).unwrap();
        assert_eq!(
            parsed.playlist.entries[0].location,
            EntryLocation::Local("/music/lists/x.mp3".into())
        );
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let mut file = String::from("<playlist><trackList><track><location>a.mp3</location>");
        file.push_str(&"<x>".repeat(10_000));
        file.push_str(&"</x>".repeat(10_000));
        file.push_str("</track></trackList></playlist>");
        let parsed = Xspf.parse(file.as_bytes(), &options()).unwrap();
        assert_eq!(parsed.playlist.entries.len(), 1);
        assert!(
            parsed
                .warnings
                .iter()
                .any(|w| w.kind == PlaylistWarningKind::TooDeep)
        );
    }

    #[test]
    fn other_documents_are_not_playlists() {
        let parsed = Xspf.parse(b"<html><body/></html>", &options()).unwrap();
        assert!(parsed.playlist.entries.is_empty());
        assert_eq!(parsed.warnings[0].kind, PlaylistWarningKind::NotXspf);
        let parsed = Xspf.parse(b"", &options()).unwrap();
        assert_eq!(parsed.warnings[0].kind, PlaylistWarningKind::NotXspf);
    }

    #[test]
    fn writing_round_trips() {
        let latin1 = PathBuf::from(OsStr::from_bytes(b"/music/Bj\xf6rk/J\xf3ga & <1>.flac"));
        let mut first = PlaylistEntry::local(latin1.clone());
        first.hints = EntryHints {
            title: Some("Jóga & <1>\u{1}".into()),
            artist: Some("Björk".into()),
            album: Some("Homogenic".into()),
            duration: Some(Duration::from_millis(305_123)),
            track_number: Some(3),
        };
        let playlist = Playlist {
            title: Some("Iceland".into()),
            entries: vec![
                first,
                PlaylistEntry::local("/music/lists/near.mp3"),
                PlaylistEntry::new(EntryLocation::Remote("http://x/?a=1&b=2".into())),
            ],
        };
        for style in [PathStyle::Relative, PathStyle::Absolute] {
            let written = Xspf.write(
                &playlist,
                &WriteOptions {
                    path_style: style,
                    base_dir: Some("/music/lists".into()),
                },
            );
            let text = String::from_utf8(written.bytes.clone()).unwrap();
            assert!(text.contains("<duration>305123</duration>"));
            let back = Xspf.parse(&written.bytes, &options()).unwrap();
            assert!(
                back.warnings
                    .iter()
                    .all(|w| matches!(w.kind, PlaylistWarningKind::RemoteEntry(_)))
            );
            assert_eq!(back.playlist.title.as_deref(), Some("Iceland"));
            let entries = &back.playlist.entries;
            assert_eq!(entries[0].location, EntryLocation::Local(latin1.clone()));
            assert_eq!(entries[0].hints.title.as_deref(), Some("Jóga & <1>"));
            assert_eq!(
                entries[0].hints.duration,
                Some(Duration::from_millis(305_123))
            );
            assert_eq!(entries[0].hints.track_number, Some(3));
            assert_eq!(entries[1].location, playlist.entries[1].location);
            assert_eq!(entries[2].location, playlist.entries[2].location);
        }
    }
}
