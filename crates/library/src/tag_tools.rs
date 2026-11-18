// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tag tools: the values they would give each file, worked out without
//! touching any file. A front end shows them beside the old values and writes
//! the ones the user keeps through the tag editor.
//!
//! - [`NamePattern`] fills fields from file names: `{track} - {artist} - {title}`,
//!   with `{skip}` for a part that is not needed. The extension is left out; a
//!   pattern with `/` also matches the folders above the file.
//! - [`number_tracks`] numbers files in their order, again from the start for each
//!   disc if wanted, with the total.
//! - [`change_case`] gives Title Case, Sentence case, lower case or UPPER CASE.
//! - [`Replace`] finds and replaces text, literally or by a regular expression.
//! - [`repair_garbled`] repairs mojibake, as the tag reader does.
//! - [`sort_name`] makes a sort name: "Beatles, The" or "Kask, Mari".

use std::path::Path;

use regex::{Regex, RegexBuilder};

pub use helikodu_meta::tags::charset::{GarbledRepair, repair_garbled};

/// The legacy charsets offered to repair garbled text, as WHATWG labels.
pub const LEGACY_CHARSETS: &[&str] = &[
    "windows-1251",
    "koi8-r",
    "koi8-u",
    "windows-1250",
    "windows-1253",
    "windows-1254",
    "windows-1255",
    "windows-1256",
    "windows-1257",
    "windows-1258",
    "windows-874",
    "shift_jis",
    "euc-jp",
    "gbk",
    "gb18030",
    "big5",
    "euc-kr",
];

/// The fields a name pattern can hold, by their names in the pattern.
pub const PATTERN_FIELDS: &[&str] = &[
    "track",
    "disc",
    "title",
    "artist",
    "albumartist",
    "album",
    "year",
    "genre",
    "composer",
    "comment",
    SKIP,
];

/// The pattern field for a part that is not needed.
pub const SKIP: &str = "skip";

/// Why a name pattern cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    /// A `{` without its `}`.
    #[error("a field is not closed with }}")]
    Unclosed,
    /// A field the pattern does not know.
    #[error("{{{0}}} is not a field")]
    UnknownField(String),
    /// Two fields with nothing between them, which cannot be told apart.
    #[error("two fields need something between them")]
    Adjacent,
    /// The pattern has no field.
    #[error("the pattern has no field")]
    NoField,
}

/// A pattern of file names with fields, such as `{track} - {artist} - {title}`.
#[derive(Debug, Clone)]
pub struct NamePattern {
    regex: Regex,
    fields: Vec<String>,
    depth: usize,
}

impl NamePattern {
    /// Parses `pattern`.
    pub fn parse(pattern: &str) -> Result<Self, PatternError> {
        let mut regex = String::from("^");
        let mut fields = Vec::new();
        let mut rest = pattern.trim();
        let mut last_was_field = false;
        while !rest.is_empty() {
            match rest.find('{') {
                Some(0) => {
                    let end = rest.find('}').ok_or(PatternError::Unclosed)?;
                    let name = rest[1..end].trim().to_lowercase();
                    if !PATTERN_FIELDS.contains(&name.as_str()) {
                        return Err(PatternError::UnknownField(name));
                    }
                    if last_was_field {
                        return Err(PatternError::Adjacent);
                    }
                    regex.push_str(match name.as_str() {
                        "track" | "disc" => r"(\d+)",
                        "year" => r"(\d{4}(?:-\d\d(?:-\d\d)?)?)",
                        _ => r"([^/]+?)",
                    });
                    if name != SKIP {
                        fields.push(name);
                    } else {
                        fields.push(String::new());
                    }
                    last_was_field = true;
                    rest = &rest[end + 1..];
                }
                found => {
                    let end = found.unwrap_or(rest.len());
                    regex.push_str(&regex::escape(&rest[..end]));
                    last_was_field = false;
                    rest = &rest[end..];
                }
            }
        }
        regex.push('$');
        if fields.iter().all(String::is_empty) {
            return Err(PatternError::NoField);
        }
        let regex = RegexBuilder::new(&regex)
            .build()
            .map_err(|_| PatternError::Unclosed)?;
        Ok(Self {
            regex,
            fields: fields.clone(),
            depth: pattern.matches('/').count(),
        })
    }

