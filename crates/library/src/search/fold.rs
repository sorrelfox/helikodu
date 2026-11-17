// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Folding: the case- and accent-insensitive form of text.

use icu_normalizer::{ComposingNormalizerBorrowed, DecomposingNormalizerBorrowed};

/// The folded form of `text`, for search, grouping and album keys.
///
/// The text is compatibility-decomposed (NFKD), lower-cased, stripped of the diacritics
/// on Latin and Greek letters, and recomposed (NFC). Language-specific folds apply too:
/// `ß` becomes `ss`, `ё` becomes `е`, and final `ς` becomes `σ`.
///
/// Marks on other scripts stay. They are vowels and tone marks in Thai and Devanagari,
/// and the voicing marks of Japanese kana, so removing them would change the word.
/// Full-width Latin letters and half-width katakana become their ordinary forms.
///
/// The same function feeds the database (as the SQL function `hk_fold`) and the
/// queries, so the index and the search always agree.
pub fn fold(text: &str) -> String {
    let decomposed = DecomposingNormalizerBorrowed::new_nfkd().normalize(text);
    let lower = decomposed.to_lowercase();
    let mut folded = String::with_capacity(lower.len());
    // The script of the last character that was not a combining mark.
    let mut base = Base::Other;
    for c in lower.chars() {
        if is_diacritic(c) {
            let strip = match base {
                Base::LatinOrGreek => true,
                // `ё` is `е` with a diaeresis; Russian text is written both ways.
                Base::CyrillicIe => c == '\u{0308}',
                Base::Other => false,
            };
            if !strip {
                folded.push(c);
            }
            continue;
        }
        base = Base::of(c);
        match c {
            'ß' => folded.push_str("ss"),
            'ς' => folded.push('σ'),
            _ => folded.push(c),
        }
    }
    ComposingNormalizerBorrowed::new_nfc()
        .normalize(&folded)
        .into_owned()
}

#[derive(Clone, Copy)]
enum Base {
    LatinOrGreek,
    CyrillicIe,
    Other,
}

impl Base {
    fn of(c: char) -> Self {
        match c {
            'е' => Self::CyrillicIe,
            '\u{0000}'..='\u{024F}'     // Basic Latin to Latin Extended-B
            | '\u{0370}'..='\u{03FF}'   // Greek and Coptic
            | '\u{1E00}'..='\u{1FFF}'   // Latin Extended Additional, Greek Extended
            | '\u{2C60}'..='\u{2C7F}'   // Latin Extended-C
            | '\u{A720}'..='\u{A7FF}'   // Latin Extended-D
            | '\u{AB30}'..='\u{AB6F}' => Self::LatinOrGreek, // Latin Extended-E
            _ => Self::Other,
        }
    }
}

/// Whether `c` is a combining diacritical mark, as opposed to a mark that is part of the
/// letters of a script.
fn is_diacritic(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036F}'     // Combining Diacritical Marks
        | '\u{1AB0}'..='\u{1AFF}'   // Combining Diacritical Marks Extended
        | '\u{1DC0}'..='\u{1DFF}'   // Combining Diacritical Marks Supplement
        | '\u{20D0}'..='\u{20FF}'   // Combining Diacritical Marks for Symbols
        | '\u{FE20}'..='\u{FE2F}' // Combining Half Marks
    )
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn case_and_latin_diacritics() {
        assert_eq!(fold("Ärzte"), "arzte");
        assert_eq!(fold("Björk Guðmundsdóttir"), "bjork guðmundsdottir");
        assert_eq!(fold("Tiếng Việt"), "tieng viet");
        assert_eq!(fold("Dvořák"), "dvorak");
        assert_eq!(fold("İstanbul"), "istanbul");
    }

    #[test]
    fn decomposed_input_folds_like_composed_input() {
        assert_eq!(fold("Cafe\u{0301}"), fold("Café"));
        assert_eq!(fold("Café"), "cafe");
    }

    #[test]
    fn language_specific_folds() {
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("STRAẞE"), "strasse");
        assert_eq!(fold("Ёлка"), "елка");
        assert_eq!(fold("ёлка"), fold("елка"));
        assert_eq!(fold("ΆΣΜΑΤΑ ΣΟΦΟΣ"), "ασματα σοφοσ");
    }

    #[test]
    fn letters_with_marks_in_other_scripts_stay() {
        assert_eq!(fold("Чайковский"), "чайковский");
        assert_eq!(fold("ภาษาไทย"), "ภาษาไทย");
        assert_eq!(fold("がっこう"), "がっこう");
        assert_eq!(fold("हिन्दी"), "हिन्दी");
    }

    #[test]
    fn compatibility_forms() {
        assert_eq!(fold("ＡＢＣ１２３"), "abc123");
        assert_eq!(fold("ｶﾞｯｺｳ"), "ガッコウ");
        assert_eq!(fold("ﬁne"), "fine");
    }

    #[test]
    fn folding_is_idempotent() {
        for text in ["Ärzte", "Straße", "ΆΣΜΑ", "ｶﾞｯｺｳ", "Ёлка", "中文歌曲"]
        {
            assert_eq!(fold(&fold(text)), fold(text), "{text}");
        }
    }
}
