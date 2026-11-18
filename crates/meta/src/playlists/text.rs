// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding a playlist's bytes into lines of text, keeping each line's own bytes.
//!
//! In order: a byte order mark (UTF-8, UTF-16LE, UTF-16BE) decides; text whose even
//! or odd bytes are mostly zero is UTF-16. Then the format decides: M3U8 and XSPF are
//! UTF-8, and bytes that are not valid UTF-8 are replaced in the text (the line's bytes
//! are kept). An M3U or PLS file is UTF-8 when it is valid UTF-8, and otherwise in the
//! legacy charset the user chose or else the one `chardetng` guesses from the whole
//! file. A chosen charset only replaces the guess.

use std::borrow::Cow;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

use super::warning::{PlaylistWarningKind, Warnings};

/// How the charset of a playlist was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EncodingSource {
    /// A byte order mark, or the zero bytes of UTF-16.
    ByteOrderMark,
    /// The bytes are valid UTF-8.
    Utf8,
    /// The format is UTF-8 by definition (M3U8, XSPF).
    Format,
    /// The charset the user chose.
    Chosen,
    /// Guessed from the bytes.
    Detected,
}

/// The charset a playlist was decoded with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextEncoding {
    encoding: &'static Encoding,
    /// How the charset was decided.
    pub source: EncodingSource,
    /// Whether some bytes were not valid in the charset and were replaced.
    pub had_errors: bool,
}

impl TextEncoding {
    /// The charset's WHATWG name, such as `UTF-8`, `windows-1251` or `Shift_JIS`.
    pub fn name(&self) -> &'static str {
        self.encoding.name()
    }

    /// Whether a line's bytes may differ from its text encoded as UTF-8: a legacy
    /// charset, not UTF-8 or UTF-16.
    pub(crate) fn is_legacy(&self) -> bool {
        self.encoding != UTF_8 && self.encoding != UTF_16LE && self.encoding != UTF_16BE
    }
}

/// Which charsets a format allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Charset {
    /// UTF-8 (or UTF-16 with a mark).
    Utf8,
    /// UTF-8, or else a legacy charset.
    Detect,
}

/// One line of a playlist.
#[derive(Debug)]
pub(crate) struct Line<'a> {
    /// Counted from 1.
    pub(crate) number: usize,
    /// The line's own bytes, without its end. For UTF-16 the text in UTF-8.
    pub(crate) raw: Cow<'a, [u8]>,
    /// The decoded line.
    pub(crate) text: String,
}

/// The whole text of `bytes`, for XML.
pub(crate) fn decode_document(
    bytes: &[u8],
    charset: Charset,
    chosen: Option<&'static Encoding>,
) -> (String, TextEncoding) {
    let (encoding, source, body) = choose(bytes, charset, chosen);
    let (text, had_errors) = encoding.decode_without_bom_handling(body);
    (
        text.into_owned(),
        TextEncoding {
            encoding,
            source,
            had_errors,
        },
    )
}

/// The first bytes of a file as text, for sniffing its format.
pub(crate) fn decode_head(bytes: &[u8]) -> (Cow<'_, str>, &'static Encoding) {
    let (encoding, _, body) = choose(bytes, Charset::Utf8, None);
    (encoding.decode_without_bom_handling(body).0, encoding)
}

/// The lines of `bytes`, split at LF, CRLF or CR. Lines longer than `max_line` bytes
/// are skipped with a warning.
pub(crate) fn decode_lines<'a>(
    bytes: &'a [u8],
    charset: Charset,
    chosen: Option<&'static Encoding>,
    max_line: usize,
    warnings: &mut Warnings,
) -> (Vec<Line<'a>>, TextEncoding) {
    let (encoding, source, body) = choose(bytes, charset, chosen);
    let mut had_errors = false;
    let mut lines = Vec::new();
    if encoding == UTF_16LE || encoding == UTF_16BE {
        let (text, errors) = encoding.decode_without_bom_handling(body);
        had_errors = errors;
        for (index, line) in split_lines(text.as_bytes()).enumerate() {
            if line.len() > max_line {
                warnings.at(index + 1, PlaylistWarningKind::LineTooLong);
                continue;
            }
            let text = String::from_utf8_lossy(line).into_owned();
            lines.push(Line {
                number: index + 1,
                raw: Cow::Owned(text.as_bytes().to_vec()),
                text,
            });
        }
    } else {
        for (index, line) in split_lines(body).enumerate() {
            if line.len() > max_line {
                warnings.at(index + 1, PlaylistWarningKind::LineTooLong);
                continue;
            }
            let (text, errors) = encoding.decode_without_bom_handling(line);
            had_errors |= errors;
            lines.push(Line {
                number: index + 1,
                raw: Cow::Borrowed(line),
                text: text.into_owned(),
            });
        }
    }
    let encoding = TextEncoding {
        encoding,
        source,
        had_errors,
    };
    if had_errors {
        warnings.push(
            None,
            PlaylistWarningKind::UndecodableText {
                encoding: encoding.name(),
            },
        );
    }
    (lines, encoding)
}

