// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading and writing the EqualizerAPO `ParametricEQ.txt` syntax that AutoEQ
//! publishes:
//!
//! ```text
//! Preamp: -6.2 dB
//! Filter 1: ON LSC Fc 105 Hz Gain 5.5 dB Q 0.70
//! Filter 2: ON PK Fc 186 Hz Gain -3.1 dB Q 0.53
//! Filter 3: OFF HSC Fc 10000 Hz Gain -3.4 dB Q 0.70
//! ```
//!
//! Filter types `PK` (and `PEQ`), `LS`/`LSC`, `HS`/`HSC`, `LP`/`LPQ` and `HP`/`HPQ`
//! are read; a shelf's slope token (`LSC 12dB`) is accepted and its `Q` used, a
//! missing `Q` of a shelf or pass filter is 0.707, and `BW Oct` stands in for `Q`.
//! Several `Preamp` lines add up, as in EqualizerAPO. Lines this reader does not
//! use (`Channel:`, `Device:`, other filter types, …) are skipped with a warning,
//! and so are filters with missing or unreadable values. Values outside the ranges
//! of the settings are clamped into them, with a warning. At most
//! [`EqualizerSettings::MAX_FILTERS`] filters are read.

use std::fmt::{self, Write as _};

use helikodu_model::settings::{EqualizerSettings, FilterKind, ParametricFilter};
use thiserror::Error;

/// The quality factor of a shelf or pass filter without one.
const DEFAULT_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// The filters and preamp read from a file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutoEqImport {
    /// The preamp in dB, if the file has one.
    pub preamp_db: Option<f64>,
    /// The filters, in the file's order; those marked `OFF` are disabled.
    pub filters: Vec<ParametricFilter>,
    /// What was skipped or changed.
    pub warnings: Vec<AutoEqWarning>,
}

/// Something in the file that was skipped or changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoEqWarning {
    /// The line, counted from 1.
    pub line: usize,
    /// What happened, in English.
    pub message: String,
}

impl fmt::Display for AutoEqWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// Why a file could not be read as a parametric equalizer.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AutoEqError {
    /// No line was a filter this reader understands.
    #[error("the file has no parametric equalizer filters")]
    NoFilters,
}

/// Reads a file's bytes: UTF-8 with or without a byte-order mark; other bytes are
/// replaced.
pub fn parse_bytes(bytes: &[u8]) -> Result<AutoEqImport, AutoEqError> {
    parse(&String::from_utf8_lossy(bytes))
}

/// Reads the text of a `ParametricEQ.txt` file.
pub fn parse(text: &str) -> Result<AutoEqImport, AutoEqError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut result = AutoEqImport::default();
    let mut dropped = 0;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut warn = |message: String| {
            result.warnings.push(AutoEqWarning {
                line: number,
                message,
            });
        };
        let Some((key, rest)) = line.split_once(':') else {
            warn("not a setting; skipped".into());
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if key == "preamp" {
            match parse_preamp(rest) {
                Some(db) => {
                    let total = result.preamp_db.unwrap_or(0.0) + db;
                    result.preamp_db = Some(total);
                }
                None => warn("the preamp cannot be read; skipped".into()),
            }
        } else if key == "filter" || key.strip_prefix("filter").is_some_and(is_number) {
            match parse_filter(rest) {
                Ok((filter, notes)) => {
                    for note in notes {
                        warn(note);
                    }
                    if result.filters.len() < EqualizerSettings::MAX_FILTERS {
                        result.filters.push(filter);
                    } else {
                        dropped += 1;
                    }
                }
                Err(problem) => warn(format!("{problem}; skipped")),
            }
        } else {
            warn(format!("\"{}\" is not used; skipped", key_text(line)));
        }
    }
    if dropped > 0 {
        result.warnings.push(AutoEqWarning {
            line: 0,
            message: format!(
                "only the first {} filters are used; {dropped} more were skipped",
                EqualizerSettings::MAX_FILTERS
            ),
        });
    }
    if result.filters.is_empty() {
        return Err(AutoEqError::NoFilters);
    }
    Ok(result)
}