    /// The fields of the pattern, in order, without `{skip}`, each once.
    pub fn fields(&self) -> Vec<&str> {
        let mut fields: Vec<&str> = Vec::new();
        for field in self.fields.iter().filter(|f| !f.is_empty()) {
            if !fields.contains(&field.as_str()) {
                fields.push(field);
            }
        }
        fields
    }

    /// The values the file at `path` gives, by field; `None` when its name does
    /// not fit. Numbers lose their leading zeros; values are trimmed.
    pub fn values(&self, path: &Path) -> Option<Vec<(String, String)>> {
        let stem = path.file_stem()?.to_string_lossy().into_owned();
        let mut subject = stem;
        let mut folder = path.parent();
        for _ in 0..self.depth {
            let parent = folder?;
            let name = parent.file_name()?.to_string_lossy().into_owned();
            subject = format!("{name}/{subject}");
            folder = parent.parent();
        }
        let captures = self.regex.captures(&subject)?;
        let mut values: Vec<(String, String)> = Vec::new();
        for (i, field) in self.fields.iter().enumerate() {
            if field.is_empty() {
                continue;
            }
            let raw = captures.get(i + 1)?.as_str().trim();
            let value = if matches!(field.as_str(), "track" | "disc") {
                raw.trim_start_matches('0').to_owned()
            } else {
                raw.to_owned()
            };
            let value = if value.is_empty() && !raw.is_empty() {
                "0".to_owned()
            } else {
                value
            };
            if !values.iter().any(|(f, _)| f == field) {
                values.push((field.clone(), value));
            }
        }
        Some(values)
    }
}

/// How [`number_tracks`] numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Numbering {
    /// The first number.
    pub first: u32,
    /// Whether each disc starts again at the first number.
    pub per_disc: bool,
}

/// The track numbers and totals of files in this order, given each file's disc
/// number. With `per_disc`, the files of each disc are numbered on their own
/// (in their order) and the total is the count of that disc's files.
pub fn number_tracks(discs: &[Option<u32>], numbering: Numbering) -> Vec<(u32, u32)> {
    if !numbering.per_disc {
        let total = u32::try_from(discs.len()).unwrap_or(u32::MAX);
        let last = numbering.first.saturating_add(total.saturating_sub(1));
        return (0..total)
            .map(|i| (numbering.first.saturating_add(i), last.max(total)))
            .collect();
    }
    let mut seen: Vec<(Option<u32>, u32)> = Vec::new();
    let mut numbers = Vec::with_capacity(discs.len());
    for disc in discs {
        let count = match seen.iter_mut().find(|(d, _)| d == disc) {
            Some((_, n)) => {
                *n += 1;
                *n
            }
            None => {
                seen.push((*disc, 1));
                1
            }
        };
        numbers.push(numbering.first.saturating_add(count - 1));
    }
    discs
        .iter()
        .zip(numbers)
        .map(|(disc, number)| {
            let count = seen.iter().find(|(d, _)| d == disc).map_or(1, |(_, n)| *n);
            let last = numbering.first.saturating_add(count.saturating_sub(1));
            (number, last.max(count))
        })
        .collect()
}

/// The ways [`change_case`] writes capital letters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Case {
    /// Each word starts with a capital letter, short English words in the middle
    /// excepted ("Song of the Sea").
    Title,
    /// Only the first word starts with a capital letter.
    Sentence,
    /// Everything in small letters.
    Lower,
    /// Everything in capital letters.
    Upper,
}

/// Short words Title Case leaves in small letters inside a title.
const MINOR_WORDS: &[&str] = &[
    "a", "an", "and", "as", "at", "but", "by", "feat.", "for", "from", "in", "nor", "of", "on",
    "or", "the", "to", "vs.", "with",
];

fn capitalized(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut start = true;
    for c in word.chars() {
        if start && c.is_alphanumeric() {
            out.extend(c.to_uppercase());
            start = false;
        } else {
            out.extend(c.to_lowercase());
            // A part after a hyphen, slash or bracket starts again.
            if matches!(c, '-' | '/' | '(' | '[' | '"') {
                start = true;
            }
        }
    }
    out
}

