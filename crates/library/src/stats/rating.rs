// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Ratings in half stars.

use std::fmt;

/// A rating from zero to five stars in half stars.
///
/// "Not rated" is `Option::<Rating>::None`, which is different from zero stars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rating(u8);

impl Rating {
    /// The most half stars: five stars.
    pub const MAX_HALF_STARS: u8 = 10;

    /// The rating of `half_stars` half stars; `None` above [`MAX_HALF_STARS`](Self::MAX_HALF_STARS).
    pub const fn from_half_stars(half_stars: u8) -> Option<Self> {
        if half_stars <= Self::MAX_HALF_STARS {
            Some(Self(half_stars))
        } else {
            None
        }
    }

    /// The rating of `stars` whole stars; `None` above five.
    pub const fn from_stars(stars: u8) -> Option<Self> {
        if stars <= Self::MAX_HALF_STARS / 2 {
            Some(Self(stars * 2))
        } else {
            None
        }
    }

    /// The number of half stars, 0 to 10.
    pub const fn half_stars(self) -> u8 {
        self.0
    }

    /// The number of whole stars, rounding a half star up.
    pub const fn stars_rounded(self) -> u8 {
        self.0.div_ceil(2)
    }
}

impl fmt::Display for Rating {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / 2;
        if self.0.is_multiple_of(2) {
            write!(f, "{whole}")
        } else {
            write!(f, "{whole}.5")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range() {
        assert_eq!(Rating::from_half_stars(0).map(Rating::half_stars), Some(0));
        assert_eq!(
            Rating::from_half_stars(10).map(Rating::half_stars),
            Some(10)
        );
        assert_eq!(Rating::from_half_stars(11), None);
        assert_eq!(Rating::from_stars(5), Rating::from_half_stars(10));
        assert_eq!(Rating::from_stars(6), None);
    }

    #[test]
    fn display_and_rounding() {
        let three_and_a_half = Rating::from_half_stars(7).unwrap();
        assert_eq!(three_and_a_half.to_string(), "3.5");
        assert_eq!(three_and_a_half.stars_rounded(), 4);
        assert_eq!(Rating::from_stars(2).unwrap().to_string(), "2");
    }
}
