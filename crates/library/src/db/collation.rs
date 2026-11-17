// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `HK_LOCALE` collation: the alphabet order of the interface language, from ICU4X.

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use icu_collator::options::CollatorOptions;
use icu_collator::{Collator, CollatorBorrowed};
use icu_locale_core::Locale;
use rusqlite::Connection;

use super::DbError;

/// The name under which every connection registers the collation.
pub(crate) const COLLATION_NAME: &str = "HK_LOCALE";

/// A locale's sort order, for the `HK_LOCALE` collation and for sorting in Rust.
///
/// Cloning is cheap; clones share the collator.
#[derive(Clone)]
pub struct LocaleCollation {
    locale: Arc<str>,
    collator: Arc<CollatorBorrowed<'static>>,
}

impl LocaleCollation {
    /// The sort order for `locale`.
    ///
    /// Takes a BCP 47 tag (`sv-SE`, `et`, `de-u-co-phonebk`) or a POSIX locale name as
    /// found in `LANG` (`sv_SE.UTF-8`, `et_EE@euro`). `C`, `POSIX` and the empty string
    /// select the root order, which suits English and is the base of every other locale.
    /// A language without its own rules sorts in the root order.
    pub fn new(locale: &str) -> Result<Self, DbError> {
        let tag = bcp47_from(locale);
        let parsed = Locale::try_from_str(&tag).map_err(|error| DbError::Locale {
            locale: locale.to_owned(),
            reason: error.to_string(),
        })?;
        let collator =
            Collator::try_new((&parsed).into(), CollatorOptions::default()).map_err(|error| {
                DbError::Locale {
                    locale: locale.to_owned(),
                    reason: error.to_string(),
                }
            })?;
        Ok(Self {
            locale: parsed.to_string().into(),
            collator: Arc::new(collator),
        })
    }

    /// The root order, which suits English and is the base of every other locale.
    pub fn root() -> Self {
        // The root locale is always present in the compiled data.
        Self::new("und").unwrap_or_else(|_| unreachable!("the root collation is compiled in"))
    }

    /// The locale as a canonical BCP 47 tag, for example `sv-SE`.
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// Compares two strings in this locale's order.
    pub fn compare(&self, left: &str, right: &str) -> Ordering {
        self.collator.compare(left, right)
    }

    /// Registers this order as the `HK_LOCALE` collation of `connection`, replacing an
    /// earlier registration.
    pub(crate) fn register(&self, connection: &Connection) -> rusqlite::Result<()> {
        let collator = Arc::clone(&self.collator);
        connection.create_collation(COLLATION_NAME, move |left, right| {
            collator.compare(left, right)
        })
    }
}

impl fmt::Debug for LocaleCollation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocaleCollation")
            .field("locale", &self.locale)
            .finish_non_exhaustive()
    }
}

/// Turns a POSIX locale name into a BCP 47 tag; a BCP 47 tag passes unchanged.
fn bcp47_from(locale: &str) -> String {
    let name = locale.trim();
    // `language_TERRITORY.codeset@modifier`: the code set and modifier say nothing about
    // the alphabet order that ICU4X understands.
    let name = name.split(['.', '@']).next().unwrap_or_default();
    match name {
        "" | "C" | "POSIX" => "und".to_owned(),
        _ => name.replace('_', "-"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(locale: &str, words: &[&str]) -> Vec<String> {
        let collation = LocaleCollation::new(locale).unwrap();
        let mut words: Vec<String> = words.iter().map(|&w| w.to_owned()).collect();
        words.sort_by(|a, b| collation.compare(a, b));
        words
    }

    #[test]
    fn posix_names_become_tags() {
        assert_eq!(bcp47_from("sv_SE.UTF-8"), "sv-SE");
        assert_eq!(bcp47_from("et_EE@euro"), "et-EE");
        assert_eq!(bcp47_from("C.UTF-8"), "und");
        assert_eq!(bcp47_from("de-u-co-phonebk"), "de-u-co-phonebk");
        assert_eq!(
            LocaleCollation::new("sv_SE.UTF-8").unwrap().locale(),
            "sv-SE"
        );
    }

    #[test]
    fn malformed_locale_is_refused() {
        assert!(matches!(
            LocaleCollation::new("not a locale!"),
            Err(DbError::Locale { .. })
        ));
    }

    #[test]
    fn swedish_puts_a_ring_a_umlaut_o_umlaut_after_z() {
        let words = ["Öst", "Zebra", "Ängel", "Åsa", "Apa", "Ödla"];
        assert_eq!(
            sorted("sv", &words),
            ["Apa", "Zebra", "Åsa", "Ängel", "Ödla", "Öst"]
        );
        // The root order treats them as accented A and O.
        assert_eq!(
            sorted("en", &words),
            ["Ängel", "Apa", "Åsa", "Ödla", "Öst", "Zebra"]
        );
    }

    #[test]
    fn german_sorts_umlauts_with_their_base_letters() {
        let words = ["Zug", "Äpfel", "Ofen", "Öl", "Apfel", "Über", "Ufer"];
        assert_eq!(
            sorted("de", &words),
            ["Apfel", "Äpfel", "Ofen", "Öl", "Über", "Ufer", "Zug"]
        );
    }

    #[test]
    fn estonian_follows_its_alphabet() {
        // … s š z ž t u v w õ ä ö ü x y
        let words = [
            "tuli", "žanr", "zoo", "šokk", "sai", "ülikool", "õun", "vesi", "ämber", "öö", "yks",
        ];
        assert_eq!(
            sorted("et", &words),
            [
                "sai", "šokk", "zoo", "žanr", "tuli", "vesi", "õun", "ämber", "öö", "ülikool",
                "yks"
            ]
        );
    }
}
