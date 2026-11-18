// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The values a comparison compares a field with.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::text_form::ParseError;

/// The value side of a comparison.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Value {
    /// Text, for text fields and regular expressions.
    Text(String),
    /// A media kind, for [`Field::Kind`](super::Field::Kind).
    Kind(MediaKind),
    /// A number in the field's own unit (stars for ratings, kbit/s for bit rates).
    Number(Decimal),
    /// A length of time in milliseconds.
    Duration(u64),
    /// A number of bytes.
    Size(u64),
    /// A calendar year, month or day, in local time.
    Date(CalendarDate),
    /// A time before now: `30d` is 30 days ago.
    Age(Age),
    /// Both ends of an inclusive range of values of the same kind.
    Range(Box<Value>, Box<Value>),
}

impl Value {
    /// A range from `low` to `high`.
    pub fn range(low: Value, high: Value) -> Self {
        Self::Range(Box::new(low), Box::new(high))
    }

    /// A whole number.
    pub fn number(n: i64) -> Self {
        Self::Number(Decimal::from_int(n))
    }
}

/// What a track is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MediaKind {
    /// Music.
    Music,
    /// An audiobook.
    Audiobook,
    /// A podcast episode.
    Podcast,
    /// The audio of a video file.
    Video,
}

impl MediaKind {
    /// Every kind.
    pub const ALL: [Self; 4] = [Self::Music, Self::Audiobook, Self::Podcast, Self::Video];

    /// The name in the text form.
    pub fn name(self) -> &'static str {
        match self {
            Self::Music => "music",
            Self::Audiobook => "audiobook",
            Self::Podcast => "podcast",
            Self::Video => "video",
        }
    }

    /// The kind named `name`, in any case.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.name().eq_ignore_ascii_case(name))
    }
}

/// A decimal number with up to three digits after the point, kept exactly as
/// thousandths so that it prints back as it was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Decimal(i64);

impl Decimal {
    /// The number of thousandths in one.
    pub const SCALE: i64 = 1000;

    /// The number `thousandths / 1000`.
    pub const fn from_thousandths(thousandths: i64) -> Self {
        Self(thousandths)
    }

    /// A whole number; saturates at the limits.
    pub const fn from_int(n: i64) -> Self {
        Self(n.saturating_mul(Self::SCALE))
    }

    /// The number in thousandths.
    pub const fn thousandths(self) -> i64 {
        self.0
    }

    /// The number if it is whole.
    pub const fn as_int(self) -> Option<i64> {
        if self.0 % Self::SCALE == 0 {
            Some(self.0 / Self::SCALE)
        } else {
            None
        }
    }

    /// The number as a float.
    pub fn as_f64(self) -> f64 {
        self.0 as f64 / Self::SCALE as f64
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        let scale = Self::SCALE.unsigned_abs();
        let (whole, frac) = (abs / scale, abs % scale);
        if frac == 0 {
            write!(f, "{sign}{whole}")
        } else {
            let digits = format!("{frac:03}");
            write!(f, "{sign}{whole}.{}", digits.trim_end_matches('0'))
        }
    }
}

impl FromStr for Decimal {
    type Err = ParseError;

    /// `12`, `-3.5`, `0.125`: at most three digits after the point.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let error = || ParseError::new("number", s);
        let (negative, digits) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s.strip_prefix('+').unwrap_or(s)),
        };
        let (whole, frac) = digits.split_once('.').unwrap_or((digits, ""));
        let all_digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
        if (whole.is_empty() && frac.is_empty())
            || !all_digits(whole)
            || !all_digits(frac)
            || frac.len() > 3
            || (digits.contains('.') && frac.is_empty())
        {
            return Err(error());
        }
        let whole: i64 = if whole.is_empty() {
            0
        } else {
            whole.parse().map_err(|_| error())?
        };
        let frac: i64 = format!("{frac:0<3}").parse().map_err(|_| error())?;
        let value = whole
            .checked_mul(Self::SCALE)
            .and_then(|w| w.checked_add(frac))
            .ok_or_else(error)?;
        Ok(Self(if negative { -value } else { value }))
    }
}

/// A calendar year, a month of a year, or a day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CalendarDate {
    year: u16,
    month: Option<u8>,
    day: Option<u8>,
}

impl CalendarDate {
    /// A whole year (1 to 9999).
    pub fn year(year: u16) -> Option<Self> {
        Self::new(year, None, None)
    }

    /// A month of a year.
    pub fn month(year: u16, month: u8) -> Option<Self> {
        Self::new(year, Some(month), None)
    }

    /// A day.
    pub fn day(year: u16, month: u8, day: u8) -> Option<Self> {
        Self::new(year, Some(month), Some(day))
    }

    fn new(year: u16, month: Option<u8>, day: Option<u8>) -> Option<Self> {
        let valid = (1..=9999).contains(&year)
            && month.is_none_or(|m| (1..=12).contains(&m))
            && match (month, day) {
                (_, None) => true,
                (Some(m), Some(d)) => d >= 1 && d <= days_in_month(year, m),
                (None, Some(_)) => false,
            };
        valid.then_some(Self { year, month, day })
    }

    /// The year.
    pub fn year_number(self) -> u16 {
        self.year
    }

    /// The month (1 to 12), if the date names one.
    pub fn month_number(self) -> Option<u8> {
        self.month
    }

