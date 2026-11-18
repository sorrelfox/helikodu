// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parses arbitrary text as LRC lyrics. Any panic is a bug, and so are lines
//! out of time order, a word offset outside its line, or more lines than the
//! bound.

#![no_main]

use std::time::Duration;

use helikodu_meta::lyrics::lrc::{MAX_LINES, parse_lrc};
use helikodu_meta::lyrics::{EmbeddedKind, Lyrics, LyricsSource};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let parsed = parse_lrc(&text);
    assert!(parsed.timed.len() + parsed.plain.len() <= MAX_LINES);
    for pair in parsed.timed.windows(2) {
        assert!(pair[0].time <= pair[1].time);
    }
    for line in parsed.timed.iter().chain(&parsed.plain) {
        for word in &line.words {
            assert!(word.start <= line.text.len());
            assert!(line.text.is_char_boundary(word.start));
        }
    }
    if let Some(lyrics) = Lyrics::from_text(&text, LyricsSource::Embedded(EmbeddedKind::Other)) {
        for ms in [0u64, 1_000, 60_000, u64::from(u32::MAX)] {
            let position = Duration::from_millis(ms);
            if let Some(index) = lyrics.line_at(position, 0) {
                assert!(index < lyrics.lines.len());
            }
            let _ = lyrics.next_change(position, i64::MIN);
            let _ = lyrics.line_at(position, i64::MAX);
        }
    }
});
