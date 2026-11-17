// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding a sheet's bytes into text.
//!
//! In order: a byte order mark (UTF-8, UTF-16LE, UTF-16BE) decides; text whose
//! even or odd bytes are mostly zero is UTF-16; valid UTF-8 is UTF-8; otherwise
//! the legacy charset the user chose is used, or else the one `chardetng` guesses
//! from the whole sheet. The chosen charset only replaces the
//! guess, as with tags: a UTF-8 sheet stays UTF-8 whatever the user's legacy
//! charset is.

use std::borrow::Cow;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

/// How the charset of a sheet was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingSource {
    /// A byte order mark.
    ByteOrderMark,
    /// The bytes are valid UTF-8.
    Utf8,
    /// The charset the user chose.
    Chosen,
    /// Guessed from the bytes.
    Detected,
    /// The sheet came as text, from a tag; the tag reader decoded it.
    Tag,
}

/// The charset a sheet was decoded with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SheetEncoding {
    encoding: &'static Encoding,
    /// How the charset was decided.
    pub source: EncodingSource,
    /// Whether some bytes were not valid in the charset and were replaced.
    pub had_errors: bool,
}

impl SheetEncoding {
    /// The charset's WHATWG name, such as `UTF-8`, `windows-1251` or `Shift_JIS`.
    pub fn name(&self) -> &'static str {
        self.encoding.name()
    }

    pub(crate) fn tag() -> Self {
        Self {
            encoding: UTF_8,
            source: EncodingSource::Tag,
            had_errors: false,
        }
    }

    /// `text` in the sheet's charset, as it was before decoding.
    pub(crate) fn encode(&self, text: &str) -> Vec<u8> {
        if self.had_errors || self.encoding == UTF_8 {
            return text.as_bytes().to_vec();
        }
        // UTF-16 cannot be produced by `encode`, which gives UTF-8 for it; file
        // names on disk are bytes and UTF-8 is the right reading then.
        let (bytes, _, errors) = self.encoding.encode(text);
        if errors {
            text.as_bytes().to_vec()
        } else {
            bytes.into_owned()
        }
    }
}

/// Decodes `bytes`; `chosen` replaces the guessed legacy charset.
pub(crate) fn decode(bytes: &[u8], chosen: Option<&'static Encoding>) -> (String, SheetEncoding) {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        let (text, had_errors) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return (
            text.into_owned(),
            SheetEncoding {
                encoding,
                source: EncodingSource::ByteOrderMark,
                had_errors,
            },
        );
    }
    if let Some(encoding) = utf16_without_bom(bytes) {
        let (text, had_errors) = encoding.decode_without_bom_handling(bytes);
        return (
            text.into_owned(),
            SheetEncoding {
                encoding,
                source: EncodingSource::Detected,
                had_errors,
            },
        );
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return (
            text.to_owned(),
            SheetEncoding {
                encoding: UTF_8,
                source: EncodingSource::Utf8,
                had_errors: false,
            },
        );
    }
    let (encoding, source) = match chosen {
        Some(e) => (e, EncodingSource::Chosen),
        None => (guess(bytes), EncodingSource::Detected),
    };
    let (text, had_errors) = encoding.decode_without_bom_handling(bytes);
    let text = match text {
        Cow::Borrowed(t) => t.to_owned(),
        Cow::Owned(t) => t,
    };
    (
        text,
        SheetEncoding {
            encoding,
            source,
            had_errors,
        },
    )
}

/// UTF-16 without a byte order mark: a sheet is mostly ASCII, so most of its
/// high bytes are zero. NUL bytes are valid UTF-8, so this comes first.
fn utf16_without_bom(bytes: &[u8]) -> Option<&'static Encoding> {
    let zeros_at = |parity: usize| {
        bytes
            .iter()
            .skip(parity)
            .step_by(2)
            .take(512)
            .filter(|&&b| b == 0)
            .count()
    };
    let pairs = (bytes.len() / 2).min(512);
    if pairs >= 4 {
        // Mostly zero on one side, nearly never on the other (a CJK or Cyrillic
        // character may have a zero low byte, as U+4E00 does).
        let (even, odd) = (zeros_at(0), zeros_at(1));
        if odd * 10 >= pairs * 6 && even * 10 <= pairs {
            return Some(UTF_16LE);
        }
        if even * 10 >= pairs * 6 && odd * 10 <= pairs {
            return Some(UTF_16BE);
        }
    }
    None
}

/// The legacy charset of `bytes`, which are not valid UTF-8.
fn guess(bytes: &[u8]) -> &'static Encoding {
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    detector.guess(None, Utf8Detection::Deny)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(text: &str, label: &str) -> Vec<u8> {
        let encoding = Encoding::for_label(label.as_bytes()).expect("known label");
        let (bytes, _, errors) = encoding.encode(text);
        assert!(!errors);
        bytes.into_owned()
    }

    #[test]
    fn byte_order_marks_decide() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("TITLE \"Ægypt\"".as_bytes());
        let (text, enc) = decode(&bytes, None);
        assert_eq!(text, "TITLE \"Ægypt\"");
        assert_eq!(enc.source, EncodingSource::ByteOrderMark);

        let mut bytes = vec![0xFF, 0xFE];
        for unit in "TITLE \"Кино\"".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let (text, enc) = decode(&bytes, None);
        assert_eq!(text, "TITLE \"Кино\"");
        assert_eq!(enc.name(), "UTF-16LE");
    }

    #[test]
    fn utf16_without_a_mark_is_recognized() {
        let mut bytes = Vec::new();
        for unit in "REM DATE 1999\r\nTITLE \"Кино\"\r\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        let (text, enc) = decode(&bytes, None);
        assert_eq!(enc.name(), "UTF-16BE");
        assert!(text.contains("Кино"));
    }

    #[test]
    fn legacy_charsets_are_guessed() {
        let sheet = "PERFORMER \"Кино\"\nTITLE \"Группа крови\"\nTRACK 01 AUDIO\n  TITLE \"Звезда по имени Солнце\"\n";
        let (text, enc) = decode(&encoded(sheet, "windows-1251"), None);
        assert_eq!(enc.name(), "windows-1251");
        assert_eq!(enc.source, EncodingSource::Detected);
        assert_eq!(text, sheet);

        let sheet = "PERFORMER \"荒井由実\"\nTITLE \"ひこうき雲\"\nTRACK 01 AUDIO\n  TITLE \"きっと言える\"\n";
        let (text, enc) = decode(&encoded(sheet, "shift_jis"), None);
        assert_eq!(enc.name(), "Shift_JIS");
        assert_eq!(text, sheet);
    }

    #[test]
    fn the_chosen_charset_replaces_the_guess_only() {
        let koi8 = Encoding::for_label(b"koi8-r");
        let (text, enc) = decode(&encoded("TITLE \"Ария\"", "koi8-r"), koi8);
        assert_eq!(text, "TITLE \"Ария\"");
        assert_eq!(enc.source, EncodingSource::Chosen);
        let (text, enc) = decode("TITLE \"Björk\"".as_bytes(), koi8);
        assert_eq!(text, "TITLE \"Björk\"");
        assert_eq!(enc.source, EncodingSource::Utf8);
    }

    #[test]
    fn names_encode_back_to_their_bytes() {
        let bytes = encoded(
            "FILE \"Кино.wav\" WAVE\nTITLE \"Группа крови\"",
            "windows-1251",
        );
        let (_, enc) = decode(&bytes, None);
        assert_eq!(enc.encode("Кино.wav"), encoded("Кино.wav", "windows-1251"));
    }
}
