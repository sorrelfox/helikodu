// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Naming patterns: how the organizer (and later the converter and folder sync)
//! name files and folders from their tags.
//!
//! The syntax:
//!
//! - `{field}` is the value of a field, named as the query language names it
//!   (`{albumartist}`, `{year}`, `{track}`, `{tag:MOOD}`);
//! - `/` starts a folder;
//! - text in square brackets is left out when a field inside it is empty
//!   (`[{disc}-]{track}`); brackets may nest, and an inner bracket is left out
//!   on its own;
//! - everything else is written as it is.
//!
//! The file extension is never part of a pattern: the caller keeps the file's
//! own. A field that is empty outside square brackets is written as a word the
//! caller gives (such as "Unknown"), so that no name is ever empty.
//!
//! Names are then cleaned for the file system by a [`CharacterSet`]: a value can
//! never start a folder of its own (a `/` inside a value is replaced), and the
//! characters that the chosen set does not allow are replaced too.

use std::fmt;

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

    /// The folders and the file name (without its extension) for a file whose
    /// field values `value` gives (`None` or an empty text for an empty field),
    /// cleaned by `set`. An empty field outside brackets is written as `unknown`.
    /// The last name is kept short enough for `extension` (with its dot) to fit.
    pub fn render(
        &self,
        value: &dyn Fn(&Field) -> Option<String>,
        set: CharacterSet,
        unknown: &str,
        extension: &str,
    ) -> Vec<String> {
        let mut names = vec![String::new()];
        render_parts(&self.parts, value, set, unknown, &mut names);
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

/// Whether every field among `parts` has a value.
fn all_present(parts: &[Part], value: &dyn Fn(&Field) -> Option<String>) -> bool {
    parts.iter().all(|part| match part {
        Part::Field(field) => value(field).is_some_and(|v| !v.trim().is_empty()),
        _ => true,
    })
}

fn render_parts(
    parts: &[Part],
    value: &dyn Fn(&Field) -> Option<String>,
    set: CharacterSet,
    unknown: &str,
    names: &mut Vec<String>,
) {
    for part in parts {
        match part {
            Part::Text(text) => push(names, text),
            Part::Field(field) => {
                let text = value(field)
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| unknown.to_owned());
                push(names, &clean_value(&text, set));
            }
            Part::Folder => names.push(String::new()),
            Part::Optional(inner) => {
                if all_present(inner, value) {
                    render_parts(inner, value, set, unknown, names);
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
}
