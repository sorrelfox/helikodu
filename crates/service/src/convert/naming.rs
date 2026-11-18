// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Naming patterns: where a converted or copied file goes, from its tags.
//!
//! - `{field}` is replaced by the field's value; the names are those of the query
//!   language (`{albumartist}`, `{album}`, `{track}`, `{title}`, `{year}`, …,
//!   and `{tag:NAME}` for any other tag).
//! - `/` starts a folder. A folder whose name comes out empty is left out.
//! - Text in square brackets is left out when a field inside it is empty:
//!   `[{disc}-]{track}` gives `01` without a disc number.
//! - Track and disc numbers have two digits. On an album of several discs,
//!   `{track}` alone is written `1-01`, so the tracks of the discs do not mix.
//! - Characters that a folder or file name cannot hold are replaced, by the rules
//!   of a [`NameStyle`].
//!
//! The module stands alone: [`NamingPattern::parse`] reads a pattern and reports
//! where it cannot be read, [`NamingPattern::render`] gives the relative path for a
//! track's [`FieldValues`] (without the extension).

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use helikodu_model::query::Field;

/// The longest name, in bytes, a component is shortened to (most file systems
/// take 255 bytes; room is left for an extension).
const MAX_COMPONENT_BYTES: usize = 200;

/// Which characters names may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NameStyle {
    /// Names Windows, phones and FAT memory sticks can read: no `\ : * ? " < > |`,
    /// no control characters, no trailing dots or spaces.
    #[default]
    Portable,
    /// Names as they are; only `/` and NUL, which no name can hold, are replaced.
    AsIs,
    /// Plain ASCII: letters with accents lose them, other characters become `_`.
    Ascii,
}

impl NameStyle {
    /// A stable identifier, for settings files.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Portable => "portable",
            Self::AsIs => "as-is",
            Self::Ascii => "ascii",
        }
    }

    /// The style with the identifier `id`.
    pub fn from_id(id: &str) -> Option<Self> {
        [Self::Portable, Self::AsIs, Self::Ascii]
            .into_iter()
            .find(|s| s.id() == id)
    }
}

/// The values of one track's fields, as a pattern asks for them.
pub trait FieldValues {
    /// The value of `field`, or `None` when it is empty. Numbers as plain
    /// numbers: the pattern pads them.
    fn value(&self, field: &Field) -> Option<String>;

    /// The number of discs of the track's album, when known.
    fn disc_total(&self) -> Option<u32> {
        None
    }
}

/// [`FieldValues`] from a map, for tests and simple callers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldMap {
    /// The values.
    pub values: HashMap<Field, String>,
    /// The number of discs of the album.
    pub disc_total: Option<u32>,
}

impl FieldMap {
    /// Sets `field` to `value`; an empty value is left out.
    pub fn with(mut self, field: Field, value: impl Into<String>) -> Self {
        let value = value.into();
        if !value.trim().is_empty() {
            self.values.insert(field, value);
        }
        self
    }
}

impl FieldValues for FieldMap {
    fn value(&self, field: &Field) -> Option<String> {
        self.values.get(field).cloned()
    }

    fn disc_total(&self) -> Option<u32> {
        self.disc_total
    }
}

/// One piece of a pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    Field(Field),
    Folder,
    /// Square brackets: left out when a field inside is empty.
    Optional(Vec<Piece>),
}

/// Why a pattern cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PatternErrorKind {
    /// The pattern is empty.
    Empty,
    /// `{name}` names no field.
    UnknownField(String),
    /// `{` without `}`.
    UnclosedField,
    /// `[` without `]`.
    UnclosedBracket,
    /// `}` or `]` without its opening.
    Unopened(char),
    /// `[` inside square brackets.
    NestedBracket,
    /// `/` inside square brackets.
    FolderInBracket,
    /// The pattern makes no file name: it ends with `/`.
    NoFileName,
}

/// Why a pattern cannot be read, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError {
    /// What is wrong.
    pub kind: PatternErrorKind,
    /// The character position (not byte) where the problem is.
    pub position: usize,
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            PatternErrorKind::Empty => write!(f, "the pattern is empty"),
            PatternErrorKind::UnknownField(name) => write!(f, "{{{name}}} is not a field"),
            PatternErrorKind::UnclosedField => write!(f, "a {{ is not closed"),
            PatternErrorKind::UnclosedBracket => write!(f, "a [ is not closed"),
            PatternErrorKind::Unopened(c) => write!(f, "{c} was not opened"),
            PatternErrorKind::NestedBracket => write!(f, "square brackets cannot be nested"),
            PatternErrorKind::FolderInBracket => write!(f, "/ cannot be inside square brackets"),
            PatternErrorKind::NoFileName => write!(f, "the pattern ends with a folder"),
        }?;
        write!(f, " (at {})", self.position + 1)
    }
}

