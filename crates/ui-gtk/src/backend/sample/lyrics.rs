// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lyrics for the sample player: synced lyrics from a `.lrc` file for odd
//! track numbers, plain lyrics from the tags for track 2 and other even
//! numbers, and none for every fourth track.

use std::time::Duration;

use super::super::{LyricRow, LyricsOrigin, LyricsView};

/// The verses of the sample song, with the second each line starts.
const LINES: [(u64, &str); 16] = [
    (46, "The tide comes in on quiet feet"),
    (58, "Grey light folding over the bay"),
    (71, "I count the boats, I count the gulls"),
    (85, "Nobody is awake but the sea"),
    (99, "Salt on the window, salt on my hands"),
    (113, "The kettle hums in a lower key"),
    (127, "Shoreline, six a.m."),
    (141, "Hold the morning still for me"),
    (158, "The tide goes out on quiet feet"),
    (172, "And leaves the day for me to keep"),
    (186, "A door, a coat, the gravel road"),
    (200, "The lighthouse blinks itself to sleep"),
    (214, "Shoreline, six a.m."),
    (228, "Hold the morning still for me"),
    (242, "Hold the morning still"),
    (260, ""),
];

/// The lyrics of the sample track numbered `number`, with `offset_ms`.
pub(super) fn sample_lyrics(number: Option<u32>, offset_ms: i64) -> Option<LyricsView> {
    let number = number.unwrap_or(1);
    if number.is_multiple_of(4) {
        return None;
    }
    let synced = number % 2 == 1;
    let lines = LINES
        .iter()
        .enumerate()
        .filter(|(_, (_, text))| !text.is_empty())
        .map(|(i, (second, text))| LyricRow {
            time: synced.then(|| Duration::from_secs(*second)),
            text: (*text).to_owned(),
            verse_start: matches!(i, 4 | 8 | 12),
        })
        .collect();
    Some(LyricsView {
        lines,
        synced,
        origin: if synced {
            LyricsOrigin::LrcFile
        } else {
            LyricsOrigin::Tags
        },
        file: synced.then(|| "/home/user/Music/Mari Kask/Tidewater Rooms/03 Shoreline.lrc".into()),
        offset_ms,
    })
}

/// The line of `lyrics` sung at `elapsed`.
pub(super) fn line_at(lyrics: &LyricsView, elapsed: Duration) -> Option<usize> {
    if !lyrics.synced {
        return None;
    }
    let ms = i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX);
    let at = Duration::from_millis(u64::try_from(ms.saturating_add(lyrics.offset_ms)).unwrap_or(0));
    lyrics
        .lines
        .partition_point(|l| l.time.is_some_and(|t| t <= at))
        .checked_sub(1)
}
