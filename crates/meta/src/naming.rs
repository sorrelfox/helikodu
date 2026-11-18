// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Naming patterns: how the file organizer, the converter and folder sync name
//! files and folders from their tags. All three read and write patterns here, so
//! one pattern gives one name for one track in every tool.
//!
//! The syntax:
//!
//! - `{field}` is the value of a field, named as the query language names it
//!   (`{albumartist}`, `{year}`, `{track}`, `{tag:MOOD}`);
//! - `/` starts a folder;
//! - text in square brackets is left out when a field inside it is empty
//!   (`[{disc}-]{track}`); brackets may nest, an inner bracket is left out on its
//!   own, and a bracket may hold a `/` (`[{disc}/]` makes a folder per disc only
//!   on albums of several discs);
//! - everything else is written as it is.
//!
//! Numbers: `{track}` has two digits (three on an album of a hundred tracks or
//! more); `{disc}` has a value only on an album of several discs, with as many
//! digits as the number of discs needs. `{track}` never holds the disc: write
//! `[{disc}-]{track}` for that.
//!
//! The file extension is never part of a pattern: the caller keeps the file's
//! own. A field that is empty outside square brackets is written as a word the
//! caller gives (such as "Unknown"), so that no name is ever empty.
//!
//! Names are then cleaned for the file system by a [`CharacterSet`]: a value can
//! never start a folder of its own (a `/` inside a value is replaced), the
//! characters that the chosen set does not allow are replaced too, and each name
//! is cut to [`MAX_NAME_BYTES`] bytes, the file name with room for its extension.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use helikodu_model::query::Field;

/// The longest name of one file or folder, in bytes, on the file systems of Linux.
pub const MAX_NAME_BYTES: usize = 255;

/// The character that stands in for one that a name may not hold.
const REPLACEMENT: char = '_';

/// Which characters names may hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CharacterSet {
    /// Names that Windows can read too (for drives shared with other systems):
    /// no `< > : " \ | ? *`, no control characters, no folder name ending in a dot
    /// or a space, no device names such as `CON`.
    #[default]
    Windows,
    /// Names as they are: only `/` and NUL, which no Linux name can hold, are
    /// replaced.
    AsIs,
    /// Plain ASCII only: accents are dropped (`é` becomes `e`), a few letters are
    /// spelled out (`ß` becomes `ss`), other characters are replaced; and the
    /// rules of [`Windows`](Self::Windows) apply too.
    Ascii,
}

impl CharacterSet {
    /// Every set, in the order of the choices.
    pub const ALL: [Self; 3] = [Self::Windows, Self::AsIs, Self::Ascii];

    /// A stable identifier, for settings files.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Windows => "portable",
            Self::AsIs => "as-is",
            Self::Ascii => "ascii",
        }
    }

    /// The set with the identifier `id`.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }
}

/// The values of one track's fields, as a pattern asks for them.
pub trait FieldValues {
    /// The value of `field`, or `None` (or an empty text) when it is empty.
    /// Numbers as plain numbers: the pattern pads them.
    fn value(&self, field: &Field) -> Option<String>;

    /// The number of tracks of the track's disc, when known: three digits for
    /// `{track}` from a hundred on.
    fn track_count(&self) -> Option<u32> {
        None
    }

    /// The number of discs of the track's album (or its highest disc number),
    /// when known: `{disc}` is empty unless it is more than one.
    fn disc_count(&self) -> Option<u32> {
        None
    }
}

impl<F: Fn(&Field) -> Option<String>> FieldValues for F {
    fn value(&self, field: &Field) -> Option<String> {
        self(field)
    }
}

/// [`FieldValues`] from a map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldMap {
    /// The values.
    pub values: HashMap<Field, String>,
    /// The number of tracks of the disc.
    pub track_count: Option<u32>,
    /// The number of discs of the album.
    pub disc_count: Option<u32>,
}

impl FieldMap {
    /// Sets `field` to `value`; an empty value is left out.
    #[must_use]
    pub fn with(mut self, field: Field, value: impl Into<String>) -> Self {
        self.set(field, value);
        self
    }