/// `text` with its capital letters changed to `case`.
pub fn change_case(text: &str, case: Case) -> String {
    match case {
        Case::Lower => text.to_lowercase(),
        Case::Upper => text.to_uppercase(),
        Case::Sentence => {
            let lower = text.to_lowercase();
            let mut out = String::with_capacity(lower.len());
            let mut done = false;
            for c in lower.chars() {
                if !done && c.is_alphanumeric() {
                    out.extend(c.to_uppercase());
                    done = true;
                } else {
                    out.push(c);
                }
            }
            out
        }
        Case::Title => {
            let words: Vec<&str> = text.split(' ').collect();
            let last = words.iter().rposition(|w| !w.is_empty()).unwrap_or(0);
            let first = words.iter().position(|w| !w.is_empty()).unwrap_or(0);
            words
                .iter()
                .enumerate()
                .map(|(i, word)| {
                    let lower = word.to_lowercase();
                    let after_break = i > 0 && words[i - 1].ends_with([':', '(', '-']);
                    if i != first
                        && i != last
                        && !after_break
                        && MINOR_WORDS.contains(&lower.as_str())
                    {
                        lower
                    } else {
                        capitalized(word)
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

/// A find and replace.
#[derive(Debug, Clone)]
pub struct Replace {
    regex: Regex,
    replacement: String,
    literal: bool,
}

impl Replace {
    /// Replaces `find` by `replacement`; `find` is a regular expression when
    /// `regular` (and `replacement` may then use `$1`), otherwise plain text.
    pub fn new(
        find: &str,
        replacement: &str,
        regular: bool,
        match_case: bool,
    ) -> Result<Self, regex::Error> {
        let pattern = if regular {
            find.to_owned()
        } else {
            regex::escape(find)
        };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!match_case)
            .size_limit(1 << 20)
            .build()?;
        Ok(Self {
            regex,
            replacement: replacement.to_owned(),
            literal: !regular,
        })
    }

    /// `text` with every match replaced.
    pub fn apply(&self, text: &str) -> String {
        if self.literal {
            self.regex
                .replace_all(text, regex::NoExpand(&self.replacement))
                .into_owned()
        } else {
            self.regex
                .replace_all(text, self.replacement.as_str())
                .into_owned()
        }
    }
}

/// The forms [`sort_name`] can give.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SortForms {
    /// "The Beatles" becomes "Beatles, The".
    pub article_last: bool,
    /// "Mari Kask" becomes "Kask, Mari".
    pub surname_first: bool,
}

/// Articles moved to the end of a sort name.
const ARTICLES: &[&str] = &["The", "A", "An"];

/// The sort name of `name` in `forms`; `None` when no form changes it.
pub fn sort_name(name: &str, forms: SortForms) -> Option<String> {
    let name = name.trim();
    if forms.article_last {
        for article in ARTICLES {
            if let Some(rest) = name
                .strip_prefix(article)
                .and_then(|r| r.strip_prefix(' '))
                .filter(|r| !r.trim().is_empty())
            {
                return Some(format!("{}, {article}", rest.trim()));
            }
        }
    }
    if forms.surname_first && !name.contains(',') {
        let words: Vec<&str> = name.split_whitespace().collect();
        if words.len() >= 2 && words.iter().all(|w| w.chars().any(char::is_alphabetic)) {
            let (last, given) = words.split_last()?;
            return Some(format!("{last}, {}", given.join(" ")));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_fill_fields() {
        let pattern = NamePattern::parse("{track} - {artist} - {title}").unwrap();
        assert_eq!(pattern.fields(), ["track", "artist", "title"]);
        let values = pattern
            .values(Path::new("/m/01 - Okto - Departure.flac"))
            .unwrap();
        assert_eq!(
            values,
            [
                ("track".to_owned(), "1".to_owned()),
                ("artist".to_owned(), "Okto".to_owned()),
                ("title".to_owned(), "Departure".to_owned())
            ]
        );
        let feat = pattern
            .values(Path::new("11 - Okto feat. Sola - Car Deck (Live).flac"))
            .unwrap();
        assert_eq!(feat[1].1, "Okto feat. Sola");
        assert_eq!(feat[2].1, "Car Deck (Live)");
        assert!(
            pattern
                .values(Path::new("bonus_open_water_live.flac"))
                .is_none()
        );
    }

    #[test]
    fn patterns_skip_and_reach_into_folders() {
        let pattern = NamePattern::parse("{album}/{skip} {title}").unwrap();
        assert_eq!(pattern.fields(), ["album", "title"]);
        let values = pattern
            .values(Path::new("/m/Letters/02 Juniper Road.mp3"))
            .unwrap();
        assert_eq!(values[0].1, "Letters");
        assert_eq!(values[1].1, "Juniper Road");
        assert_eq!(
            NamePattern::parse("{title").unwrap_err(),
            PatternError::Unclosed
        );
        assert_eq!(
            NamePattern::parse("{nope}").unwrap_err(),
            PatternError::UnknownField("nope".into())
        );
        assert_eq!(
            NamePattern::parse("{artist}{title}").unwrap_err(),
            PatternError::Adjacent
        );
        assert_eq!(
            NamePattern::parse("{skip}").unwrap_err(),
            PatternError::NoField
        );
        let zero = NamePattern::parse("{track} {title}").unwrap();
        assert_eq!(zero.values(Path::new("00 Intro.ogg")).unwrap()[0].1, "0");
    }

    #[test]
    fn tracks_are_numbered() {
        let plain = Numbering {
            first: 1,
            per_disc: false,
        };
        assert_eq!(
            number_tracks(&[None, None, None], plain),
            [(1, 3), (2, 3), (3, 3)]
        );
        let per_disc = Numbering {
            first: 1,
            per_disc: true,
        };
        assert_eq!(
            number_tracks(&[Some(1), Some(1), Some(2)], per_disc),
            [(1, 2), (2, 2), (1, 1)]
        );
        let from_five = Numbering {
            first: 5,
            per_disc: false,
        };
        assert_eq!(number_tracks(&[None, None], from_five), [(5, 6), (6, 6)]);
    }

    #[test]
    fn capital_letters_change() {
        assert_eq!(
            change_case("song OF the sea", Case::Title),
            "Song of the Sea"
        );
        assert_eq!(
            change_case("the end of the world", Case::Title),
            "The End of the World"
        );
        assert_eq!(change_case("rock-and-roll", Case::Title), "Rock-And-Roll");
        assert_eq!(change_case("don't stop", Case::Title), "Don't Stop");
        assert_eq!(change_case("ÕHTU LAUL", Case::Sentence), "Õhtu laul");
        assert_eq!(change_case("Kõpu", Case::Upper), "KÕPU");
        assert_eq!(change_case("Kõpu", Case::Lower), "kõpu");
    }

    #[test]
    fn text_is_replaced() {
        let plain = Replace::new("(Demo)", "[Demo]", false, true).unwrap();
        assert_eq!(plain.apply("Night Ferry (Demo)"), "Night Ferry [Demo]");
        let any_case = Replace::new("feat", "ft", false, false).unwrap();
        assert_eq!(any_case.apply("A FEAT B"), "A ft B");
        let regular = Replace::new(r"^(\d+)\. ", "$1 - ", true, true).unwrap();
        assert_eq!(regular.apply("01. Wake"), "01 - Wake");
        let dollars = Replace::new("x", "$1", false, true).unwrap();
        assert_eq!(dollars.apply("x"), "$1");
        assert!(Replace::new("(", "", true, true).is_err());
    }

    #[test]
    fn sort_names() {
        let both = SortForms {
            article_last: true,
            surname_first: true,
        };
        assert_eq!(
            sort_name("The Beatles", both).as_deref(),
            Some("Beatles, The")
        );
        assert_eq!(sort_name("Mari Kask", both).as_deref(), Some("Kask, Mari"));
        assert_eq!(
            sort_name("Johann Sebastian Bach", both).as_deref(),
            Some("Bach, Johann Sebastian")
        );
        assert_eq!(sort_name("Okto", both), None);
        assert_eq!(sort_name("Kask, Mari", both), None);
        let articles = SortForms {
            article_last: true,
            surname_first: false,
        };
        assert_eq!(sort_name("Mari Kask", articles), None);
        assert_eq!(sort_name("Theatre", articles), None);
    }

    #[test]
    fn garbled_text_is_repaired() {
        let broken: String = encoding_rs_free_cp1251("Кино");
        let found = repair_garbled(&[&broken], None);
        assert_eq!(found.repaired[0].as_deref(), Some("Кино"));
    }

    /// "Кино" in cp1251, read as Latin-1.
    fn encoding_rs_free_cp1251(_: &str) -> String {
        [0xCA_u8, 0xE8, 0xED, 0xEE]
            .iter()
            .map(|&b| char::from(b))
            .collect()
    }
}