/// The lines of `bytes` without their ends.
fn split_lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = Some(bytes);
    std::iter::from_fn(move || {
        let current = rest?;
        match current.iter().position(|&b| b == b'\n' || b == b'\r') {
            Some(end) => {
                let after = if current[end] == b'\r' && current.get(end + 1) == Some(&b'\n') {
                    end + 2
                } else {
                    end + 1
                };
                rest = Some(&current[after..]);
                Some(&current[..end])
            }
            None => {
                rest = None;
                (!current.is_empty()).then_some(current)
            }
        }
    })
}

/// The charset of `bytes`, how it was decided, and the bytes after any mark.
fn choose<'a>(
    bytes: &'a [u8],
    charset: Charset,
    chosen: Option<&'static Encoding>,
) -> (&'static Encoding, EncodingSource, &'a [u8]) {
    if let Some((encoding, mark)) = Encoding::for_bom(bytes) {
        return (encoding, EncodingSource::ByteOrderMark, &bytes[mark..]);
    }
    if let Some(encoding) = utf16_without_bom(bytes) {
        return (encoding, EncodingSource::ByteOrderMark, bytes);
    }
    match charset {
        Charset::Utf8 => (UTF_8, EncodingSource::Format, bytes),
        Charset::Detect if std::str::from_utf8(bytes).is_ok() => {
            (UTF_8, EncodingSource::Utf8, bytes)
        }
        Charset::Detect => match chosen {
            Some(encoding) => (encoding, EncodingSource::Chosen, bytes),
            None => (guess(bytes), EncodingSource::Detected, bytes),
        },
    }
}

/// UTF-16 without a byte order mark: a playlist is mostly ASCII, so most of its high
/// bytes are zero. NUL bytes are valid UTF-8, so this comes first.
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

    fn lines(bytes: &[u8], charset: Charset) -> (Vec<(usize, Vec<u8>, String)>, TextEncoding) {
        let mut warnings = Warnings::default();
        let (lines, encoding) = decode_lines(bytes, charset, None, 64, &mut warnings);
        (
            lines
                .into_iter()
                .map(|l| (l.number, l.raw.into_owned(), l.text))
                .collect(),
            encoding,
        )
    }

    #[test]
    fn line_ends_of_every_kind() {
        let (lines, encoding) = lines(b"a\r\nb\rc\n\nd", Charset::Detect);
        assert_eq!(encoding.name(), "UTF-8");
        let numbers: Vec<_> = lines.iter().map(|l| (l.0, l.2.as_str())).collect();
        assert_eq!(numbers, [(1, "a"), (2, "b"), (3, "c"), (4, ""), (5, "d")]);
    }

    #[test]
    fn legacy_lines_keep_their_bytes() {
        let (bytes, _, _) =
            encoding_rs::WINDOWS_1251.encode("Кино - Группа крови\nКино\\Звезда.mp3\n");
        let (lines, encoding) = lines(&bytes, Charset::Detect);
        assert_eq!(encoding.name(), "windows-1251");
        assert_eq!(encoding.source, EncodingSource::Detected);
        assert_eq!(lines[1].2, "Кино\\Звезда.mp3");
        assert_eq!(
            lines[1].1,
            encoding_rs::WINDOWS_1251
                .encode("Кино\\Звезда.mp3")
                .0
                .as_ref()
        );
    }

    #[test]
    fn utf8_formats_replace_bad_bytes() {
        let mut warnings = Warnings::default();
        let (lines, encoding) =
            decode_lines(b"Caf\xe9.mp3", Charset::Utf8, None, 64, &mut warnings);
        assert!(encoding.had_errors);
        assert_eq!(lines[0].raw.as_ref(), b"Caf\xe9.mp3");
        assert_eq!(lines[0].text, "Caf\u{fffd}.mp3");
        assert_eq!(warnings.into_vec().len(), 1);
    }

    #[test]
    fn utf16_is_recognized_with_and_without_a_mark() {
        let text = "#EXTM3U\r\nКино.mp3\r\n";
        let mut le = vec![0xFF, 0xFE];
        let mut be = Vec::new();
        for unit in text.encode_utf16() {
            le.extend_from_slice(&unit.to_le_bytes());
            be.extend_from_slice(&unit.to_be_bytes());
        }
        for bytes in [le, be] {
            let (lines, encoding) = lines(&bytes, Charset::Utf8);
            assert!(encoding.name().starts_with("UTF-16"));
            assert_eq!(lines[1].2, "Кино.mp3");
            assert_eq!(lines[1].1, "Кино.mp3".as_bytes());
        }
    }

    #[test]
    fn long_lines_are_skipped() {
        let mut bytes = vec![b'a'; 100];
        bytes.extend_from_slice(b"\nb");
        let mut warnings = Warnings::default();
        let (lines, _) = decode_lines(&bytes, Charset::Detect, None, 64, &mut warnings);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].number, 2);
        assert_eq!(warnings.into_vec()[0].line, Some(1));
    }
}
