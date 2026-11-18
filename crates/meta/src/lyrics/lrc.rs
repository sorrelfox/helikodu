// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The LRC parser.
//!
//! LRC as found in the wild: one or more time tags at the start of a line
//! (`[01:02.34]`, `[1:02]`, `[01:02:34]`, `[1:01:02.345]`), metadata tags on
//! lines of their own (`[ti:…]`, `[ar:…]`, `[offset:+250]`, `[length:3:12]`),
//! and in "enhanced" files word tags inside the text (`<01:02.50>word`), which
//! are taken out of the text and kept as [`WordTiming`]s. Line ends may be
//! LF, CRLF or CR.
//!
//! The parser never fails: what it cannot read is either kept as a plain line
//! or skipped. It is bounded: at most [`MAX_LINES`] lines, [`MAX_STAMPS`] time
//! tags per line, [`MAX_LINE_CHARS`] characters per line and times up to
//! [`MAX_TIME`].

use std::time::Duration;

use super::{LyricLine, WordTiming};

/// The most lines kept, timed or not.
pub const MAX_LINES: usize = 5_000;
/// The most time tags read at the start of one line.
pub const MAX_STAMPS: usize = 64;
/// The most characters kept of one line.
pub const MAX_LINE_CHARS: usize = 1_000;
/// The latest time a tag may give.
pub const MAX_TIME: Duration = Duration::from_secs(24 * 3600);
/// The largest `[offset:]`, either way, in milliseconds.
pub const MAX_OFFSET_MS: i64 = 3_600_000;
/// The most unknown metadata tags kept.
const MAX_OTHER_TAGS: usize = 32;

/// The metadata tags of an LRC file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LrcMetadata {
    /// `[ti:]`, the title.
    pub title: Option<String>,
    /// `[ar:]`, the artist.
    pub artist: Option<String>,
    /// `[al:]`, the album.
    pub album: Option<String>,
    /// `[au:]`, the author of the song text.
    pub author: Option<String>,
    /// `[by:]`, who made the LRC file.
    pub by: Option<String>,
    /// `[length:]`, the length of the song.
    pub length: Option<Duration>,
    /// `[offset:]` in milliseconds: positive shows the lines sooner. It is
    /// already applied to the times of the lines.
    pub offset_ms: i64,
    /// `[la:]` or `[lang:]`, the language.
    pub language: Option<String>,
    /// Other tags, such as `[re:]` and `[ve:]`, in file order.
    pub other: Vec<(String, String)>,
}

/// What [`parse_lrc`] read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedLrc {
    /// The timed lines, by time, with the offset applied; lines with several
    /// time tags appear once per tag.
    pub timed: Vec<LyricLine>,
    /// The lines without a time tag, in file order (blank lines between verses
    /// are kept as `verse_start` of the next line).
    pub plain: Vec<LyricLine>,
    /// The metadata tags.
    pub metadata: LrcMetadata,
    /// Lines that were cut or dropped to stay within the bounds.
    pub truncated: bool,
}

/// A timed entry before sorting.
struct Entry {
    time: Duration,
    order: usize,
    text: String,
    words: Vec<WordTiming>,
    blank_before: bool,
}