    /// Sets `field` to `value`; an empty value is left out.
    pub fn set(&mut self, field: Field, value: impl Into<String>) {
        let value = value.into();
        if !value.trim().is_empty() {
            self.values.insert(field, value);
        }
    }
}

impl FieldValues for FieldMap {
    fn value(&self, field: &Field) -> Option<String> {
        self.values.get(field).cloned()
    }

    fn track_count(&self) -> Option<u32> {
        self.track_count
    }

    fn disc_count(&self) -> Option<u32> {
        self.disc_count
    }
}

/// The number of digits of `n`.
fn digits(n: u32) -> usize {
    n.max(1).to_string().len()
}

/// The text of `field` for a name: empty as `None`, numbers padded, the disc
/// only on albums of several discs.
fn field_text(field: &Field, values: &dyn FieldValues) -> Option<String> {
    let value = values.value(field)?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    match field {
        Field::Track => Some(match value.parse::<u32>() {
            Ok(n) => {
                let width = values.track_count().map_or(2, |t| digits(t).max(2));
                format!("{n:0width$}")
            }
            Err(_) => value.to_owned(),
        }),
        Field::Disc => {
            let n = value.parse::<u32>().ok();
            let most = values.disc_count().unwrap_or(1).max(n.unwrap_or(1));
            if most <= 1 {
                return None;
            }
            Some(match n {
                Some(n) => {
                    let width = digits(most);
                    format!("{n:0width$}")
                }
                None => value.to_owned(),
            })
        }
        _ => Some(value.to_owned()),
    }
}

/// One piece of a pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    /// Text written as it is.
    Text(String),
    /// A field's value.
    Field(Field),
    /// A folder starts here.
    Folder,
    /// Parts left out when a field among them is empty.
    Optional(Vec<Part>),
}

/// A naming pattern that has been read. See the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamingPattern {
    text: String,
    parts: Vec<Part>,
}

/// Why a pattern cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternProblem {
    /// The pattern is empty.
    Empty,
    /// A `{` without its `}`.
    UnclosedField,
    /// `{}` without a name.
    EmptyField,
    /// A field name the query language does not know.
    UnknownField(String),
    /// A `[` without its `]`.
    UnclosedBracket,
    /// A `]` or `}` without its opening half.
    UnexpectedClose(char),
    /// A `{` inside a field name.
    NestedField,
    /// The pattern starts or ends with `/`, or has two in a row: a folder name
    /// would be empty.
    EmptyFolder,
}

/// A pattern that cannot be read: what is wrong and where, in characters from the
/// start of the pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError {
    /// What is wrong.
    pub problem: PatternProblem,
    /// The first character of the problem, counted from 0.
    pub start: usize,
    /// The character after the problem.
    pub end: usize,
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.problem {
            PatternProblem::Empty => write!(f, "the pattern is empty"),
            PatternProblem::UnclosedField => write!(f, "a field is not closed with }}"),
            PatternProblem::EmptyField => write!(f, "{{}} names no field"),
            PatternProblem::UnknownField(name) => write!(f, "there is no field {name}"),
            PatternProblem::UnclosedBracket => write!(f, "a [ is not closed with ]"),
            PatternProblem::UnexpectedClose(c) => write!(f, "{c} closes nothing"),
            PatternProblem::NestedField => write!(f, "a field name cannot hold {{"),
            PatternProblem::EmptyFolder => write!(f, "a folder name would be empty"),
        }?;
        write!(f, " (at character {})", self.start + 1)
    }
}

impl std::error::Error for PatternError {}

impl NamingPattern {
    /// The pattern the converter and folder sync start with.
    pub const DEFAULT: &'static str = "{albumartist}/{album}/[{disc}-]{track} {title}";

    /// Reads `text`.
    pub fn parse(text: &str) -> Result<Self, PatternError> {
        let chars: Vec<char> = text.chars().collect();
        if text.trim().is_empty() {
            return Err(PatternError {
                problem: PatternProblem::Empty,
                start: 0,
                end: chars.len(),
            });
        }
        let mut position = 0;
        let parts = parse_parts(&chars, &mut position, None)?;
        check_folders(&chars)?;
        Ok(Self {
            text: text.to_owned(),
            parts,
        })
    }

