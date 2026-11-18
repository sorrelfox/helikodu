// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Repair of tag text written in a legacy charset.
//!
//! Old taggers wrote ID3v1 fields and ID3v2.3 "Latin-1" frames in whatever code
//! page the system used: cp1251 for Cyrillic, Shift-JIS for Japanese, GBK for
//! Chinese. Read as Latin-1, such text becomes mojibake ("Êèíî" for "Кино"). Other
//! programs then copied the mojibake into UTF-8 frames, and some wrote UTF-8 bytes
//! into frames declared as Latin-1.
//!
//! The repair is conservative. Text is only touched when its characters can be
//! turned back into the original bytes (every character is in Latin-1 or is one of
//! the windows-1252 punctuation marks), and then only:
//!
//! 1. when the bytes are valid UTF-8 with at least one multi-byte sequence: they
//!    were UTF-8 all along;
//! 2. when the bytes look like mojibake (runs of three or more non-ASCII bytes, or
//!    mostly non-ASCII letters) and decode cleanly in the legacy charset guessed for
//!    the whole tag, giving text that is mostly in a non-Latin script (Cyrillic,
//!    Greek, Hebrew, Arabic, Thai, CJK, Hangul);
//! 3. when the text holds C1 control characters, which are windows-1252 punctuation
//!    (curly quotes, dashes) read as Latin-1.
//!
//! The charset is guessed once per tag from all of its suspicious text, which gives
//! the detector more to work with than a single field. A charset chosen by the user
//! replaces the guess.

use std::borrow::Cow;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};

/// The windows-1252 characters at bytes 0x80–0x9F, in byte order; `None` where
/// windows-1252 leaves the byte undefined.
const CP1252_HIGH: [Option<char>; 32] = [
    Some('\u{20ac}'),
    None,
    Some('\u{201a}'),
    Some('\u{0192}'),
    Some('\u{201e}'),
    Some('\u{2026}'),
    Some('\u{2020}'),
    Some('\u{2021}'),
    Some('\u{02c6}'),
    Some('\u{2030}'),
    Some('\u{0160}'),
    Some('\u{2039}'),
    Some('\u{0152}'),
    None,
    Some('\u{017d}'),
    None,
    None,
    Some('\u{2018}'),
    Some('\u{2019}'),
    Some('\u{201c}'),
    Some('\u{201d}'),
    Some('\u{2022}'),
    Some('\u{2013}'),
    Some('\u{2014}'),
    Some('\u{02dc}'),
    Some('\u{2122}'),
    Some('\u{0161}'),
    Some('\u{203a}'),
    Some('\u{0153}'),
    None,
    Some('\u{017e}'),
    Some('\u{0178}'),
];

/// The bytes `text` was decoded from, if it was decoded byte by byte as Latin-1 or
/// windows-1252; `None` when it holds a character neither can produce.
pub(crate) fn recover_bytes(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len());
    for c in text.chars() {
        if let Ok(b) = u8::try_from(u32::from(c)) {
            bytes.push(b);
        } else {
            let i = CP1252_HIGH.iter().position(|x| *x == Some(c))?;
            bytes.push(0x80 + i as u8);
        }
    }
    Some(bytes)
}

/// Whether `bytes`, read as Latin-1, look like text in another charset.
///
/// Western European text has its accented letters one at a time between ASCII
/// letters ("Motörhead", "Sigur Rós"); legacy Cyrillic, Greek and CJK text is made
/// of runs of high bytes, or alternates lead bytes with ASCII trail bytes.
fn looks_like_mojibake(bytes: &[u8]) -> bool {
    let mut high = 0usize;
    let mut ascii_alnum = 0usize;
    let mut run = 0usize;
    let mut longest_run = 0usize;
    for &b in bytes {
        if b >= 0x80 {
            high += 1;
            run += 1;
            longest_run = longest_run.max(run);
        } else {
            run = 0;
            if b.is_ascii_alphanumeric() {
                ascii_alnum += 1;
            }
        }
    }
    if high == 0 {
        return false;
    }
    longest_run >= 3 || (high >= 2 && high * 2 >= high + ascii_alnum)
}

/// Whether decoded text is mostly in a script that Latin-1 cannot hold.
fn plausible_repair(text: &str) -> bool {
    let mut non_ascii = 0usize;
    let mut foreign = 0usize;
    for c in text.chars() {
        if c == '\u{fffd}' || (c.is_control() && !matches!(c, '\t' | '\n' | '\r')) {
            return false;
        }
        if c.is_ascii() {
            continue;
        }
        non_ascii += 1;
        let u = u32::from(c);
        let non_latin_script = matches!(u,
            0x0370..=0x03FF      // Greek
            | 0x0400..=0x052F    // Cyrillic
            | 0x0590..=0x05FF    // Hebrew
            | 0x0600..=0x06FF    // Arabic
            | 0x0E00..=0x0E7F    // Thai
            | 0x1100..=0x11FF    // Hangul Jamo
            | 0x2E80..=0x9FFF    // CJK radicals, punctuation, kana, ideographs
            | 0xAC00..=0xD7AF    // Hangul syllables
            | 0xF900..=0xFAFF    // CJK compatibility ideographs
            | 0xFF00..=0xFFEF    // half- and full-width forms
        );
        if non_latin_script {
            foreign += 1;
        }
    }
    foreign > 0 && foreign * 10 >= non_ascii * 6
}

