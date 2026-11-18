// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The text forms of values: reading a value for a field, and writing it back.
//!
//! What a value means depends on its field: `30m` is 30 minutes of a length but 30
//! months of an age, and `96k` is 96000 Hz of a sample rate. Values are stored in the
//! tree as the user means them, and written back in a canonical spelling that reads
//! back to the same value.

use helikodu_model::query::{Age, CalendarDate, Decimal, Field, FieldKind, MediaKind, Value};

/// What a field takes, and an example, when a value does not fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Expected {
    pub(super) what: &'static str,
    pub(super) example: &'static str,
}

/// What `field` takes.
pub(super) fn expected(field: &Field) -> Expected {
    let (what, example) = match (field.kind(), field) {
        (FieldKind::Kind, _) => ("music, audiobook, podcast or video", "kind:audiobook"),
        (FieldKind::Rating, _) => ("0 to 5 stars in half stars", "rating>=4"),
        (FieldKind::Duration, _) => ("a length such as 30s, 4min or 3:30", "duration<30s"),
        (FieldKind::Size, _) => ("a size such as 700kB, 10MB or 1.5GB", "size>100MB"),
        (FieldKind::Date, _) => (
            "a date such as 2024-05-01 or an age such as 30d",
            "added<30d",
        ),
        (_, Field::SampleRate) => ("a rate in Hz or kHz", "samplerate>=88.2k"),
        (_, Field::Bitrate) => ("a bit rate in kbit/s", "bitrate>=320"),
        (_, Field::TrackGain | Field::AlbumGain) => ("a gain in dB", "trackgain<-6.5"),
        _ => ("a number", "year>=1970"),
    };
    Expected { what, example }
}

/// Reads `text` as a single value (not a range) of `field`. Text and regular
/// expressions are taken as they are.
pub(super) fn parse_single(field: &Field, text: &str) -> Option<Value> {
    match field.kind() {
        FieldKind::Text => Some(Value::Text(text.to_owned())),
        FieldKind::Kind => MediaKind::from_name(text).map(Value::Kind),
        FieldKind::Rating => text.parse::<Decimal>().ok().map(Value::Number),
        FieldKind::Number => parse_number(field, text).map(Value::Number),
        FieldKind::Duration => parse_duration(text).map(Value::Duration),
        FieldKind::Size => parse_size(text).map(Value::Size),
        FieldKind::Date => parse_date(text),
        _ => None,
    }
}

/// Writes `value` (not a range) of `field` so that [`parse_single`] reads it back.
pub(super) fn print_single(value: &Value) -> String {
    match value {
        Value::Text(text) => text.clone(),
        Value::Kind(kind) => kind.name().to_owned(),
        Value::Number(n) => n.to_string(),
        Value::Duration(ms) => print_duration(*ms),
        Value::Size(bytes) => print_size(*bytes),
        Value::Date(date) => date.to_string(),
        Value::Age(age) => age.to_string(),
        Value::Range(low, high) => format!("{}..{}", print_single(low), print_single(high)),
        _ => String::new(),
    }
}

/// Splits `text` into a number and the letters after it.
fn split_unit(text: &str) -> (&str, &str) {
    let end = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(text.len());
    (&text[..end], &text[end..])
}

/// `thousandths × factor / 1000` when it is a whole number that fits.
fn scale(n: Decimal, factor: i128) -> Option<i128> {
    let scaled = i128::from(n.thousandths()).checked_mul(factor)?;
    (scaled % 1000 == 0).then_some(scaled / 1000)
}

fn parse_number(field: &Field, text: &str) -> Option<Decimal> {
    let (number, unit) = split_unit(text);
    let n: Decimal = number.parse().ok()?;
    let factor = match (field, unit.to_ascii_lowercase().as_str()) {
        (_, "") => 1,
        (Field::SampleRate, "hz") => 1,
        (Field::SampleRate, "k" | "khz") => 1000,
        (Field::Bitrate, "k" | "kbps" | "kbit/s") => 1,
        (Field::TrackGain | Field::AlbumGain, "db") => 1,
        _ => return None,
    };
    let thousandths = i64::try_from(i128::from(n.thousandths()).checked_mul(factor)?).ok()?;
    Some(Decimal::from_thousandths(thousandths))
}

/// `30`, `30s`, `1500ms`, `4m`, `4min`, `2h`, `3:30`, `1:02:03` → milliseconds.
pub(super) fn parse_duration(text: &str) -> Option<u64> {
    if text.contains(':') {
        let parts: Vec<&str> = text.split(':').collect();
        if !(2..=3).contains(&parts.len())
            || parts
                .iter()
                .any(|p| p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit()))
        {
            return None;
        }
        let mut seconds: u64 = 0;
        for part in parts {
            seconds = seconds.checked_mul(60)?.checked_add(part.parse().ok()?)?;
        }
        return seconds.checked_mul(1000);
    }
    let (number, unit) = split_unit(text);
    if number.starts_with(['-', '+']) {
        return None;
    }
    let n: Decimal = number.parse().ok()?;
    // A number of seconds in thousandths is a number of milliseconds.
    let ms = match unit.to_ascii_lowercase().as_str() {
        "" | "s" | "sec" | "secs" => scale(n, 1000)?,
        "ms" => scale(n, 1)?,
        "m" | "min" | "mins" => scale(n, 60_000)?,
        "h" | "hr" | "hrs" => scale(n, 3_600_000)?,
        _ => return None,
    };
    u64::try_from(ms).ok()
}