    /// The pattern as written.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The fields the pattern uses, in order, each once.
    pub fn fields(&self) -> Vec<Field> {
        let mut fields = Vec::new();
        collect_fields(&self.parts, &mut fields);
        fields
    }

    /// The folders and the file name (without its extension) for a track whose
    /// field values `values` gives, cleaned by `set`. An empty field outside
    /// brackets is written as `unknown`. The last name is kept short enough for
    /// `extension` (with its dot) to fit.
    pub fn render(
        &self,
        values: &dyn FieldValues,
        set: CharacterSet,
        unknown: &str,
        extension: &str,
    ) -> Vec<String> {
        let mut names = vec![String::new()];
        render_parts(&self.parts, values, set, unknown, &mut names);
        let count = names.len();
        names
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let last = i + 1 == count;
                let reserve = if last { extension.len() } else { 0 };
                let cleaned = clean_name(&name, set, !last);
                let cleaned = if cleaned.is_empty() {
                    clean_name(unknown, set, !last)
                } else {
                    cleaned
                };
                let cleaned = if cleaned.is_empty() {
                    REPLACEMENT.to_string()
                } else {
                    cleaned
                };
                shorten(&cleaned, MAX_NAME_BYTES.saturating_sub(reserve).max(1))
            })
            .collect()
    }

    /// [`render`](Self::render) as a relative path, without the extension.
    pub fn render_path(
        &self,
        values: &dyn FieldValues,
        set: CharacterSet,
        unknown: &str,
        extension: &str,
    ) -> PathBuf {
        self.render(values, set, unknown, extension)
            .into_iter()
            .collect()
    }
}

impl Default for NamingPattern {
    /// [`NamingPattern::DEFAULT`].
    fn default() -> Self {
        Self::parse(Self::DEFAULT).unwrap_or_else(|_| Self {
            text: Self::DEFAULT.to_owned(),
            parts: Vec::new(),
        })
    }
}

/// `name` (a playlist's, say) as the name of one file in `set`, without the
/// extension it gets; empty when nothing of it is left.
pub fn clean_file_name(name: &str, set: CharacterSet) -> String {
    if name.trim().chars().all(|c| c == '.') {
        return String::new();
    }
    let cleaned = clean_name(&clean_value(name, set), set, false);
    shorten(&cleaned, MAX_NAME_BYTES - 8)
}

fn collect_fields(parts: &[Part], out: &mut Vec<Field>) {
    for part in parts {
        match part {
            Part::Field(field) if !out.contains(field) => out.push(field.clone()),
            Part::Optional(inner) => collect_fields(inner, out),
            _ => {}
        }
    }
}

/// Reads parts until the end or the `]` that closes the bracket opened at `open`.
fn parse_parts(
    chars: &[char],
    position: &mut usize,
    open: Option<usize>,
) -> Result<Vec<Part>, PatternError> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let flush = |text: &mut String, parts: &mut Vec<Part>| {
        if !text.is_empty() {
            parts.push(Part::Text(std::mem::take(text)));
        }
    };
    while let Some(&c) = chars.get(*position) {
        match c {
            '{' => {
                flush(&mut text, &mut parts);
                let start = *position;
                let mut name = String::new();
                *position += 1;
                loop {
                    match chars.get(*position) {
                        None => {
                            return Err(PatternError {
                                problem: PatternProblem::UnclosedField,
                                start,
                                end: chars.len(),
                            });
                        }
                        Some('}') => break,
                        Some('{') => {
                            return Err(PatternError {
                                problem: PatternProblem::NestedField,
                                start: *position,
                                end: *position + 1,
                            });
                        }
                        Some(&c) => name.push(c),
                    }
                    *position += 1;
                }
                *position += 1;
                let trimmed = name.trim();
                if trimmed.is_empty() {
                    return Err(PatternError {
                        problem: PatternProblem::EmptyField,
                        start,
                        end: *position,
                    });
                }
                let Some(field) = Field::from_name(trimmed) else {
                    return Err(PatternError {
                        problem: PatternProblem::UnknownField(trimmed.to_owned()),
                        start,
                        end: *position,
                    });
                };
                parts.push(Part::Field(field));
            }
            '[' => {
                flush(&mut text, &mut parts);
                let start = *position;
                *position += 1;
                let inner = parse_parts(chars, position, Some(start))?;
                parts.push(Part::Optional(inner));
            }
            ']' => {
                if open.is_some() {
                    flush(&mut text, &mut parts);
                    *position += 1;
                    return Ok(parts);
                }
                return Err(PatternError {
                    problem: PatternProblem::UnexpectedClose(']'),
                    start: *position,
                    end: *position + 1,
                });
            }
            '}' => {
                return Err(PatternError {
                    problem: PatternProblem::UnexpectedClose('}'),
                    start: *position,
                    end: *position + 1,
                });
            }
            '/' => {
                flush(&mut text, &mut parts);
                parts.push(Part::Folder);
                *position += 1;
            }
            c => {
                text.push(c);
                *position += 1;
            }
        }
    }
    if let Some(start) = open {
        return Err(PatternError {
            problem: PatternProblem::UnclosedBracket,
            start,
            end: chars.len(),
        });
    }
    flush(&mut text, &mut parts);
    Ok(parts)
}