/// Writes filters and a preamp in the same syntax.
pub fn format(preamp_db: Option<f64>, filters: &[ParametricFilter]) -> String {
    let mut text = String::new();
    if let Some(db) = preamp_db {
        let _ = writeln!(text, "Preamp: {} dB", round(db, 100.0));
    }
    for (i, filter) in filters.iter().enumerate() {
        let state = if filter.enabled { "ON" } else { "OFF" };
        let kind = match filter.kind {
            FilterKind::Peak => "PK",
            FilterKind::LowShelf => "LSC",
            FilterKind::HighShelf => "HSC",
            FilterKind::LowPass => "LPQ",
            FilterKind::HighPass => "HPQ",
        };
        let _ = write!(
            text,
            "Filter {}: {state} {kind} Fc {} Hz",
            i + 1,
            round(filter.frequency_hz, 100.0)
        );
        if filter.kind.has_gain() {
            let _ = write!(text, " Gain {} dB", round(filter.gain_db, 100.0));
        }
        let _ = writeln!(text, " Q {}", round(filter.q, 1000.0));
    }
    text
}

fn round(value: f64, scale: f64) -> f64 {
    let rounded = (value * scale).round() / scale;
    // No "-0".
    if rounded == 0.0 { 0.0 } else { rounded }
}

