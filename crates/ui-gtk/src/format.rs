// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Text for times, formats and sizes.

use std::time::Duration;

use crate::backend::AudioFormat;
use crate::i18n::{fill, ntr, tr};

/// `m:ss`, or `h:mm:ss` from an hour.
pub(crate) fn duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// `h:mm:ss` always, for times that are compared in a column of long ones.
pub(crate) fn duration_hours(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// The time left, with a minus sign: `−3:12`.
pub(crate) fn remaining(elapsed: Duration, length: Duration) -> String {
    format!("\u{2212}{}", duration(length.saturating_sub(elapsed)))
}

/// A total length in minutes, or hours and minutes: `43 min`, `1 h 12 min`.
pub(crate) fn minutes(d: Duration) -> String {
    let m = (d.as_secs() + 30) / 60;
    if m >= 60 {
        fill(
            &tr("{} h {} min"),
            &[&(m / 60).to_string(), &(m % 60).to_string()],
        )
    } else {
        fill(&tr("{} min"), &[&m.to_string()])
    }
}

/// A sample rate in kHz without a needless decimal: `44.1`, `96`.
pub(crate) fn khz(hz: u32) -> String {
    if hz.is_multiple_of(1000) {
        (hz / 1000).to_string()
    } else {
        format!("{:.1}", f64::from(hz) / 1000.0)
    }
}

/// The short format of a tile: `FLAC 24/96`, `MP3 320`, `DSD64`.
pub(crate) fn short(format: &AudioFormat) -> String {
    let codec = format.codec.clone().unwrap_or_default();
    if codec.to_ascii_lowercase().starts_with("dsd") {
        return match format.sample_rate {
            Some(rate) => format!("DSD{}", rate / 44_100),
            None => codec,
        };
    }
    match (format.bits, format.sample_rate, format.bitrate_kbps) {
        (Some(bits), Some(rate), _) => format!("{codec} {bits}/{}", khz(rate)),
        (None, _, Some(kbps)) => format!("{codec} {kbps}"),
        _ => codec,
    }
}

/// The badges of the album page: codec, rate and depth, channels, album gain.
pub(crate) fn badges(format: &AudioFormat, album_gain_db: Option<f64>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(codec) = &format.codec {
        out.push(codec.clone());
    }
    match (format.sample_rate, format.bits) {
        (Some(rate), Some(bits)) => out.push(fill(
            &tr("{} kHz / {}-bit"),
            &[&khz(rate), &bits.to_string()],
        )),
        (Some(rate), None) => out.push(fill(&tr("{} kHz"), &[&khz(rate)])),
        _ => {}
    }
    match format.channels {
        Some(1) => out.push(tr("Mono")),
        Some(2) => out.push(tr("Stereo")),
        Some(n) => out.push(fill(
            &ntr("{} channel", "{} channels", usize::from(n)),
            &[&n.to_string()],
        )),
        None => {}
    }
    if let Some(gain) = album_gain_db {
        let sign = if gain < 0.0 { "\u{2212}" } else { "+" };
        out.push(fill(
            &tr("Album gain {} dB"),
            &[&format!("{sign}{:.1}", gain.abs())],
        ));
    }
    out
}

/// A count with the locale's thousands separator: `1,284`. The separator is
/// translated (context "thousands separator"), so each language sets its own.
pub(crate) fn count(n: impl TryInto<u64>) -> String {
    group(
        n.try_into().unwrap_or(u64::MAX),
        &gettextrs::pgettext("thousands separator", ","),
    )
}

fn group(n: u64, separator: &str) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 * separator.len());
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push_str(separator);
        }
        out.push(digit);
    }
    out
}

/// A size in bytes: `912 MB`.
pub(crate) fn size(bytes: u64) -> String {
    gtk::glib::format_size(bytes).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(duration(Duration::from_secs(102)), "1:42");
        assert_eq!(duration(Duration::from_secs(3723)), "1:02:03");
        assert_eq!(
            remaining(Duration::from_secs(102), Duration::from_secs(295)),
            "\u{2212}3:13"
        );
        assert_eq!(minutes(Duration::from_secs(43 * 60)), "43 min");
    }

    #[test]
    fn counts() {
        assert_eq!(group(0, ","), "0");
        assert_eq!(group(999, ","), "999");
        assert_eq!(group(1284, ","), "1,284");
        assert_eq!(group(18_402, ","), "18,402");
        assert_eq!(group(1_000_000, "\u{202f}"), "1\u{202f}000\u{202f}000");
    }

    #[test]
    fn formats() {
        let flac = AudioFormat {
            codec: Some("FLAC".into()),
            sample_rate: Some(44_100),
            bits: Some(16),
            bitrate_kbps: None,
            channels: Some(2),
        };
        assert_eq!(short(&flac), "FLAC 16/44.1");
        let mp3 = AudioFormat {
            codec: Some("MP3".into()),
            sample_rate: Some(44_100),
            bits: None,
            bitrate_kbps: Some(320),
            channels: Some(2),
        };
        assert_eq!(short(&mp3), "MP3 320");
        assert_eq!(
            badges(&flac, Some(-6.3)),
            [
                "FLAC",
                "44.1 kHz / 16-bit",
                "Stereo",
                "Album gain \u{2212}6.3 dB"
            ]
        );
    }
}