/// Refuses a pattern whose folder names would always be empty: a `/` at the
/// start or the end, or two with nothing between them but brackets and spaces.
fn check_folders(chars: &[char]) -> Result<(), PatternError> {
    let mut last_slash: Option<usize> = None;
    let mut seen_content = false;
    let mut in_field = false;
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '{' => {
                in_field = true;
                seen_content = true;
            }
            '}' => in_field = false,
            '/' if !in_field => {
                if !seen_content {
                    return Err(PatternError {
                        problem: PatternProblem::EmptyFolder,
                        start: last_slash.unwrap_or(i),
                        end: i + 1,
                    });
                }
                last_slash = Some(i);
                seen_content = false;
            }
            '[' | ']' => {}
            c if c.is_whitespace() => {}
            _ => seen_content = true,
        }
    }
    if !seen_content {
        let start = last_slash.unwrap_or(0);
        return Err(PatternError {
            problem: PatternProblem::EmptyFolder,
            start,
            end: chars.len(),
        });
    }
    Ok(())
}

/// Whether every field among `parts` has a value (those in inner brackets do not
/// count: an inner bracket is left out on its own).
fn all_present(parts: &[Part], values: &dyn FieldValues) -> bool {
    parts.iter().all(|part| match part {
        Part::Field(field) => field_text(field, values).is_some(),
        _ => true,
    })
}

fn render_parts(
    parts: &[Part],
    values: &dyn FieldValues,
    set: CharacterSet,
    unknown: &str,
    names: &mut Vec<String>,
) {
    for part in parts {
        match part {
            Part::Text(text) => push(names, text),
            Part::Field(field) => {
                let text = field_text(field, values).unwrap_or_else(|| unknown.to_owned());
                push(names, &clean_value(&text, set));
            }
            Part::Folder => names.push(String::new()),
            Part::Optional(inner) => {
                if all_present(inner, values) {
                    render_parts(inner, values, set, unknown, names);
                }
            }
        }
    }
}

fn push(names: &mut [String], text: &str) {
    if let Some(last) = names.last_mut() {
        last.push_str(text);
    }
}

/// A field's value, made unable to start a folder of its own.
fn clean_value(text: &str, set: CharacterSet) -> String {
    let text: String = text
        .chars()
        .map(|c| if c == '/' { REPLACEMENT } else { c })
        .collect();
    match set {
        CharacterSet::Ascii => to_ascii(&text),
        _ => text,
    }
}

/// `name` as a name of one file or folder in `set`; `folder` names lose trailing
/// dots too under the Windows rules.
fn clean_name(name: &str, set: CharacterSet, folder: bool) -> String {
    let name = match set {
        CharacterSet::Ascii => to_ascii(name),
        _ => name.to_owned(),
    };
    let mut out: String = name
        .chars()
        .filter(|&c| c != '\0')
        .map(|c| match set {
            CharacterSet::AsIs => {
                if c == '/' {
                    REPLACEMENT
                } else {
                    c
                }
            }
            CharacterSet::Windows | CharacterSet::Ascii => {
                if c.is_control()
                    || matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*' | '/')
                {
                    REPLACEMENT
                } else {
                    c
                }
            }
        })
        .collect();
    out = out.trim().to_owned();
    if set != CharacterSet::AsIs {
        if folder {
            while out.ends_with('.') || out.ends_with(' ') {
                out.pop();
            }
        }
        if is_device_name(&out) {
            out.insert(0, REPLACEMENT);
        }
    }
    if out == "." || out == ".." {
        out = REPLACEMENT.to_string();
    }
    out
}