/// Parses `text` as LRC. Text without any time tag comes back as plain lines.
pub fn parse_lrc(text: &str) -> ParsedLrc {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = ParsedLrc::default();
    let mut entries: Vec<Entry> = Vec::new();
    let mut blank_before = false;
    let mut seen_text = false;
    for raw in split_lines(text) {
        if entries.len() + out.plain.len() >= MAX_LINES {
            out.truncated = true;
            break;
        }
        let line = raw.trim();
        if line.is_empty() {
            blank_before = seen_text;
            continue;
        }
        let (stamps, rest) = leading_stamps(line);
        if !stamps.is_empty() {
            let (clean, mut words) = take_word_tags(rest.trim());
            let (clean, cut) = bounded(clean);
            out.truncated |= cut;
            // Words that started in the part cut off start at the end.
            for word in &mut words {
                word.start = word.start.min(clean.len());
            }
            for time in stamps {
                entries.push(Entry {
                    time,
                    order: entries.len(),
                    text: clean.clone(),
                    words: words.clone(),
                    blank_before,
                });
            }
            seen_text = true;
            blank_before = false;
            continue;
        }
        if let Some((key, value)) = metadata_tag(line) {
            apply_metadata(&mut out.metadata, &key, value);
            continue;
        }
        // Not a tag line: plain text. Word tags without line tags are
        // dropped from the text.
        let (clean, _) = take_word_tags(line);
        let (clean, cut) = bounded(clean);
        out.truncated |= cut;
        out.plain.push(LyricLine {
            time: None,
            text: clean,
            words: Vec::new(),
            verse_start: blank_before && !out.plain.is_empty(),
        });
        seen_text = true;
        blank_before = false;
    }
    entries.truncate(MAX_LINES);

    let offset = out.metadata.offset_ms;
    for entry in &mut entries {
        entry.time = shift(entry.time, offset);
        for word in &mut entry.words {
            word.time = shift(word.time, offset);
        }
    }
    entries.sort_by(|a, b| a.time.cmp(&b.time).then(a.order.cmp(&b.order)));

    // An empty timed line is a pause between verses: the next line starts a
    // verse and the empty one is not shown.
    let mut gap = false;
    for entry in entries {
        if entry.text.is_empty() {
            gap = !out.timed.is_empty();
            continue;
        }
        let verse_start = !out.timed.is_empty() && (gap || entry.blank_before);
        gap = false;
        out.timed.push(LyricLine {
            time: Some(entry.time),
            text: entry.text,
            words: entry.words,
            verse_start,
        });
    }
    out
}

/// Lines split at LF, CRLF or a lone CR.
fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n')
        .flat_map(|l| l.strip_suffix('\r').unwrap_or(l).split('\r'))
}

/// `time` moved by an `[offset:]` of `offset_ms`: positive is sooner.
fn shift(time: Duration, offset_ms: i64) -> Duration {
    let ms = i64::try_from(time.as_millis()).unwrap_or(i64::MAX);
    Duration::from_millis(u64::try_from(ms.saturating_sub(offset_ms)).unwrap_or(0))
}

/// At most [`MAX_LINE_CHARS`] characters of `text`; whether it was cut.
fn bounded(text: String) -> (String, bool) {
    match text.char_indices().nth(MAX_LINE_CHARS) {
        Some((at, _)) => (text[..at].trim_end().to_owned(), true),
        None => (text, false),
    }
}

/// The time tags at the start of `line` and the rest of it.
fn leading_stamps(line: &str) -> (Vec<Duration>, &str) {
    let mut stamps = Vec::new();
    let mut rest = line;
    loop {
        let trimmed = rest.trim_start();
        let Some(inner) = trimmed.strip_prefix('[') else {
            break;
        };
        let Some(close) = inner.find(']') else {
            break;
        };
        let Some(time) = parse_time(&inner[..close]) else {
            break;
        };
        if stamps.len() < MAX_STAMPS {
            stamps.push(time);
        }
        rest = &inner[close + 1..];
    }
    if stamps.is_empty() {
        (stamps, line)
    } else {
        (stamps, rest)
    }
}