impl std::error::Error for PatternError {}

/// A naming pattern, read and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamingPattern {
    text: String,
    pieces: Vec<Piece>,
}

impl NamingPattern {
    /// The default pattern: `{albumartist}/{album}/{track} {title}`.
    pub const DEFAULT: &'static str = "{albumartist}/{album}/{track} {title}";

    /// Reads `text`.
    pub fn parse(text: &str) -> Result<Self, PatternError> {
        let chars: Vec<char> = text.chars().collect();
        if chars.iter().all(|c| c.is_whitespace()) {
            return Err(PatternError {
                kind: PatternErrorKind::Empty,
                position: 0,
            });
        }
        let mut top: Vec<Piece> = Vec::new();
        let mut group: Option<(usize, Vec<Piece>)> = None;
        let mut text_run = String::new();
        let mut i = 0;
        let error = |kind, position| Err(PatternError { kind, position });
        while i < chars.len() {
            let c = chars[i];
            let in_group = group.is_some();
            let target = match group.as_mut() {
                Some((_, pieces)) => pieces,
                None => &mut top,
            };
            match c {
                '{' => {
                    flush(&mut text_run, target);
                    let Some(end) = chars[i + 1..].iter().position(|&c| c == '}') else {
                        return error(PatternErrorKind::UnclosedField, i);
                    };
                    let name: String = chars[i + 1..i + 1 + end].iter().collect();
                    let Some(field) = Field::from_name(name.trim()) else {
                        return error(PatternErrorKind::UnknownField(name), i);
                    };
                    target.push(Piece::Field(field));
                    i += end + 2;
                    continue;
                }
                '}' | ']' if c == '}' || !in_group => {
                    return error(PatternErrorKind::Unopened(c), i);
                }
                ']' => {
                    flush(&mut text_run, target);
                    if let Some((_, pieces)) = group.take() {
                        top.push(Piece::Optional(pieces));
                    }
                }
                '[' => {
                    if in_group {
                        return error(PatternErrorKind::NestedBracket, i);
                    }
                    flush(&mut text_run, target);
                    group = Some((i, Vec::new()));
                }
                '/' => {
                    if in_group {
                        return error(PatternErrorKind::FolderInBracket, i);
                    }
                    flush(&mut text_run, target);
                    target.push(Piece::Folder);
                }
                c => text_run.push(c),
            }
            i += 1;
        }
        if let Some((start, _)) = group {
            return error(PatternErrorKind::UnclosedBracket, start);
        }
        flush(&mut text_run, &mut top);
        if matches!(top.last(), Some(Piece::Folder)) {
            return error(PatternErrorKind::NoFileName, chars.len().saturating_sub(1));
        }
        Ok(Self {
            text: text.to_owned(),
            pieces: top,
        })
    }