/// Whether `name` is (or starts as) a device name Windows reserves.
fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end();
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.as_bytes()[3].is_ascii_digit()
            && upper.as_bytes()[3] != b'0')
}

/// `text` in plain ASCII: accents dropped, some letters spelled out, other
/// characters replaced.
fn to_ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
            continue;
        }
        if let Some(spelled) = spelled_out(c) {
            out.push_str(spelled);
            continue;
        }
        let mut base = None;
        let mut decomposed = String::new();
        decomposed.push(c);
        for d in icu_normalizer::DecomposingNormalizerBorrowed::new_nfkd()
            .normalize(&decomposed)
            .chars()
        {
            if d.is_ascii() {
                base.get_or_insert_with(String::new).push(d);
            } else if let Some(spelled) = spelled_out(d) {
                base.get_or_insert_with(String::new).push_str(spelled);
            } else if is_combining(d) {
                // An accent: dropped.
            } else {
                base = None;
                break;
            }
        }
        match base {
            Some(base) if !base.is_empty() => out.push_str(&base),
            _ => out.push(REPLACEMENT),
        }
    }
    out
}

/// Letters that do not decompose into a plain letter and an accent.
fn spelled_out(c: char) -> Option<&'static str> {
    Some(match c {
        'ß' => "ss",
        'ẞ' => "SS",
        'æ' => "ae",
        'Æ' => "AE",
        'œ' => "oe",
        'Œ' => "OE",
        'ø' => "o",
        'Ø' => "O",
        'đ' | 'ð' => "d",
        'Đ' | 'Ð' => "D",
        'ł' => "l",
        'Ł' => "L",
        'þ' => "th",
        'Þ' => "Th",
        'ı' => "i",
        '‘' | '’' | '‚' | '′' => "'",
        '“' | '”' | '„' | '″' => "\"",
        '–' | '—' | '‐' | '−' => "-",
        '…' => "...",
        '\u{00A0}' | '\u{2009}' | '\u{202F}' => " ",
        _ => return None,
    })
}

fn is_combining(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036F}'
        | '\u{1AB0}'..='\u{1AFF}'
        | '\u{1DC0}'..='\u{1DFF}'
        | '\u{20D0}'..='\u{20FF}'
        | '\u{FE20}'..='\u{FE2F}')
}