    /// The day of the month, if the date names one.
    pub fn day_number(self) -> Option<u8> {
        self.day
    }
}

/// The number of days in `month` of `year` in the proleptic Gregorian calendar.
pub(crate) fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 31,
    }
}

impl fmt::Display for CalendarDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}", self.year)?;
        if let Some(month) = self.month {
            write!(f, "-{month:02}")?;
        }
        if let Some(day) = self.day {
            write!(f, "-{day:02}")?;
        }
        Ok(())
    }
}

impl FromStr for CalendarDate {
    type Err = ParseError;

    /// `2024`, `2024-05` or `2024-05-01`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let error = || ParseError::new("date", s);
        let mut parts = s.split('-');
        let mut number = |len: std::ops::RangeInclusive<usize>| -> Result<Option<u16>, ParseError> {
            match parts.next() {
                None => Ok(None),
                Some(part)
                    if len.contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit()) =>
                {
                    part.parse().map(Some).map_err(|_| error())
                }
                Some(_) => Err(error()),
            }
        };
        let year = number(4..=4)?.ok_or_else(error)?;
        let month = number(1..=2)?;
        let day = number(1..=2)?;
        if parts.next().is_some() {
            return Err(error());
        }
        let narrow = |n: Option<u16>| n.map(|n| u8::try_from(n).map_err(|_| error())).transpose();
        Self::new(year, narrow(month)?, narrow(day)?).ok_or_else(error)
    }
}

/// How long ago: an amount of a unit of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Age {
    /// The number of units.
    pub amount: u32,
    /// The unit.
    pub unit: AgeUnit,
}

/// The unit of an [`Age`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgeUnit {
    /// Hours (`h`).
    Hours,
    /// Days (`d`).
    Days,
    /// Weeks (`w`).
    Weeks,
    /// Calendar months (`m`).
    Months,
    /// Calendar years (`y`).
    Years,
}

impl AgeUnit {
    /// The suffix in the text form.
    pub fn suffix(self) -> char {
        match self {
            Self::Hours => 'h',
            Self::Days => 'd',
            Self::Weeks => 'w',
            Self::Months => 'm',
            Self::Years => 'y',
        }
    }

    /// The unit with `suffix`, in any case.
    pub fn from_suffix(suffix: char) -> Option<Self> {
        match suffix.to_ascii_lowercase() {
            'h' => Some(Self::Hours),
            'd' => Some(Self::Days),
            'w' => Some(Self::Weeks),
            'm' => Some(Self::Months),
            'y' => Some(Self::Years),
            _ => None,
        }
    }
}

impl fmt::Display for Age {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.amount, self.unit.suffix())
    }
}

impl FromStr for Age {
    type Err = ParseError;

    /// `30d`, `2w`, `6m`, `1y`, `12h`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let error = || ParseError::new("age", s);
        let unit = s
            .chars()
            .last()
            .and_then(AgeUnit::from_suffix)
            .ok_or_else(error)?;
        let digits = &s[..s.len() - 1];
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(error());
        }
        let amount = digits.parse().map_err(|_| error())?;
        Ok(Self { amount, unit })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_print_as_written() {
        for text in ["0", "12", "-3.5", "0.125", "4.1", "-0.001"] {
            let n: Decimal = text.parse().unwrap();
            assert_eq!(n.to_string(), text);
        }
        assert_eq!("1.50".parse::<Decimal>().unwrap().to_string(), "1.5");
        assert_eq!(
            ".5".parse::<Decimal>().unwrap(),
            Decimal::from_thousandths(500)
        );
        for bad in [
            "",
            "-",
            "1.",
            "1.2345",
            "1e3",
            "x",
            "1..2",
            "99999999999999999999",
        ] {
            assert!(bad.parse::<Decimal>().is_err(), "{bad}");
        }
        assert_eq!(Decimal::from_int(4).as_int(), Some(4));
        assert_eq!(Decimal::from_thousandths(4500).as_int(), None);
    }

    #[test]
    fn dates_are_checked_and_print_back() {
        for text in ["2024", "2024-05", "2024-02-29", "0001-01-01"] {
            assert_eq!(text.parse::<CalendarDate>().unwrap().to_string(), text);
        }
        assert_eq!(
            "2024-5-1".parse::<CalendarDate>().unwrap().to_string(),
            "2024-05-01"
        );
        for bad in [
            "2023-02-29",
            "2024-13",
            "24",
            "2024-05-01-01",
            "2024--01",
            "0000",
            "x",
        ] {
            assert!(bad.parse::<CalendarDate>().is_err(), "{bad}");
        }
    }

    #[test]
    fn ages_parse_their_units() {
        let age: Age = "30d".parse().unwrap();
        assert_eq!(
            age,
            Age {
                amount: 30,
                unit: AgeUnit::Days
            }
        );
        assert_eq!(age.to_string(), "30d");
        assert_eq!("2W".parse::<Age>().unwrap().to_string(), "2w");
        for bad in ["d", "30", "30x", "-3d", "3.5d"] {
            assert!(bad.parse::<Age>().is_err(), "{bad}");
        }
    }

    #[test]
    fn kinds_have_names() {
        for kind in MediaKind::ALL {
            assert_eq!(MediaKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(
            MediaKind::from_name("AUDIOBOOK"),
            Some(MediaKind::Audiobook)
        );
        assert_eq!(MediaKind::from_name("film"), None);
    }
}