/// The charset repair for one tag.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CharsetRepair {
    legacy: Option<&'static Encoding>,
}

impl CharsetRepair {
    /// Prepares the repair of the text of one tag: guesses its legacy charset from
    /// every suspicious value, unless `preferred` names one.
    pub(crate) fn detect<'a>(
        texts: impl IntoIterator<Item = &'a str>,
        preferred: Option<&'static Encoding>,
    ) -> Self {
        let mut sample = Vec::new();
        for text in texts {
            let Some(bytes) = recover_bytes(text) else {
                continue;
            };
            if looks_like_mojibake(&bytes) && std::str::from_utf8(&bytes).is_err() {
                sample.extend_from_slice(&bytes);
                sample.push(b'\n');
            }
        }
        if sample.is_empty() {
            return Self::default();
        }
        let legacy = preferred.unwrap_or_else(|| {
            let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
            detector.feed(&sample, true);
            detector.guess(None, Utf8Detection::Deny)
        });
        let legacy = (legacy != UTF_8 && legacy != WINDOWS_1252).then_some(legacy);
        Self { legacy }
    }

    /// The repaired form of `text`, or `None` when it is left as it is.
    pub(crate) fn repair(&self, text: &str) -> Option<String> {
        let bytes = recover_bytes(text)?;
        if bytes.is_ascii() {
            return None;
        }
        if let Ok(utf8) = std::str::from_utf8(&bytes) {
            return Some(utf8.to_owned());
        }
        if looks_like_mojibake(&bytes) {
            let legacy = self.legacy?;
            let (decoded, had_errors) = legacy.decode_without_bom_handling(&bytes);
            return (!had_errors && plausible_repair(&decoded)).then(|| decoded.into_owned());
        }
        if text.chars().any(|c| ('\u{80}'..='\u{9f}').contains(&c)) {
            let (decoded, _) = WINDOWS_1252.decode_without_bom_handling(&bytes);
            if let Cow::Owned(decoded) = decoded {
                return Some(decoded);
            }
        }
        None
    }

    /// The name of the legacy charset in use, if any.
    #[cfg(test)]
    fn legacy_name(&self) -> Option<&'static str> {
        self.legacy.map(Encoding::name)
    }
}

/// The charset named by `label` (`cp1251`, `shift_jis`, `gbk`, …), as the WHATWG
/// Encoding Standard names them.
pub(crate) fn encoding_for_label(label: &str) -> Option<&'static Encoding> {
    Encoding::for_label(label.trim().as_bytes())
}

/// What [`repair_garbled`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GarbledRepair {
    /// The legacy charset the text was taken to be in, as its WHATWG name
    /// (`windows-1251`, `Shift_JIS`, …); `None` when no text needed one.
    pub charset: Option<&'static str>,
    /// The repaired form of each text, or `None` when it stays as it is.
    pub repaired: Vec<Option<String>>,
}

