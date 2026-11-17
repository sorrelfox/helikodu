// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Ratings stored in tags, read onto the library's half-star scale.
//!
//! | Tag | Scale | Read as |
//! |---|---|---|
//! | ID3v2 `POPM` | 0–255; 0 = not rated | By range: 1–31 → 1 star, 32–95 → 2, 96–159 → 3, 160–223 → 4, 224–255 → 5. Linear (`value / 255 × 5`, to the half star) for the players known to write it linearly. MusicBee's half-star values are recognised |
//! | `FMPS_RATING` (Vorbis comment, APE, `TXXX`) | 0.0–1.0 | × 10, rounded to the half star |
//! | `RATING` (Vorbis comment, APE, MP4 `rate`) | 0–5 or 0–100, told apart by range | Stars, or ÷ 10. 0 = not rated |
//! | ASF `WM/SharedUserRating` | 0–99; 0 = not rated | Windows Media Player's ranges: 1–12 → 1 star, 13–37 → 2, 38–62 → 3, 63–86 → 4, 87–99 → 5 |

use std::fmt;

/// A rating from 0 to 10 in half stars: 7 is three and a half stars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rating(u8);

impl Rating {
    /// The highest rating: five stars.
    pub const MAX: Rating = Rating(10);

    /// A rating of `half_stars` (0 to 10), or `None` when it is out of range.
    pub fn from_half_stars(half_stars: u8) -> Option<Self> {
        (half_stars <= 10).then_some(Self(half_stars))
    }

    /// The rating in half stars, 0 to 10.
    pub fn half_stars(self) -> u8 {
        self.0
    }

    /// A fraction of five stars, rounded to the nearest half star.
    fn from_fraction(fraction: f64) -> Option<Self> {
        if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
            return None;
        }
        Self::from_half_stars((fraction * 10.0).round() as u8)
    }
}

impl fmt::Display for Rating {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0 / 2)?;
        if self.0 % 2 == 1 {
            f.write_str(".5")?;
        }
        f.write_str("/5")
    }
}

/// The player that wrote a `POPM` frame, recognised by its e-mail field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PopmWriter {
    /// Writes 0.5 to 5 stars as 13, 1, 54, 64, 118, 128, 186, 196, 242, 255.
    MusicBee,
    /// Writes `stars / 5 × 255` (Quod Libet, MusicBrainz Picard).
    Linear,
    /// Windows Media Player, foobar2000, MediaMonkey, Winamp, Strawberry and
    /// every unknown writer: the range table.
    Ranges,
}

fn popm_writer(email: &str) -> PopmWriter {
    let email = email.trim();
    if email.eq_ignore_ascii_case("MusicBee") {
        PopmWriter::MusicBee
    } else if email.eq_ignore_ascii_case("quodlibet@lists.sacredchao.net")
        || email.eq_ignore_ascii_case("users@musicbrainz.org")
    {
        PopmWriter::Linear
    } else {
        PopmWriter::Ranges
    }
}

/// A rating from an ID3v2 `POPM` frame, or `None` for 0 (not rated).
pub fn from_popm(email: &str, value: u8) -> Option<Rating> {
    if value == 0 {
        return None;
    }
    match popm_writer(email) {
        PopmWriter::Linear => Rating::from_fraction(f64::from(value) / 255.0),
        PopmWriter::MusicBee => {
            let half = match value {
                13 => Some(1),
                54 => Some(3),
                118 => Some(5),
                186 => Some(7),
                242 => Some(9),
                _ => None,
            };
            half.and_then(Rating::from_half_stars)
                .or_else(|| popm_ranges(value))
        }
        PopmWriter::Ranges => popm_ranges(value),
    }
}

fn popm_ranges(value: u8) -> Option<Rating> {
    let stars = match value {
        0 => return None,
        1..=31 => 1,
        32..=95 => 2,
        96..=159 => 3,
        160..=223 => 4,
        224..=255 => 5,
    };
    Rating::from_half_stars(stars * 2)
}

/// A rating from an `FMPS_RATING` value (0.0 to 1.0). 0.0 is a rating of no stars.
pub fn from_fmps(text: &str) -> Option<Rating> {
    let value: f64 = text.trim().parse().ok()?;
    Rating::from_fraction(value)
}

/// A rating from a `RATING` value: 0 to 5 stars (halves allowed), or 0 to 100.
/// 0 means not rated.
pub fn from_rating_field(text: &str) -> Option<Rating> {
    let value: f64 = text.trim().parse().ok()?;
    if !value.is_finite() || value <= 0.0 || value > 100.0 {
        return None;
    }
    if value <= 5.0 {
        Rating::from_fraction(value / 5.0)
    } else {
        Rating::from_fraction(value / 100.0)
    }
}

