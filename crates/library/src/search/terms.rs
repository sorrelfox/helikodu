// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning the text typed into a search into folded terms and FTS5 expressions.

use super::fold::fold;

/// The most terms a query uses; the rest are ignored.
const MAX_TERMS: usize = 16;

/// The shortest term the trigram index can look up, in characters.
pub(crate) const TRIGRAM_MIN_CHARS: usize = 3;

/// A folded query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Terms {
    /// The terms joined by single spaces, for ranking.
    pub(super) whole: String,
    /// The distinct terms, in the order typed.
    pub(super) terms: Vec<String>,
}

impl Terms {
    /// The folded terms of `text`; `None` if it has none.
    pub(super) fn parse(text: &str) -> Option<Self> {
        let folded = fold(text);
        let mut terms: Vec<String> = Vec::new();
        for term in folded.split_whitespace() {
            if terms.len() == MAX_TERMS {
                break;
            }
            if !terms.iter().any(|known| known == term) {
                terms.push(term.to_owned());
            }
        }
        if terms.is_empty() {
            return None;
        }
        Some(Self {
            whole: folded.split_whitespace().collect::<Vec<_>>().join(" "),
            terms,
        })
    }
}

/// Whether `term` is long enough for the trigram index.
pub(crate) fn is_long(term: &str) -> bool {
    term.chars().nth(TRIGRAM_MIN_CHARS - 1).is_some()
}

/// `term` as an FTS5 string: in double quotes, with quotes doubled. FTS5 operators and
/// column filters inside it are plain text.
pub(crate) fn fts_string(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// `term` as a word-prefix query for the word index, or `None` if the word tokenizer
/// would find no word in it (only punctuation or symbols).
pub(crate) fn word_prefix(term: &str) -> Option<String> {
    term.chars()
        .any(char::is_alphanumeric)
        .then(|| format!("{}*", fts_string(term)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_are_folded_split_and_distinct() {
        let terms = Terms::parse("  Björk   JOGA björk ").unwrap();
        assert_eq!(terms.terms, ["bjork", "joga"]);
        assert_eq!(terms.whole, "bjork joga bjork");
        assert_eq!(Terms::parse(" \t "), None);
    }

    #[test]
    fn length_counts_characters() {
        assert!(!is_long("ab"));
        assert!(is_long("abc"));
        assert!(!is_long("月亮"));
        assert!(is_long("月亮代"));
    }

    #[test]
    fn fts_strings_quote_operators() {
        assert_eq!(fts_string("a\"b"), "\"a\"\"b\"");
        assert_eq!(word_prefix("title:x").as_deref(), Some("\"title:x\"*"));
        assert_eq!(word_prefix("NOT"), Some("\"NOT\"*".to_owned()));
        assert_eq!(word_prefix("..."), None);
    }
}