fn is_number(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// The start of a line, for a warning.
fn key_text(line: &str) -> String {
    let key = line.split(':').next().unwrap_or(line).trim();
    key.chars().take(40).collect()
}

/// A number, with a comma accepted as the decimal separator.
fn number(token: &str) -> Option<f64> {
    let value: f64 = token.replace(',', ".").parse().ok()?;
    value.is_finite().then_some(value)
}

fn parse_preamp(rest: &str) -> Option<f64> {
    let mut tokens = rest.split_whitespace();
    let value = number(tokens.next()?)?;
    match tokens.next() {
        None => Some(value),
        Some(unit) if unit.eq_ignore_ascii_case("db") => Some(value),
        Some(_) => None,
    }
}

fn parse_filter(rest: &str) -> Result<(ParametricFilter, Vec<String>), String> {
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let mut at = 0;
    let enabled = match tokens.first().map(|t| t.to_ascii_uppercase()) {
        Some(state) if state == "ON" => true,
        Some(state) if state == "OFF" => false,
        _ => return Err("a filter must start with ON or OFF".into()),
    };
    at += 1;
    let kind_token = tokens
        .get(at)
        .map(|t| t.to_ascii_uppercase())
        .ok_or("the filter has no type")?;
    let kind = match kind_token.as_str() {
        "PK" | "PEQ" => FilterKind::Peak,
        "LS" | "LSC" => FilterKind::LowShelf,
        "HS" | "HSC" => FilterKind::HighShelf,
        "LP" | "LPQ" => FilterKind::LowPass,
        "HP" | "HPQ" => FilterKind::HighPass,
        "NONE" => return Err("the filter has no type".into()),
        other => return Err(format!("filter type {} is not supported", clip(other))),
    };
    at += 1;
    let (mut frequency, mut gain, mut q, mut bandwidth) = (None, None, None, None);
    while at < tokens.len() {
        let key = tokens[at].to_ascii_lowercase();
        let value = tokens.get(at + 1).copied();
        match key.as_str() {
            "fc" => {
                frequency = value.and_then(number);
                at += 2;
                if tokens.get(at).is_some_and(|t| t.eq_ignore_ascii_case("hz")) {
                    at += 1;
                }
            }
            "gain" => {
                gain = value.and_then(number);
                at += 2;
                if tokens.get(at).is_some_and(|t| t.eq_ignore_ascii_case("db")) {
                    at += 1;
                }
            }
            "q" => {
                q = value.and_then(number);
                at += 2;
            }
            "bw" => {
                // "BW Oct 1.0"
                let value = tokens.get(at + 2).copied();
                if tokens
                    .get(at + 1)
                    .is_some_and(|t| t.eq_ignore_ascii_case("oct"))
                {
                    bandwidth = value.and_then(number);
                }
                at += 3;
            }
            // A shelf's slope, such as "12dB", or a unit on its own.
            slope if slope.ends_with("db") && number(&slope[..slope.len() - 2]).is_some() => {
                at += 1;
            }
            _ => return Err(format!("\"{}\" cannot be read", clip(tokens[at]))),
        }
    }
    let frequency = frequency.ok_or("the filter has no frequency")?;
    let gain = match (gain, kind.has_gain()) {
        (Some(gain), true) => gain,
        (None, true) => return Err("the filter has no gain".into()),
        (_, false) => 0.0,
    };
    let q = match (q, bandwidth) {
        (Some(q), _) => q,
        (None, Some(octaves)) if octaves > 0.0 => {
            let ratio = 2f64.powf(octaves);
            ratio.sqrt() / (ratio - 1.0)
        }
        _ if kind == FilterKind::Peak => return Err("the filter has no Q".into()),
        _ => DEFAULT_Q,
    };
    if q <= 0.0 || frequency <= 0.0 {
        return Err("the filter's frequency and Q must be above zero".into());
    }
    let mut notes = Vec::new();
    let mut clamp = |name: &str, value: f64, range: std::ops::RangeInclusive<f64>| {
        let clamped = value.clamp(*range.start(), *range.end());
        if clamped != value {
            notes.push(format!("the {name} {value} was changed to {clamped}"));
        }
        clamped
    };
    let filter = ParametricFilter {
        kind,
        frequency_hz: clamp("frequency", frequency, ParametricFilter::FREQUENCY_RANGE),
        gain_db: clamp("gain", gain, ParametricFilter::GAIN_RANGE),
        q: clamp("Q", q, ParametricFilter::Q_RANGE),
        enabled,
    };
    Ok((filter, notes))
}

/// A token, shortened for a message.
fn clip(token: &str) -> String {
    token.chars().take(20).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_lines() {
        let import = parse(
            "Preamp: -6.2 dB\r\nFilter 1: ON PK Fc 105 Hz Gain -3.1 dB Q 0.70\r\n\
             Filter 2: OFF LSC Fc 80 Hz Gain 4 dB Q 0.71\r\n",
        )
        .unwrap();
        assert_eq!(import.preamp_db, Some(-6.2));
        assert_eq!(
            import.filters,
            [
                ParametricFilter {
                    kind: FilterKind::Peak,
                    frequency_hz: 105.0,
                    gain_db: -3.1,
                    q: 0.7,
                    enabled: true,
                },
                ParametricFilter {
                    kind: FilterKind::LowShelf,
                    frequency_hz: 80.0,
                    gain_db: 4.0,
                    q: 0.71,
                    enabled: false,
                },
            ]
        );
        assert!(import.warnings.is_empty());
    }

    #[test]
    fn variants_of_the_syntax() {
        let import = parse(
            "\u{feff}# comment\nFilter: ON HS 12dB Fc 8000 Hz Gain 2.5 dB\n\
             Filter 2: ON LP Fc 18000 Hz\nFilter 3: ON PK Fc 1000 Hz Gain 1,5 dB BW Oct 1\n\
             Preamp: -1 dB\nPreamp: -0.5 dB\n",
        )
        .unwrap();
        assert_eq!(import.preamp_db, Some(-1.5));
        assert_eq!(import.filters[0].kind, FilterKind::HighShelf);
        assert_eq!(import.filters[0].q, DEFAULT_Q);
        assert_eq!(import.filters[1].kind, FilterKind::LowPass);
        assert_eq!(import.filters[2].gain_db, 1.5);
        assert!((import.filters[2].q - std::f64::consts::SQRT_2).abs() < 1e-12);
    }

    #[test]
    fn problems_are_warnings() {
        let import = parse(
            "Device: Speakers\nFilter 1: ON BP Fc 100 Hz Q 1\nFilter 2: ON PK Fc 100 Hz Q 1\n\
             Filter 3: ON PK Fc 5 Hz Gain 40 dB Q 1\nFilter 4: ON PK Fc 300 Hz Gain 1 dB Q 1\n\
             just text\n",
        )
        .unwrap();
        assert_eq!(import.filters.len(), 2);
        assert_eq!(import.filters[0].frequency_hz, 10.0);
        assert_eq!(import.filters[0].gain_db, 30.0);
        let lines: Vec<usize> = import.warnings.iter().map(|w| w.line).collect();
        assert_eq!(lines, [1, 2, 3, 4, 4, 6]);
        assert_eq!(
            import.warnings[1].to_string(),
            "line 2: filter type BP is not supported; skipped"
        );
        assert_eq!(parse("Preamp: -3 dB\n"), Err(AutoEqError::NoFilters));
        assert_eq!(parse(""), Err(AutoEqError::NoFilters));
    }

    #[test]
    fn at_most_64_filters() {
        let text: String = (1..=70)
            .map(|i| format!("Filter {i}: ON PK Fc {} Hz Gain 1 dB Q 1\n", 100 + i))
            .collect();
        let import = parse(&text).unwrap();
        assert_eq!(import.filters.len(), 64);
        assert_eq!(import.warnings.len(), 1);
    }

    #[test]
    fn writing_and_reading_round_trip() {
        let filters = [
            ParametricFilter {
                kind: FilterKind::LowShelf,
                frequency_hz: 105.0,
                gain_db: 5.5,
                q: 0.7,
                enabled: true,
            },
            ParametricFilter {
                kind: FilterKind::HighPass,
                frequency_hz: 20.0,
                gain_db: 0.0,
                q: 0.5,
                enabled: false,
            },
        ];
        let text = format(Some(-5.5), &filters);
        assert_eq!(
            text,
            "Preamp: -5.5 dB\nFilter 1: ON LSC Fc 105 Hz Gain 5.5 dB Q 0.7\n\
             Filter 2: OFF HPQ Fc 20 Hz Q 0.5\n"
        );
        let import = parse(&text).unwrap();
        assert_eq!(import.preamp_db, Some(-5.5));
        assert_eq!(import.filters, filters);
    }

    #[test]
    fn scrambled_files_give_valid_filters_or_an_error() {
        let words = [
            "Filter",
            "Filter 1:",
            "Preamp:",
            "ON",
            "OFF",
            "PK",
            "LSC",
            "HS",
            "LPQ",
            "HP",
            "BP",
            "Fc",
            "Gain",
            "Q",
            "BW",
            "Oct",
            "Hz",
            "dB",
            "12dB",
            "-3.5",
            "0",
            "1e9",
            "NaN",
            "0,7",
            "20000",
            "-1e300",
            ":",
            "#",
            "\u{feff}",
            "\n",
            "\r\n",
            "Filter 2: ON PK",
        ];
        let mut seed: u64 = 1;
        let mut next = |n: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize % n
        };
        for _ in 0..3000 {
            let text: String = (0..next(40))
                .map(|_| format!("{} ", words[next(words.len())]))
                .collect();
            let Ok(import) = parse(&text) else {
                continue;
            };
            let settings = EqualizerSettings {
                parametric: import.filters.clone(),
                ..EqualizerSettings::default()
            };
            assert_eq!(settings.validate(), Ok(()), "{text:?}");
            let again = parse(&format(import.preamp_db, &import.filters)).unwrap();
            assert_eq!(again.filters.len(), import.filters.len());
        }
    }
}