/// A time such as `01:02.34`, `1:02`, `01:02:34` (hundredths after a colon),
/// `1:01:02.345` (hours) or `02.5` is not one: minutes are needed.
pub(crate) fn parse_time(text: &str) -> Option<Duration> {
    let text = text.trim();
    if text.is_empty() || text.len() > 16 {
        return None;
    }
    let parts: Vec<&str> = text.split(':').collect();
    let number = |s: &str| -> Option<u64> {
        (!s.is_empty() && s.len() <= 6 && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    // Seconds with an optional fraction after a dot.
    let seconds = |s: &str| -> Option<(u64, u64)> {
        let (whole, fraction) = match s.split_once('.') {
            Some((w, f)) => (w, Some(f)),
            None => (s, None),
        };
        let whole = number(whole)?;
        let millis = match fraction {
            None => 0,
            Some(f) => fraction_millis(f)?,
        };
        Some((whole, millis))
    };
    let (hours, minutes, secs, millis) = match parts.as_slice() {
        [m, s] => {
            let (s, ms) = seconds(s)?;
            (0, number(m)?, s, ms)
        }
        [a, b, c] => {
            // `h:mm:ss.fff`, or `mm:ss:xx` with hundredths.
            if c.contains('.') {
                let (s, ms) = seconds(c)?;
                (number(a)?, number(b)?, s, ms)
            } else if c.len() <= 3 {
                (0, number(a)?, number(b)?, fraction_millis(c)?)
            } else {
                return None;
            }
        }
        _ => return None,
    };
    if secs >= 60 || (hours > 0 && minutes >= 60) {
        return None;
    }
    let total = hours
        .checked_mul(3600)?
        .checked_add(minutes.checked_mul(60)?)?
        .checked_add(secs)?;
    let time = Duration::from_secs(total) + Duration::from_millis(millis);
    (time <= MAX_TIME).then_some(time)
}

/// Milliseconds of a decimal fraction: `5` is 500, `34` is 340, `345` is 345;
/// digits after the third are ignored.
fn fraction_millis(digits: &str) -> Option<u64> {
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut millis = 0u64;
    for (i, b) in digits.bytes().take(3).enumerate() {
        millis += u64::from(b - b'0') * [100, 10, 1][i];
    }
    Some(millis)
}

/// `[key:value]` with an alphabetic key (or `#` for a comment).
fn metadata_tag(line: &str) -> Option<(String, &str)> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let (key, value) = inner.split_once(':')?;
    let key = key.trim();
    if key.is_empty()
        || key.len() > 16
        || !(key == "#" || key.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_'))
    {
        return None;
    }
    Some((key.to_ascii_lowercase(), value.trim()))
}

fn apply_metadata(metadata: &mut LrcMetadata, key: &str, value: &str) {
    let text = || {
        let (v, _) = bounded(value.to_owned());
        (!v.is_empty()).then_some(v)
    };
    match key {
        "ti" => metadata.title = text(),
        "ar" => metadata.artist = text(),
        "al" => metadata.album = text(),
        "au" => metadata.author = text(),
        "by" => metadata.by = text(),
        "la" | "lang" => metadata.language = text(),
        "length" => metadata.length = parse_time(value),
        "offset" => {
            let v = value.trim().trim_start_matches('+');
            if let Ok(ms) = v.parse::<i64>() {
                metadata.offset_ms = ms.clamp(-MAX_OFFSET_MS, MAX_OFFSET_MS);
            }
        }
        _ => {
            if metadata.other.len() < MAX_OTHER_TAGS
                && let Some(v) = text()
            {
                metadata.other.push((key.to_owned(), v));
            }
        }
    }
}

/// Takes the enhanced-LRC word tags (`<mm:ss.xx>`) out of `text`: the text
/// without them, and each tag's time with the byte offset in that text where
/// its word starts.
fn take_word_tags(text: &str) -> (String, Vec<WordTiming>) {
    if !text.contains('<') {
        return (text.to_owned(), Vec::new());
    }
    let mut clean = String::with_capacity(text.len());
    let mut words = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        match after.find('>') {
            Some(close) if close <= 16 => {
                if let Some(time) = parse_time(&after[..close]) {
                    clean.push_str(&rest[..open]);
                    if words.len() < MAX_LINE_CHARS {
                        words.push(WordTiming {
                            time,
                            start: clean.len(),
                        });
                    }
                    rest = &after[close + 1..];
                    continue;
                }
                clean.push_str(&rest[..=open]);
                rest = after;
            }
            _ => {
                clean.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    clean.push_str(rest);
    // Spaces next to the tags at the ends are not part of the line.
    let lead = clean.len() - clean.trim_start().len();
    let trimmed = clean.trim().to_owned();
    for word in &mut words {
        word.start = word.start.saturating_sub(lead).min(trimmed.len());
    }
    (trimmed, words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    fn texts(lines: &[LyricLine]) -> Vec<&str> {
        lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn times_in_their_common_shapes() {
        assert_eq!(parse_time("01:02.34"), Some(ms(62_340)));
        assert_eq!(parse_time("1:02"), Some(ms(62_000)));
        assert_eq!(parse_time("01:02.5"), Some(ms(62_500)));
        assert_eq!(parse_time("01:02.345"), Some(ms(62_345)));
        assert_eq!(parse_time("01:02:34"), Some(ms(62_340)));
        assert_eq!(parse_time("1:01:02.345"), Some(ms(3_662_345)));
        assert_eq!(parse_time(" 00:05.00 "), Some(ms(5_000)));
        assert_eq!(parse_time("125:00.00"), Some(ms(7_500_000)));
        for bad in [
            "", "ti", "1", "01:60.00", "aa:bb", "-1:00", "01:02.x", "1:2:3:4",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn lines_metadata_and_several_times_per_line() {
        let lrc = "[ti:Shoreline, 6 a.m.]\n[ar:Mari Kask]\n[al:Tidewater Rooms]\n[by:someone]\n\
                   [length: 4:55]\n[re:Some Editor 1.0]\n\
                   [00:46.00]The tide comes in on quiet feet\n\
                   [00:58.20][02:38.00]Grey light folding over the bay\n\
                   [01:11.00] I count the boats \n";
        let parsed = parse_lrc(lrc);
        assert_eq!(
            texts(&parsed.timed),
            [
                "The tide comes in on quiet feet",
                "Grey light folding over the bay",
                "I count the boats",
                "Grey light folding over the bay",
            ]
        );
        let times: Vec<_> = parsed.timed.iter().map(|l| l.time).collect();
        assert_eq!(
            times,
            [
                Some(ms(46_000)),
                Some(ms(58_200)),
                Some(ms(71_000)),
                Some(ms(158_000))
            ]
        );
        let m = &parsed.metadata;
        assert_eq!(m.title.as_deref(), Some("Shoreline, 6 a.m."));
        assert_eq!(m.artist.as_deref(), Some("Mari Kask"));
        assert_eq!(m.album.as_deref(), Some("Tidewater Rooms"));
        assert_eq!(m.by.as_deref(), Some("someone"));
        assert_eq!(m.length, Some(ms(295_000)));
        assert_eq!(m.other, [("re".to_owned(), "Some Editor 1.0".to_owned())]);
        assert!(parsed.plain.is_empty());
    }

    #[test]
    fn offsets_move_the_lines_and_stop_at_zero() {
        let parsed = parse_lrc("[offset:+500]\n[00:00.20]a\n[00:10.00]b\n");
        assert_eq!(parsed.metadata.offset_ms, 500);
        assert_eq!(parsed.timed[0].time, Some(Duration::ZERO));
        assert_eq!(parsed.timed[1].time, Some(ms(9_500)));
        let parsed = parse_lrc("[offset: -250]\n[00:10.00]b\n");
        assert_eq!(parsed.timed[0].time, Some(ms(10_250)));
        let parsed = parse_lrc("[offset:nonsense]\n[00:10.00]b\n");
        assert_eq!(parsed.metadata.offset_ms, 0);
        let parsed = parse_lrc("[offset:99999999999]\n[00:10.00]b\n");
        assert_eq!(parsed.metadata.offset_ms, MAX_OFFSET_MS);
    }

    #[test]
    fn crlf_cr_and_byte_order_mark() {
        let parsed = parse_lrc("\u{feff}[ti:x]\r\n[00:01.00]one\r\n[00:02.00]two\rthree\r\n");
        assert_eq!(texts(&parsed.timed), ["one", "two"]);
        assert_eq!(texts(&parsed.plain), ["three"]);
        assert_eq!(parsed.metadata.title.as_deref(), Some("x"));
    }

    #[test]
    fn enhanced_word_tags_are_kept_apart() {
        let parsed = parse_lrc("[00:12.00]<00:12.00> Salt <00:12.40>on the <00:13.10>window\n");
        let line = &parsed.timed[0];
        assert_eq!(line.text, "Salt on the window");
        let words: Vec<_> = line
            .words
            .iter()
            .map(|w| (w.time, &line.text[w.start..]))
            .collect();
        assert_eq!(
            words,
            [
                (ms(12_000), "Salt on the window"),
                (ms(12_400), "on the window"),
                (ms(13_100), "window"),
            ]
        );
        // Angle brackets that are not times stay.
        assert_eq!(parse_lrc("[00:01.00]a <b> c").timed[0].text, "a <b> c");
    }

    #[test]
    fn verses_from_blank_lines_and_empty_timed_lines() {
        let parsed = parse_lrc(
            "[00:01.00]one\n[00:02.00]two\n\n[00:03.00]three\n[00:04.00]\n[00:05.00]four\n",
        );
        let verses: Vec<_> = parsed.timed.iter().map(|l| l.verse_start).collect();
        assert_eq!(texts(&parsed.timed), ["one", "two", "three", "four"]);
        assert_eq!(verses, [false, false, true, true]);
    }

    #[test]
    fn text_without_times_is_plain() {
        let parsed = parse_lrc("[ar:Someone]\nFirst line\nSecond line\n\n\nThird [line]\n");
        assert!(parsed.timed.is_empty());
        assert_eq!(
            texts(&parsed.plain),
            ["First line", "Second line", "Third [line]"]
        );
        let verses: Vec<_> = parsed.plain.iter().map(|l| l.verse_start).collect();
        assert_eq!(verses, [false, false, true]);
    }

    /// A line cut to its bound kept word offsets past its end; found by fuzzing.
    #[test]
    fn words_of_a_cut_line_stay_inside_it() {
        let long = format!(
            "[00:01.00]{} <00:02.00>tail  <00:03.00>end",
            "x".repeat(MAX_LINE_CHARS)
        );
        let parsed = parse_lrc(&long);
        let line = &parsed.timed[0];
        assert!(!line.words.is_empty());
        for word in &line.words {
            assert!(word.start <= line.text.len());
            let _ = &line.text[word.start..];
        }
    }

    #[test]
    fn broken_tags_are_text_and_bounds_hold() {
        let parsed = parse_lrc("[00:01.00\n[xx:01.00]oops\n[]\n[00:02.00]fine\n");
        assert_eq!(texts(&parsed.timed), ["fine"]);
        assert_eq!(texts(&parsed.plain), ["[00:01.00", "[xx:01.00]oops", "[]"]);

        let long = format!("[00:01.00]{}", "é".repeat(MAX_LINE_CHARS + 50));
        let parsed = parse_lrc(&long);
        assert_eq!(parsed.timed[0].text.chars().count(), MAX_LINE_CHARS);
        assert!(parsed.truncated);

        let many: String = (0..MAX_LINES + 10)
            .map(|i| format!("[00:{:02}.00]l{i}\n", i % 60))
            .collect();
        let parsed = parse_lrc(&many);
        assert_eq!(parsed.timed.len(), MAX_LINES);
        assert!(parsed.truncated);

        let stamps = format!("{}x", "[00:01.00]".repeat(MAX_STAMPS * 2));
        assert_eq!(parse_lrc(&stamps).timed.len(), MAX_STAMPS);
    }
}