    /// The pattern as it was written.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The fields the pattern uses, in order, each once.
    pub fn fields(&self) -> Vec<Field> {
        let mut out: Vec<Field> = Vec::new();
        let mut add = |field: &Field| {
            if !out.contains(field) {
                out.push(field.clone());
            }
        };
        for piece in &self.pieces {
            match piece {
                Piece::Field(f) => add(f),
                Piece::Optional(inner) => {
                    for p in inner {
                        if let Piece::Field(f) = p {
                            add(f);
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The relative path for `values`, without an extension. Empty folders are left
    /// out; an empty file name becomes `_`.
    pub fn render(&self, values: &dyn FieldValues, style: NameStyle) -> PathBuf {
        let mut components: Vec<String> = vec![String::new()];
        for piece in &self.pieces {
            match piece {
                Piece::Folder => components.push(String::new()),
                Piece::Text(t) => push(&mut components, t),
                Piece::Field(f) => {
                    if let Some(v) = field_text(f, values) {
                        push(&mut components, &v);
                    }
                }
                Piece::Optional(inner) => {
                    let mut text = String::new();
                    let mut complete = true;
                    for p in inner {
                        match p {
                            Piece::Text(t) => text.push_str(t),
                            Piece::Field(f) => match field_text(f, values) {
                                Some(v) => text.push_str(&v),
                                None => complete = false,
                            },
                            _ => {}
                        }
                    }
                    if complete {
                        push(&mut components, &text);
                    }
                }
            }
        }
        let count = components.len();
        let mut path = PathBuf::new();
        for (index, component) in components.into_iter().enumerate() {
            let clean = clean_name(&component, style);
            if clean.is_empty() {
                if index + 1 == count {
                    path.push("_");
                }
                continue;
            }
            path.push(clean);
        }
        path
    }
}

impl Default for NamingPattern {
    /// [`NamingPattern::DEFAULT`].
    fn default() -> Self {
        Self {
            text: Self::DEFAULT.to_owned(),
            pieces: vec![
                Piece::Field(Field::AlbumArtist),
                Piece::Folder,
                Piece::Field(Field::Album),
                Piece::Folder,
                Piece::Field(Field::Track),
                Piece::Text(" ".to_owned()),
                Piece::Field(Field::Title),
            ],
        }
    }
}

fn flush(run: &mut String, target: &mut Vec<Piece>) {
    if !run.is_empty() {
        target.push(Piece::Text(std::mem::take(run)));
    }
}

fn push(components: &mut [String], text: &str) {
    if let Some(last) = components.last_mut() {
        // A value never starts a folder of its own.
        last.push_str(&text.replace('/', "-"));
    }
}

/// The text of `field`: numbers padded; `{track}` with its disc on albums of
/// several discs.
fn field_text(field: &Field, values: &dyn FieldValues) -> Option<String> {
    let value = values.value(field).filter(|v| !v.trim().is_empty())?;
    let two_digits = |v: &str| match v.trim().parse::<u32>() {
        Ok(n) => format!("{n:02}"),
        Err(_) => v.trim().to_owned(),
    };
    match field {
        Field::Track => {
            let track = two_digits(&value);
            let several_discs = values.disc_total().is_some_and(|t| t > 1);
            match values.value(&Field::Disc).filter(|_| several_discs) {
                Some(disc) => Some(format!("{}-{track}", disc.trim())),
                None => Some(track),
            }
        }
        Field::Disc => Some(two_digits(&value)),
        _ => Some(value.trim().to_owned()),
    }
}

/// `name` as a folder or file name in `style`, trimmed and shortened.
pub fn clean_name(name: &str, style: NameStyle) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match style {
            NameStyle::AsIs => out.push(if c == '/' || c == '\0' { '_' } else { c }),
            NameStyle::Portable => out.push(match c {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                c if c.is_control() => '_',
                c => c,
            }),
            NameStyle::Ascii => match fold_ascii(c) {
                Some(s) => out.push_str(s),
                None if c.is_ascii() && !c.is_ascii_control() => out.push(match c {
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                    c => c,
                }),
                None => out.push('_'),
            },
        }
    }
    let mut out = out.trim().to_owned();
    if style != NameStyle::AsIs {
        while out.ends_with('.') || out.ends_with(' ') {
            out.pop();
        }
    }
    // `.` and `..` are not names.
    if out.chars().all(|c| c == '.') {
        out.clear();
    }
    if out.len() > MAX_COMPONENT_BYTES {
        let mut end = MAX_COMPONENT_BYTES;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out = out.trim_end().to_owned();
    }
    out
}

/// The plain ASCII form of a letter with an accent, and of a few others.
fn fold_ascii(c: char) -> Option<&'static str> {
    if c.is_ascii() {
        return None;
    }
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' => "a",
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ą' => "A",
        'æ' => "ae",
        'Æ' => "AE",
        'ç' | 'ć' | 'č' => "c",
        'Ç' | 'Ć' | 'Č' => "C",
        'ď' | 'đ' => "d",
        'Ď' | 'Đ' => "D",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => "e",
        'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ę' | 'Ě' => "E",
        'ì' | 'í' | 'î' | 'ï' | 'ī' => "i",
        'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' => "I",
        'ł' => "l",
        'Ł' => "L",
        'ñ' | 'ń' | 'ň' => "n",
        'Ñ' | 'Ń' | 'Ň' => "N",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' | 'Ő' => "O",
        'œ' => "oe",
        'Œ' => "OE",
        'ř' => "r",
        'Ř' => "R",
        'ś' | 'š' | 'ş' => "s",
        'Ś' | 'Š' | 'Ş' => "S",
        'ß' => "ss",
        'ť' | 'ţ' => "t",
        'Ť' | 'Ţ' => "T",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' | 'Ů' | 'Ű' => "U",
        'ý' | 'ÿ' => "y",
        'Ý' | 'Ÿ' => "Y",
        'ź' | 'ż' | 'ž' => "z",
        'Ź' | 'Ż' | 'Ž' => "Z",
        'þ' => "th",
        'Þ' => "TH",
        'ð' => "d",
        'Ð' => "D",
        '‘' | '’' | '´' => "'",
        '“' | '”' => "'",
        '–' | '—' => "-",
        '…' => "...",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tidewater() -> FieldMap {
        FieldMap::default()
            .with(Field::AlbumArtist, "Mari Kask")
            .with(Field::Album, "Tidewater Rooms")
            .with(Field::Track, "1")
            .with(Field::Title, "Tidewater")
    }

    #[test]
    fn the_default_pattern() {
        let p = NamingPattern::parse(NamingPattern::DEFAULT).unwrap();
        assert_eq!(p, NamingPattern::default());
        assert_eq!(
            p.render(&tidewater(), NameStyle::Portable),
            PathBuf::from("Mari Kask/Tidewater Rooms/01 Tidewater")
        );
        assert_eq!(
            p.fields(),
            [Field::AlbumArtist, Field::Album, Field::Track, Field::Title]
        );
    }

    #[test]
    fn several_discs_number_the_tracks_with_the_disc() {
        let p = NamingPattern::parse("{album}/{track} {title}").unwrap();
        let mut values = tidewater().with(Field::Disc, "1");
        values.disc_total = Some(2);
        assert_eq!(
            p.render(&values, NameStyle::Portable),
            PathBuf::from("Tidewater Rooms/1-01 Tidewater")
        );
    }

    #[test]
    fn brackets_leave_out_empty_parts() {
        let p = NamingPattern::parse("{album}[ ({year})]/[{disc}-]{track}").unwrap();
        assert_eq!(
            p.render(&tidewater(), NameStyle::Portable),
            PathBuf::from("Tidewater Rooms/01")
        );
        let with = tidewater().with(Field::Year, "2019").with(Field::Disc, "2");
        assert_eq!(
            p.render(&with, NameStyle::Portable),
            PathBuf::from("Tidewater Rooms (2019)/02-01")
        );
    }

    #[test]
    fn empty_folders_are_left_out_and_values_never_make_folders() {
        let p = NamingPattern::parse("{albumartist}/{album}/{title}").unwrap();
        let values = FieldMap::default()
            .with(Field::Album, "AC/DC Live")
            .with(Field::Title, "What?");
        assert_eq!(
            p.render(&values, NameStyle::Portable),
            PathBuf::from("AC-DC Live/What_")
        );
        assert_eq!(
            p.render(&FieldMap::default(), NameStyle::Portable),
            PathBuf::from("_")
        );
    }

    #[test]
    fn styles() {
        assert_eq!(
            clean_name("Sigur Rós: Ágætis byrjun.", NameStyle::Portable),
            "Sigur Rós_ Ágætis byrjun"
        );
        assert_eq!(
            clean_name("Sigur Rós: Ágætis byrjun", NameStyle::Ascii),
            "Sigur Ros_ Agaetis byrjun"
        );
        assert_eq!(clean_name("a:b.", NameStyle::AsIs), "a:b.");
        assert_eq!(clean_name("..", NameStyle::AsIs), "");
        assert_eq!(clean_name("東京", NameStyle::Ascii), "__");
    }

    #[test]
    fn errors_say_where() {
        let e = |t: &str| NamingPattern::parse(t).unwrap_err();
        assert_eq!(
            e("{album}/{nope}").kind,
            PatternErrorKind::UnknownField("nope".into())
        );
        assert_eq!(e("{album}/{nope}").position, 8);
        assert_eq!(e("{album").kind, PatternErrorKind::UnclosedField);
        assert_eq!(e("[{album}").kind, PatternErrorKind::UnclosedBracket);
        assert_eq!(e("a]").kind, PatternErrorKind::Unopened(']'));
        assert_eq!(e("[[a]]").kind, PatternErrorKind::NestedBracket);
        assert_eq!(e("[a/b]").kind, PatternErrorKind::FolderInBracket);
        assert_eq!(e("{album}/").kind, PatternErrorKind::NoFileName);
        assert_eq!(e("  ").kind, PatternErrorKind::Empty);
    }
}