/// Repairs the garbled text among `texts` as the reader does (see the module
/// documentation), with the charset guessed from all of them together, or the
/// charset `label` names when one is given and known.
pub fn repair_garbled(texts: &[&str], label: Option<&str>) -> GarbledRepair {
    let preferred = label.and_then(encoding_for_label);
    let repair = CharsetRepair::detect(texts.iter().copied(), preferred);
    GarbledRepair {
        charset: repair.legacy.map(Encoding::name),
        repaired: texts.iter().map(|t| repair.repair(t)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text as a Latin-1 reader shows the bytes of `text` encoded in `label`.
    fn as_latin1(text: &str, label: &str) -> String {
        let encoding = encoding_for_label(label).expect("known label");
        let (bytes, _, errors) = encoding.encode(text);
        assert!(!errors, "{text} fits {label}");
        bytes.iter().map(|&b| char::from(b)).collect()
    }

    fn repair_alone(text: &str) -> Option<String> {
        CharsetRepair::detect([text], None).repair(text)
    }

    #[test]
    fn garbled_text_is_repaired_with_the_chosen_charset() {
        let broken = as_latin1("Кино", "windows-1251");
        let found = repair_garbled(&[&broken, "Plain"], None);
        assert_eq!(found.charset, Some("windows-1251"));
        assert_eq!(found.repaired, vec![Some("Кино".to_owned()), None]);
        let koi = repair_garbled(&[&broken], Some("koi8-r"));
        assert_eq!(koi.charset, Some("KOI8-R"));
        assert_ne!(koi.repaired[0].as_deref(), Some("Кино"));
    }

    #[test]
    fn cyrillic_in_cp1251() {
        let texts = ["Кино", "Группа крови", "Звезда по имени Солнце"];
        let mojibake: Vec<String> = texts.iter().map(|t| as_latin1(t, "windows-1251")).collect();
        let repair = CharsetRepair::detect(mojibake.iter().map(String::as_str), None);
        assert_eq!(repair.legacy_name(), Some("windows-1251"));
        for (text, broken) in texts.iter().zip(&mojibake) {
            assert_eq!(repair.repair(broken).as_deref(), Some(*text));
        }
    }

    #[test]
    fn japanese_in_shift_jis() {
        let texts = ["カラオケ", "ひこうき雲", "荒井由実"];
        let mojibake: Vec<String> = texts.iter().map(|t| as_latin1(t, "shift_jis")).collect();
        let repair = CharsetRepair::detect(mojibake.iter().map(String::as_str), None);
        assert_eq!(repair.legacy_name(), Some("Shift_JIS"));
        for (text, broken) in texts.iter().zip(&mojibake) {
            assert_eq!(repair.repair(broken).as_deref(), Some(*text));
        }
    }

    #[test]
    fn chinese_in_gbk() {
        let texts = ["月亮代表我的心", "邓丽君", "甜蜜蜜"];
        let mojibake: Vec<String> = texts.iter().map(|t| as_latin1(t, "gbk")).collect();
        let repair = CharsetRepair::detect(mojibake.iter().map(String::as_str), None);
        assert_eq!(repair.legacy_name(), Some("GBK"));
        for (text, broken) in texts.iter().zip(&mojibake) {
            assert_eq!(repair.repair(broken).as_deref(), Some(*text));
        }
    }

    #[test]
    fn a_preferred_charset_replaces_the_guess() {
        let broken = as_latin1("Ария", "koi8-r");
        let repair = CharsetRepair::detect([broken.as_str()], encoding_for_label("koi8-r"));
        assert_eq!(repair.repair(&broken).as_deref(), Some("Ария"));
    }

    #[test]
    fn utf8_bytes_in_a_latin1_frame() {
        let broken = as_latin1("Sigur Rós – Ágætis byrjun", "utf-8");
        assert_eq!(
            repair_alone(&broken).as_deref(),
            Some("Sigur Rós – Ágætis byrjun")
        );
        let broken = as_latin1("Кино", "utf-8");
        assert_eq!(repair_alone(&broken).as_deref(), Some("Кино"));
    }

    #[test]
    fn windows_1252_punctuation_read_as_latin1() {
        let broken: String = b"Don\x92t Stop \x96 Live"
            .iter()
            .map(|&b| char::from(b))
            .collect();
        assert_eq!(repair_alone(&broken).as_deref(), Some("Don’t Stop – Live"));
    }

    #[test]
    fn real_latin1_text_is_left_alone() {
        let texts = [
            "Motörhead",
            "Björk",
            "Sigur Rós",
            "Ágætis byrjun",
            "Café del Mar",
            "Mañana",
            "Ænima",
            "Œuvres complètes",
            "Les Misérables",
            "Für Elise",
            "Ça plane pour moi",
            "naïve",
            "Ærø",
            "Ö",
            "Señor Coconut y su Conjunto",
            "Hüsker Dü",
            "Mötley Crüe",
            "Blue Öyster Cult",
            "Queensrÿche",
            "Don’t Stop",
            "Dvořák",
            "Plain ASCII",
            "Antonín Dvořák: Symfonie č. 9",
            "Ελληνικά",
            "Кино",
            "東京事変",
            "Beyoncé — Déjà Vu",
            "François Pérusse",
            "São Paulo",
            "Zoë Keating",
            "Gábor Szabó",
        ];
        let repair = CharsetRepair::detect(texts, None);
        for text in texts {
            assert_eq!(repair.repair(text), None, "{text}");
            assert_eq!(repair_alone(text), None, "{text}");
        }
    }

    #[test]
    fn latin1_text_with_a_wrong_preferred_charset_is_left_alone() {
        let repair = CharsetRepair::detect(["Motörhead", "Björk"], encoding_for_label("cp1251"));
        assert_eq!(repair.repair("Motörhead"), None);
        assert_eq!(repair.repair("Björk"), None);
    }

    #[test]
    fn mojibake_detection() {
        assert!(looks_like_mojibake(&[0xCA, 0xE8, 0xED, 0xEE]));
        assert!(!looks_like_mojibake(b"Mot\xf6rhead"));
        assert!(!looks_like_mojibake(b"\xd6"));
        assert!(!looks_like_mojibake(b"plain"));
    }

    #[test]
    fn bytes_are_recovered_from_both_readings() {
        assert_eq!(
            recover_bytes("a\u{e9}\u{2019}"),
            Some(vec![b'a', 0xE9, 0x92])
        );
        assert_eq!(recover_bytes("\u{0159}"), None);
    }
}