/// A rating from ASF `WM/SharedUserRating` (0 to 99). 0 means not rated.
pub fn from_wmp_shared(text: &str) -> Option<Rating> {
    let value: u32 = text.trim().parse().ok()?;
    let stars = match value {
        0 => return None,
        1..=12 => 1,
        13..=37 => 2,
        38..=62 => 3,
        63..=86 => 4,
        87..=99 => 5,
        _ => return None,
    };
    Rating::from_half_stars(stars * 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn half(r: Option<Rating>) -> Option<u8> {
        r.map(Rating::half_stars)
    }

    #[test]
    fn popm_ranges_follow_the_table() {
        let wmp = "Windows Media Player 9 Series";
        assert_eq!(half(from_popm(wmp, 0)), None);
        for (value, stars) in [
            (1, 2),
            (31, 2),
            (32, 4),
            (64, 4),
            (95, 4),
            (96, 6),
            (128, 6),
        ] {
            assert_eq!(half(from_popm(wmp, value)), Some(stars), "{value}");
        }
        for (value, stars) in [(159, 6), (160, 8), (196, 8), (223, 8), (224, 10), (255, 10)] {
            assert_eq!(half(from_popm(wmp, value)), Some(stars), "{value}");
        }
        assert_eq!(half(from_popm("", 196)), Some(8));
        assert_eq!(half(from_popm("no@email", 255)), Some(10));
    }

    #[test]
    fn linear_popm_writers() {
        let ql = "quodlibet@lists.sacredchao.net";
        assert_eq!(half(from_popm(ql, 255)), Some(10));
        assert_eq!(half(from_popm(ql, 127)), Some(5));
        assert_eq!(half(from_popm(ql, 51)), Some(2));
        assert_eq!(half(from_popm("users@musicbrainz.org", 51)), Some(2));
        assert_eq!(half(from_popm("users@musicbrainz.org", 153)), Some(6));
    }

    #[test]
    fn musicbee_half_stars() {
        let values = [13, 1, 54, 64, 118, 128, 186, 196, 242, 255];
        for (i, value) in values.into_iter().enumerate() {
            assert_eq!(
                half(from_popm("MusicBee", value)),
                Some(i as u8 + 1),
                "{value}"
            );
        }
        assert_eq!(half(from_popm("MusicBee", 100)), Some(6));
    }

    #[test]
    fn fmps_rating() {
        assert_eq!(half(from_fmps("1.0")), Some(10));
        assert_eq!(half(from_fmps("0.7")), Some(7));
        assert_eq!(half(from_fmps("0.66")), Some(7));
        assert_eq!(half(from_fmps("0")), Some(0));
        assert_eq!(half(from_fmps("1.5")), None);
        assert_eq!(half(from_fmps("-0.1")), None);
        assert_eq!(half(from_fmps("NaN")), None);
        assert_eq!(half(from_fmps("good")), None);
    }

    #[test]
    fn rating_field_scales() {
        assert_eq!(half(from_rating_field("5")), Some(10));
        assert_eq!(half(from_rating_field("3")), Some(6));
        assert_eq!(half(from_rating_field("3.5")), Some(7));
        assert_eq!(half(from_rating_field("80")), Some(8));
        assert_eq!(half(from_rating_field("100")), Some(10));
        assert_eq!(half(from_rating_field("50")), Some(5));
        assert_eq!(half(from_rating_field("0")), None);
        assert_eq!(half(from_rating_field("101")), None);
        assert_eq!(half(from_rating_field("x")), None);
    }

    #[test]
    fn wmp_shared_rating() {
        assert_eq!(half(from_wmp_shared("0")), None);
        assert_eq!(half(from_wmp_shared("1")), Some(2));
        assert_eq!(half(from_wmp_shared("25")), Some(4));
        assert_eq!(half(from_wmp_shared("50")), Some(6));
        assert_eq!(half(from_wmp_shared("75")), Some(8));
        assert_eq!(half(from_wmp_shared("99")), Some(10));
        assert_eq!(half(from_wmp_shared("100")), None);
    }

    #[test]
    fn display() {
        assert_eq!(
            Rating::from_half_stars(7).map(|r| r.to_string()),
            Some("3.5/5".into())
        );
        assert_eq!(Rating::MAX.to_string(), "5/5");
        assert_eq!(Rating::from_half_stars(11), None);
    }
}
