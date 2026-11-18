// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a file says about itself and its subsongs.

use std::time::Duration;

/// A file of a synthesized format: its text and its subsongs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SongInfo {
    /// The format, such as `Impulse Tracker`, `Nintendo NES` or `PSID`.
    pub format: String,
    /// The title of the file (a module's song name, a SID tune's title).
    pub title: Option<String>,
    /// The composer.
    pub artist: Option<String>,
    /// The game or collection the music is from.
    pub album: Option<String>,
    /// A date or a copyright line, as the file gives it (`1987 Hubbard`).
    pub date: Option<String>,
    /// A comment (a module's message, a dumper's note).
    pub comment: Option<String>,
    /// The subsongs: at least one.
    pub subsongs: Vec<SubsongInfo>,
    /// The subsong played when none is chosen, counted from 0.
    pub default_subsong: u32,
}

/// One subsong.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubsongInfo {
    /// The subsong's index, counted from 0.
    pub index: u32,
    /// Its own title, when it has one.
    pub title: Option<String>,
    /// How long it plays with the current options, fade included, when known.
    pub length: Option<Duration>,
}

impl SongInfo {
    /// The number of subsongs.
    pub fn subsong_count(&self) -> u32 {
        u32::try_from(self.subsongs.len()).unwrap_or(u32::MAX)
    }
}

/// Text from a library: trimmed, `None` when empty.
pub(crate) fn text(value: &str) -> Option<String> {
    let value = value.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_none() {
        assert_eq!(text("  \0"), None);
        assert_eq!(text(" Commando "), Some("Commando".into()));
    }
}