/// `name` cut to at most `limit` bytes at a character boundary, without a space
/// or dot left at its end.
fn shorten(name: &str, limit: usize) -> String {
    if name.len() <= limit {
        return name.to_owned();
    }
    let mut end = limit;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(field: &Field) -> Option<String> {
        match field {
            Field::AlbumArtist => Some("Mari Kask".into()),
            Field::Year => Some("2021".into()),
            Field::Album => Some("Tidewater Rooms".into()),
            Field::Track => Some("03".into()),
            Field::Title => Some("Shoreline, 6 a.m.".into()),
            Field::Artist => Some("AC/DC".into()),
            Field::Genre => Some("Jazz: Modern?".into()),
            _ => None,
        }
    }

    fn render(pattern: &str, set: CharacterSet) -> Vec<String> {
        NamingPattern::parse(pattern)
            .unwrap()
            .render(&values, set, "Unknown", ".flac")
    }

    #[test]
    fn the_drawn_pattern_names_folders_and_the_file() {
        assert_eq!(
            render(
                "{albumartist}/{year} {album}/[{disc}-]{track} {title}",
                CharacterSet::Windows
            ),
            ["Mari Kask", "2021 Tidewater Rooms", "03 Shoreline, 6 a.m."]
        );
    }

    #[test]
    fn brackets_keep_their_text_when_every_field_inside_has_a_value() {
        let with_disc = |field: &Field| match field {
            Field::Disc => Some("2".into()),
            other => values(other),
        };
        let names = NamingPattern::parse("[{disc}-]{track}").unwrap().render(
            &with_disc,
            CharacterSet::Windows,
            "Unknown",
            ".dsf",
        );
        assert_eq!(names, ["2-03"]);
        assert_eq!(render("[{disc}-]{track}", CharacterSet::Windows), ["03"]);
        assert_eq!(
            render("[{year} [{disc}]x ]{track}", CharacterSet::Windows),
            ["2021 x 03"],
            "an inner bracket is left out on its own"
        );
        assert_eq!(
            render("[{composer} [{year}]]{track}", CharacterSet::Windows),
            ["03"]
        );
    }

    #[test]
    fn an_empty_field_outside_brackets_is_written_as_the_given_word() {
        assert_eq!(
            render("{composer}/{title}", CharacterSet::Windows),
            ["Unknown", "Shoreline, 6 a.m."]
        );
    }

    #[test]
    fn a_value_never_starts_a_folder() {
        assert_eq!(render("{artist}", CharacterSet::AsIs), ["AC_DC"]);
    }

    #[test]
    fn windows_names_lose_reserved_characters_and_trailing_dots() {
        assert_eq!(render("{genre}", CharacterSet::Windows), ["Jazz_ Modern_"]);
        assert_eq!(render("{genre}", CharacterSet::AsIs), ["Jazz: Modern?"]);
        assert_eq!(
            render("{title}/x", CharacterSet::Windows),
            ["Shoreline, 6 a.m", "x"],
            "a folder name cannot end in a dot"
        );
        assert_eq!(render("CON", CharacterSet::Windows), ["_CON"]);
        assert_eq!(render("com1.x", CharacterSet::Windows), ["_com1.x"]);
        assert_eq!(render("COM10", CharacterSet::Windows), ["COM10"]);
    }

    #[test]
    fn plain_ascii_drops_accents_and_spells_out_letters() {
        let accents = |field: &Field| match field {
            Field::Title => Some("Kõpu Light – Straße Ærø 東京".into()),
            _ => None,
        };
        let names = NamingPattern::parse("{title}").unwrap().render(
            &accents,
            CharacterSet::Ascii,
            "Unknown",
            ".flac",
        );
        assert_eq!(names, ["Kopu Light - Strasse AEro __"]);
    }

    #[test]
    fn long_names_are_cut_to_leave_room_for_the_extension() {
        let long = |_: &Field| Some("é".repeat(200));
        let names = NamingPattern::parse("{title}/{title}").unwrap().render(
            &long,
            CharacterSet::AsIs,
            "Unknown",
            ".flac",
        );
        assert_eq!(names[0].len(), 254);
        assert!(names[1].len() + ".flac".len() <= MAX_NAME_BYTES);
    }

    #[test]
    fn dot_names_are_replaced() {
        let dots = |_: &Field| Some("..".into());
        let names = NamingPattern::parse("{title}/x").unwrap().render(
            &dots,
            CharacterSet::AsIs,
            "Unknown",
            ".flac",
        );
        assert_eq!(names, ["_", "x"]);
    }

    #[test]
    fn problems_are_reported_with_their_place() {
        let problem = |text: &str| NamingPattern::parse(text).unwrap_err();
        assert_eq!(problem("").problem, PatternProblem::Empty);
        assert_eq!(problem("{title").problem, PatternProblem::UnclosedField);
        assert_eq!(problem("a {}").problem, PatternProblem::EmptyField);
        let unknown = problem("{albumartist}/{nope}");
        assert_eq!(unknown.problem, PatternProblem::UnknownField("nope".into()));
        assert_eq!((unknown.start, unknown.end), (14, 20));
        assert_eq!(
            problem("[{disc}-{track}").problem,
            PatternProblem::UnclosedBracket
        );
        assert_eq!(problem("a]").problem, PatternProblem::UnexpectedClose(']'));
        assert_eq!(problem("a}").problem, PatternProblem::UnexpectedClose('}'));
        assert_eq!(problem("{a{b}").problem, PatternProblem::NestedField);
        assert_eq!(problem("/{title}").problem, PatternProblem::EmptyFolder);
        assert_eq!(
            problem("{album}//{title}").problem,
            PatternProblem::EmptyFolder
        );
        assert_eq!(problem("{album}/").problem, PatternProblem::EmptyFolder);
        assert!(problem("{x}").to_string().contains("at character 1"));
    }

    #[test]
    fn fields_are_listed_once_with_aliases_and_tags() {
        let pattern =
            NamingPattern::parse("{album_artist}/{tracknumber} {tag:mood} {track}").unwrap();
        assert_eq!(
            pattern.fields(),
            [Field::AlbumArtist, Field::Track, Field::Tag("MOOD".into())]
        );
        assert_eq!(
            pattern.text(),
            "{album_artist}/{tracknumber} {tag:mood} {track}"
        );
    }

    #[test]
    fn numbers_are_padded_and_the_disc_shows_only_on_albums_of_several_discs() {
        let pattern = NamingPattern::parse("[{disc}-]{track}").unwrap();
        let name = |values: &FieldMap| {
            pattern.render(values, CharacterSet::Windows, "Unknown", "")[0].clone()
        };
        let one = FieldMap::default()
            .with(Field::Track, "7")
            .with(Field::Disc, "1");
        assert_eq!(name(&one), "07", "one disc: no disc number");
        let mut two = one.clone();
        two.disc_count = Some(2);
        assert_eq!(name(&two), "1-07");
        let mut many = two.clone();
        many.disc_count = Some(12);
        many.track_count = Some(120);
        assert_eq!(name(&many), "01-007");
        let second = FieldMap::default()
            .with(Field::Track, "3")
            .with(Field::Disc, "2");
        assert_eq!(
            name(&second),
            "2-03",
            "a disc after the first shows several"
        );
        assert_eq!(
            NamingPattern::parse("{track}").unwrap().render(
                &two,
                CharacterSet::Windows,
                "Unknown",
                ""
            ),
            ["07"],
            "the track number never holds the disc"
        );
    }

    #[test]
    fn a_bracket_can_hold_a_folder() {
        let pattern = NamingPattern::parse("{album}/[Disc {disc}/]{track}").unwrap();
        let mut values = FieldMap::default()
            .with(Field::Album, "Tidewater Rooms")
            .with(Field::Track, "1")
            .with(Field::Disc, "2");
        assert_eq!(
            pattern.render_path(&values, CharacterSet::Windows, "Unknown", ".flac"),
            PathBuf::from("Tidewater Rooms/Disc 2/01")
        );
        values.values.remove(&Field::Disc);
        assert_eq!(
            pattern.render_path(&values, CharacterSet::Windows, "Unknown", ".flac"),
            PathBuf::from("Tidewater Rooms/01")
        );
    }

    #[test]
    fn the_default_pattern_reads() {
        let pattern = NamingPattern::default();
        assert_eq!(pattern.text(), NamingPattern::DEFAULT);
        assert_eq!(
            pattern.fields(),
            [
                Field::AlbumArtist,
                Field::Album,
                Field::Disc,
                Field::Track,
                Field::Title
            ]
        );
    }

    #[test]
    fn character_sets_have_stable_ids() {
        for set in CharacterSet::ALL {
            assert_eq!(CharacterSet::from_id(set.id()), Some(set));
        }
        assert_eq!(
            CharacterSet::from_id("portable"),
            Some(CharacterSet::Windows)
        );
        assert_eq!(CharacterSet::from_id("nope"), None);
    }

    #[test]
    fn file_names_of_their_own() {
        assert_eq!(
            clean_file_name("Road trip: 2026/07", CharacterSet::Windows),
            "Road trip_ 2026_07"
        );
        assert_eq!(
            clean_file_name("Sigur Rós: Ágætis byrjun", CharacterSet::Ascii),
            "Sigur Ros_ Agaetis byrjun"
        );
        assert_eq!(clean_file_name("a:b", CharacterSet::AsIs), "a:b");
        assert_eq!(clean_file_name("..", CharacterSet::AsIs), "");
    }
}
