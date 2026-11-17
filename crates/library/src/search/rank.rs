// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! How well a field matches a query, for ordering results.

/// How a search result matched, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchRank {
    /// The title, an artist or the album is the query.
    Exact,
    /// The title, an artist or the album starts with the query.
    Prefix,
    /// A word inside the title, an artist or the album starts with the query.
    WordPrefix,
    /// The query is inside the title, an artist or the album.
    Substring,
    /// Each term of the query was found, but not the query as a whole in the title, an
    /// artist or the album.
    Terms,
}

impl MatchRank {
    /// The value of the `hk_match_rank()` SQL function.
    pub(crate) fn to_sql(self) -> i64 {
        match self {
            Self::Exact => 0,
            Self::Prefix => 1,
            Self::WordPrefix => 2,
            Self::Substring => 3,
            Self::Terms => 4,
        }
    }

    pub(crate) fn from_sql(value: i64) -> Self {
        match value {
            0 => Self::Exact,
            1 => Self::Prefix,
            2 => Self::WordPrefix,
            3 => Self::Substring,
            _ => Self::Terms,
        }
    }
}

/// How the folded `field` matches the folded `query`. A field holding several values
/// separated by U+001F (`tracks.artists_fold`) ranks as its best value.
pub(crate) fn match_rank(field: &str, query: &str) -> MatchRank {
    field
        .split('\u{1f}')
        .map(|value| match_value(value, query))
        .min()
        .unwrap_or(MatchRank::Terms)
}

fn match_value(field: &str, query: &str) -> MatchRank {
    if query.is_empty() {
        return MatchRank::Terms;
    }
    if field == query {
        return MatchRank::Exact;
    }
    if field.starts_with(query) {
        return MatchRank::Prefix;
    }
    let mut found = false;
    for (at, _) in field.match_indices(query) {
        found = true;
        let before = field[..at].chars().next_back();
        if before.is_some_and(|c| !c.is_alphanumeric()) {
            return MatchRank::WordPrefix;
        }
    }
    if found {
        MatchRank::Substring
    } else {
        MatchRank::Terms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks() {
        assert_eq!(match_rank("love", "love"), MatchRank::Exact);
        assert_eq!(match_rank("lovesong", "love"), MatchRank::Prefix);
        assert_eq!(match_rank("my love", "love"), MatchRank::WordPrefix);
        assert_eq!(match_rank("glove (love)", "love"), MatchRank::WordPrefix);
        assert_eq!(match_rank("glove", "love"), MatchRank::Substring);
        assert_eq!(match_rank("月亮代表我的心", "我的"), MatchRank::Substring);
        assert_eq!(match_rank("hate", "love"), MatchRank::Terms);
        assert_eq!(match_rank("band\u{1f}love", "love"), MatchRank::Exact);
        assert_eq!(match_rank("lovers\u{1f}glove", "love"), MatchRank::Prefix);
        assert!(MatchRank::Exact < MatchRank::Prefix);
        assert!(MatchRank::Substring < MatchRank::Terms);
    }
}
