// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tags as FFmpeg reports them: key and value pairs, in file order.

/// Tags as FFmpeg reports them: key and value pairs in file order, with FFmpeg's
/// key names (`title`, `artist`, `album`, `track`, `date`, `REPLAYGAIN_TRACK_GAIN`
/// and whatever else the file carries). A key may appear more than once. Values that
/// are not valid UTF-8 have the invalid bytes replaced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tags {
    entries: Vec<(String, String)>,
}

impl Tags {
    pub(crate) fn from_entries(entries: Vec<(String, String)>) -> Self {
        Self { entries }
    }

    /// The first value of `key`, compared without regard to ASCII case.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// Every value of `key`, compared without regard to ASCII case.
    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> {
        self.entries
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// Every entry, in file order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Appends the entries of `other`.
    pub(crate) fn extend(&mut self, other: &Self) {
        self.entries.extend(other.entries.iter().cloned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_ignores_case_and_keeps_duplicates() {
        let tags = Tags::from_entries(vec![
            ("ARTIST".into(), "A".into()),
            ("artist".into(), "B".into()),
            ("title".into(), "T".into()),
        ]);
        assert_eq!(tags.get("Artist"), Some("A"));
        assert_eq!(tags.get_all("artist").collect::<Vec<_>>(), ["A", "B"]);
        assert_eq!(tags.get("album"), None);
        assert_eq!(tags.len(), 3);
    }
}
