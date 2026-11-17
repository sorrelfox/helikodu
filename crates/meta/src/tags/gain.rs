// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parsing of ReplayGain and Opus R128 tag values.
//!
//! The engine parses the same values for files it plays without a library entry;
//! both accept the same syntax.

/// A `REPLAYGAIN_*_GAIN` value such as `-6.30 dB`, `+1.2 dB` or `-3.5`.
pub(crate) fn parse_gain_db(text: &str) -> Option<f64> {
    let text = text.trim();
    let number = text
        .strip_suffix("dB")
        .or_else(|| text.strip_suffix("db"))
        .or_else(|| text.strip_suffix("DB"))
        .unwrap_or(text)
        .trim();
    let value: f64 = number.strip_prefix('+').unwrap_or(number).parse().ok()?;
    // Real gains stay well inside ±100 dB; anything else is a broken tag.
    (value.is_finite() && value.abs() <= 100.0).then_some(value)
}

/// A `REPLAYGAIN_*_PEAK` value: a linear peak such as `0.988525`.
pub(crate) fn parse_peak(text: &str) -> Option<f64> {
    let value: f64 = text.trim().parse().ok()?;
    (value.is_finite() && value > 0.0 && value < 100.0).then_some(value)
}

/// An Opus `R128_*_GAIN` value: a signed decimal integer in Q7.8.
pub(crate) fn parse_r128(text: &str) -> Option<i16> {
    let text = text.trim();
    text.strip_prefix('+').unwrap_or(text).parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gains() {
        assert_eq!(parse_gain_db("-6.30 dB"), Some(-6.3));
        assert_eq!(parse_gain_db("+1.2 dB"), Some(1.2));
        assert_eq!(parse_gain_db(" -3.5 "), Some(-3.5));
        assert_eq!(parse_gain_db("2 DB"), Some(2.0));
        assert_eq!(parse_gain_db("loud"), None);
        assert_eq!(parse_gain_db("-500 dB"), None);
        assert_eq!(parse_gain_db("inf"), None);
    }

    #[test]
    fn peaks() {
        assert_eq!(parse_peak("0.988525"), Some(0.988525));
        assert_eq!(parse_peak("1.2"), Some(1.2));
        assert_eq!(parse_peak("0"), None);
        assert_eq!(parse_peak("-1"), None);
    }

    #[test]
    fn r128() {
        assert_eq!(parse_r128("-1234"), Some(-1234));
        assert_eq!(parse_r128("+256"), Some(256));
        assert_eq!(parse_r128("40000"), None);
        assert_eq!(parse_r128("1.5"), None);
    }
}
