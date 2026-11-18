// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Quick filters: predefined queries a front end offers with one click.

use serde::{Deserialize, Serialize};

use super::expr::{Expr, Flag, Operator, Presence, Query, SortKey};
use super::field::Field;
use super::value::{Age, AgeUnit, Decimal, Value};

/// A predefined query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QuickFilter {
    /// Never played: `plays=0`.
    Unplayed,
    /// Rated four stars or more: `rating>=4`.
    HighlyRated,
    /// Not rated: `-has:rating`.
    Unrated,
    /// Added in the last 30 days, newest first: `added<30d sort:added desc`.
    RecentlyAdded,
    /// Played in the last 14 days, last played first:
    /// `lastplayed<14d sort:lastplayed desc`.
    RecentlyPlayed,
    /// Not played for a year, but played before: `lastplayed>1y`.
    Forgotten,
    /// In a lossless format: `is:lossless`.
    Lossless,
    /// Lossless above 48 kHz or 16 bits: `is:hires`.
    HiRes,
    /// Marked as loved: `is:loved`.
    Loved,
    /// Audiobooks: `kind:audiobook`.
    Audiobooks,
}

impl QuickFilter {
    /// Every quick filter, in the order a front end offers them.
    pub const ALL: [Self; 10] = [
        Self::Unplayed,
        Self::HighlyRated,
        Self::Unrated,
        Self::RecentlyAdded,
        Self::RecentlyPlayed,
        Self::Forgotten,
        Self::Lossless,
        Self::HiRes,
        Self::Loved,
        Self::Audiobooks,
    ];

    /// A stable name, for settings and the command line.
    pub fn name(self) -> &'static str {
        match self {
            Self::Unplayed => "unplayed",
            Self::HighlyRated => "highly-rated",
            Self::Unrated => "unrated",
            Self::RecentlyAdded => "recently-added",
            Self::RecentlyPlayed => "recently-played",
            Self::Forgotten => "forgotten",
            Self::Lossless => "lossless",
            Self::HiRes => "hi-res",
            Self::Loved => "loved",
            Self::Audiobooks => "audiobooks",
        }
    }

    /// The quick filter named `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|filter| filter.name() == name)
    }

    /// The query.
    pub fn query(self) -> Query {
        let age = |amount, unit| Value::Age(Age { amount, unit });
        match self {
            Self::Unplayed => Query::new(Expr::compare(
                Field::Plays,
                Operator::Equals,
                Value::number(0),
            )),
            Self::HighlyRated => Query::new(Expr::compare(
                Field::Rating,
                Operator::GreaterOrEqual,
                Value::Number(Decimal::from_int(4)),
            )),
            Self::Unrated => Query::new(Expr::Has(Presence::Field(Field::Rating)).negate()),
            Self::RecentlyAdded => Query::new(Expr::compare(
                Field::Added,
                Operator::Less,
                age(30, AgeUnit::Days),
            ))
            .sorted_by(SortKey::descending(Field::Added)),
            Self::RecentlyPlayed => Query::new(Expr::compare(
                Field::LastPlayed,
                Operator::Less,
                age(14, AgeUnit::Days),
            ))
            .sorted_by(SortKey::descending(Field::LastPlayed)),
            Self::Forgotten => Query::new(Expr::compare(
                Field::LastPlayed,
                Operator::Greater,
                age(1, AgeUnit::Years),
            )),
            Self::Lossless => Query::new(Expr::Is(Flag::Lossless)),
            Self::HiRes => Query::new(Expr::Is(Flag::HiRes)),
            Self::Loved => Query::new(Expr::Is(Flag::Loved)),
            Self::Audiobooks => Query::new(Expr::compare(
                Field::Kind,
                Operator::Contains,
                Value::Kind(super::MediaKind::Audiobook),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_filters_are_valid_and_named() {
        for filter in QuickFilter::ALL {
            let query = filter.query();
            assert_eq!(query.problem(), None, "{filter:?}");
            assert_eq!(query.clone().normalized(), query);
            assert_eq!(QuickFilter::from_name(filter.name()), Some(filter));
        }
    }
}