fn print_duration(ms: u64) -> String {
    match ms {
        0 => "0s".to_owned(),
        ms if ms % 3_600_000 == 0 => format!("{}h", ms / 3_600_000),
        ms if ms % 60_000 == 0 => format!("{}min", ms / 60_000),
        ms if ms % 1000 == 0 => format!("{}s", ms / 1000),
        ms => format!("{ms}ms"),
    }
}

/// `123`, `700kB`, `10MB`, `1.5GB`, `2TB` (powers of 1000) → bytes.
pub(super) fn parse_size(text: &str) -> Option<u64> {
    let (number, unit) = split_unit(text);
    if number.starts_with(['-', '+']) {
        return None;
    }
    let n: Decimal = number.parse().ok()?;
    let factor: i128 = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1_000,
        "m" | "mb" => 1_000_000,
        "g" | "gb" => 1_000_000_000,
        "t" | "tb" => 1_000_000_000_000,
        _ => return None,
    };
    u64::try_from(scale(n, factor)?).ok()
}

fn print_size(bytes: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1_000_000_000_000, "TB"),
        (1_000_000_000, "GB"),
        (1_000_000, "MB"),
        (1_000, "kB"),
    ];
    UNITS
        .iter()
        .find(|(size, _)| bytes > 0 && bytes.is_multiple_of(*size))
        .map_or_else(
            || format!("{bytes}B"),
            |(size, unit)| format!("{}{unit}", bytes / size),
        )
}

/// A calendar date (`2024`, `2024-05`, `2024-05-01`) or an age (`30d`).
fn parse_date(text: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    let calendar = bytes.len() >= 4
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && (bytes.len() == 4 || bytes[4] == b'-');
    if calendar {
        text.parse::<CalendarDate>().ok().map(Value::Date)
    } else {
        text.parse::<Age>().ok().map(Value::Age)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helikodu_model::query::AgeUnit;

    #[test]
    fn durations_read_every_spelling() {
        for (text, ms) in [
            ("30", 30_000),
            ("30s", 30_000),
            ("1500ms", 1500),
            ("4m", 240_000),
            ("4min", 240_000),
            ("2.5min", 150_000),
            ("2h", 7_200_000),
            ("3:30", 210_000),
            ("1:02:03", 3_723_000),
            ("0.5s", 500),
        ] {
            assert_eq!(parse_duration(text), Some(ms), "{text}");
            assert_eq!(parse_duration(&print_duration(ms)), Some(ms), "{text}");
        }
        for bad in ["", "s", "-3s", "3x", "1:2:3:4", "1::2", "0.0005s", "3:x"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
        assert_eq!(print_duration(210_000), "210s");
        assert_eq!(print_duration(240_000), "4min");
    }

    #[test]
    fn sizes_use_powers_of_a_thousand() {
        assert_eq!(parse_size("10MB"), Some(10_000_000));
        assert_eq!(parse_size("1.5gb"), Some(1_500_000_000));
        assert_eq!(parse_size("123"), Some(123));
        assert_eq!(parse_size("700kB"), Some(700_000));
        assert_eq!(parse_size("1.0005kb"), None);
        assert_eq!(parse_size("-1"), None);
        for bytes in [0, 1, 999, 1000, 1_500_000, 4_000_000_000, 7_000_000_000_000] {
            assert_eq!(parse_size(&print_size(bytes)), Some(bytes));
        }
    }

    #[test]
    fn numbers_take_their_field_units() {
        let n = |field: &Field, text| parse_number(field, text).map(Decimal::thousandths);
        assert_eq!(n(&Field::SampleRate, "96k"), Some(96_000_000));
        assert_eq!(n(&Field::SampleRate, "88.2kHz"), Some(88_200_000));
        assert_eq!(n(&Field::SampleRate, "44100"), Some(44_100_000));
        assert_eq!(n(&Field::Bitrate, "320kbps"), Some(320_000));
        assert_eq!(n(&Field::TrackGain, "-6.5dB"), Some(-6500));
        assert_eq!(n(&Field::Year, "1975"), Some(1_975_000));
        assert_eq!(n(&Field::Year, "1975k"), None);
    }

    #[test]
    fn dates_and_ages_are_told_apart() {
        assert_eq!(
            parse_date("30d"),
            Some(Value::Age(Age {
                amount: 30,
                unit: AgeUnit::Days
            }))
        );
        assert_eq!(
            parse_date("2024-05"),
            Some(Value::Date(CalendarDate::month(2024, 5).unwrap()))
        );
        assert_eq!(parse_date("2024x"), None);
        assert_eq!(parse_date("30"), None);
    }
}
