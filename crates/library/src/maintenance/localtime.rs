// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The local time, for the deletion date of a trashed file, which the trash
//! specification wants in the user's time zone.
//!
//! The zone is `TZ` (a path after `:`, a zone name under `/usr/share/zoneinfo`, or
//! a POSIX rule such as `CET-1CEST,M3.5.0,M10.5.0/3`), or else `/etc/localtime`.
//! Zone files (TZif, versions 1 to 4) are read with their transitions and the rule
//! in their footer for times after the last transition. Anything that cannot be
//! read gives UTC.

use std::fs;
use std::path::PathBuf;

/// `unix` (seconds since the epoch) as local time, `YYYY-MM-DDThh:mm:ss`.
pub(crate) fn format_local(unix: i64) -> String {
    let offset = utc_offset_at(unix);
    format_with_offset(unix, offset)
}

fn format_with_offset(unix: i64, offset: i64) -> String {
    let local = unix + offset;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// The offset of local time from UTC at `unix`, in seconds east.
pub(crate) fn utc_offset_at(unix: i64) -> i64 {
    let tz = std::env::var("TZ").ok();
    let file = match tz.as_deref() {
        Some("") => return 0,
        Some(tz) => {
            let name = tz.strip_prefix(':').unwrap_or(tz);
            if name.starts_with('/') {
                Some(PathBuf::from(name))
            } else if !name.contains("..") && fs::metadata(zone_path(name)).is_ok() {
                Some(zone_path(name))
            } else {
                return PosixRule::parse(name).map_or(0, |rule| rule.offset_at(unix));
            }
        }
        None => Some(PathBuf::from("/etc/localtime")),
    };
    file.and_then(|path| fs::read(path).ok())
        .and_then(|bytes| tzif_offset(&bytes, unix))
        .unwrap_or(0)
}

fn zone_path(name: &str) -> PathBuf {
    PathBuf::from("/usr/share/zoneinfo").join(name)
}

/// Reads big-endian integers from a byte slice.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let part = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(part)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
}

/// The counts of a TZif header.
struct Counts {
    isut: usize,
    isstd: usize,
    leap: usize,
    time: usize,
    types: usize,
    chars: usize,
}

fn header(r: &mut Reader<'_>) -> Option<(u8, Counts)> {
    if r.take(4)? != b"TZif" {
        return None;
    }
    let version = r.take(1)?[0];
    r.take(15)?;
    let mut n = || r.u32().and_then(|v| usize::try_from(v).ok());
    let counts = Counts {
        isut: n()?,
        isstd: n()?,
        leap: n()?,
        time: n()?,
        types: n()?,
        chars: n()?,
    };
    Some((version, counts))
}

/// The UTC offset at `unix` from a TZif file.
fn tzif_offset(bytes: &[u8], unix: i64) -> Option<i64> {
    let mut r = Reader { bytes, at: 0 };
    let (version, counts) = header(&mut r)?;
    let (time_size, counts) = if version >= b'2' {
        let v1 = counts.time * 5
            + counts.types * 6
            + counts.chars
            + counts.leap * 8
            + counts.isstd
            + counts.isut;
        r.take(v1)?;
        let (_, counts) = header(&mut r)?;
        (8, counts)
    } else {
        (4, counts)
    };
    let mut times = Vec::with_capacity(counts.time);
    for _ in 0..counts.time {
        let raw = r.take(time_size)?;
        times.push(if time_size == 8 {
            i64::from_be_bytes(raw.try_into().ok()?)
        } else {
            i64::from(i32::from_be_bytes(raw.try_into().ok()?))
        });
    }
    let indexes = r.take(counts.time)?.to_vec();
    let mut types = Vec::with_capacity(counts.types);
    for _ in 0..counts.types {
        let raw = r.take(6)?;
        let offset = i64::from(i32::from_be_bytes(raw[..4].try_into().ok()?));
        types.push((offset, raw[4] != 0));
    }
    r.take(counts.chars + counts.leap * (time_size + 4) + counts.isstd + counts.isut)?;
    let footer = if version >= b'2' {
        let rest = &bytes[r.at..];
        std::str::from_utf8(rest)
            .ok()
            .and_then(|text| text.strip_prefix('\n'))
            .and_then(|text| text.split('\n').next())
            .and_then(PosixRule::parse)
    } else {
        None
    };
    let last = times.last().copied();
    if let (Some(rule), Some(last)) = (&footer, last)
        && unix >= last
    {
        return Some(rule.offset_at(unix));
    }
    if times.is_empty() {
        if let Some(rule) = footer {
            return Some(rule.offset_at(unix));
        }
        return types.first().map(|t| t.0);
    }
    match times.iter().rposition(|&t| t <= unix) {
        Some(i) => types.get(usize::from(*indexes.get(i)?)).map(|t| t.0),
        None => types.iter().find(|t| !t.1).or(types.first()).map(|t| t.0),
    }
}

/// A POSIX `TZ` rule: standard and summer offsets and when summer time starts and
/// ends.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PosixRule {
    /// Seconds east of UTC.
    std_offset: i64,
    dst: Option<Dst>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Dst {
    offset: i64,
    start: (DateRule, i64),
    end: (DateRule, i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DateRule {
    /// `Jn`: day 1 to 365, February 29 never counted.
    Julian(i64),
    /// `n`: day 0 to 365, counting February 29.
    Zero(i64),
    /// `Mm.w.d`: day `d` (0 Sunday) of week `w` (5 the last) of month `m`.
    Month(i64, i64, i64),
}

impl PosixRule {
    fn parse(text: &str) -> Option<Self> {
        let mut rest = text;
        rest = skip_name(rest)?;
        let (std_west, after) = parse_offset(rest)?;
        rest = after;
        if rest.is_empty() {
            return Some(Self {
                std_offset: -std_west,
                dst: None,
            });
        }
        rest = skip_name(rest)?;
        let mut dst_west = std_west - 3600;
        if !rest.is_empty() && !rest.starts_with(',') {
            let (west, after) = parse_offset(rest)?;
            dst_west = west;
            rest = after;
        }
        let rest = rest.strip_prefix(',')?;
        let (start, end) = rest.split_once(',')?;
        Some(Self {
            std_offset: -std_west,
            dst: Some(Dst {
                offset: -dst_west,
                start: parse_change(start)?,
                end: parse_change(end)?,
            }),
        })
    }

    fn offset_at(&self, unix: i64) -> i64 {
        let Some(dst) = &self.dst else {
            return self.std_offset;
        };
        let (year, _, _) = civil_from_days((unix + self.std_offset).div_euclid(86_400));
        // The changes are in local time: the start in standard time, the end in
        // summer time.
        let start = change_utc(year, dst.start, self.std_offset);
        let end = change_utc(year, dst.end, dst.offset);
        let summer = if start <= end {
            unix >= start && unix < end
        } else {
            unix >= start || unix < end
        };
        if summer { dst.offset } else { self.std_offset }
    }
}

fn change_utc(year: i64, (rule, time): (DateRule, i64), offset: i64) -> i64 {
    let jan1 = days_from_civil(year, 1, 1);
    let leap = is_leap(year);
    let day = match rule {
        DateRule::Julian(n) => jan1 + n - 1 + i64::from(leap && n >= 60),
        DateRule::Zero(n) => jan1 + n,
        DateRule::Month(m, w, d) => {
            let first = days_from_civil(year, m, 1);
            // 1970-01-01 was a Thursday (4).
            let weekday = (first + 4).rem_euclid(7);
            let mut day = first + (d - weekday).rem_euclid(7) + (w - 1) * 7;
            let next_month = if m == 12 {
                days_from_civil(year + 1, 1, 1)
            } else {
                days_from_civil(year, m + 1, 1)
            };
            while day >= next_month {
                day -= 7;
            }
            day
        }
    };
    day * 86_400 + time - offset
}

fn skip_name(text: &str) -> Option<&str> {
    if let Some(rest) = text.strip_prefix('<') {
        let end = rest.find('>')?;
        return Some(&rest[end + 1..]);
    }
    let end = text
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(text.len());
    (end >= 3).then(|| &text[end..])
}

/// `[+-]hh[:mm[:ss]]` in seconds, and the rest.
fn parse_offset(text: &str) -> Option<(i64, &str)> {
    let (sign, text) = match text.as_bytes().first()? {
        b'-' => (-1, &text[1..]),
        b'+' => (1, &text[1..]),
        _ => (1, text),
    };
    let end = text
        .find(|c: char| !(c.is_ascii_digit() || c == ':'))
        .unwrap_or(text.len());
    let mut seconds = 0;
    let mut scale = 3600;
    for part in text[..end].split(':') {
        seconds += part.parse::<i64>().ok()? * scale;
        scale /= 60;
    }
    Some((sign * seconds, &text[end..]))
}

fn parse_change(text: &str) -> Option<(DateRule, i64)> {
    let (date, time) = match text.split_once('/') {
        Some((date, time)) => (date, parse_offset(time)?.0),
        None => (text, 7200),
    };
    let rule = if let Some(m) = date.strip_prefix('M') {
        let mut parts = m.split('.').map(|p| p.parse::<i64>().ok());
        DateRule::Month(parts.next()??, parts.next()??, parts.next()??)
    } else if let Some(n) = date.strip_prefix('J') {
        DateRule::Julian(n.parse().ok()?)
    } else {
        DateRule::Zero(date.parse().ok()?)
    };
    Some((rule, time))
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Days since 1970-01-01 of a date (proleptic Gregorian).
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day since 1970-01-01.
pub(crate) fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_formatting() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(format_with_offset(0, 0), "1970-01-01T00:00:00");
        // 2026-11-18 12:34:56 UTC, shown an hour east.
        let t = days_from_civil(2026, 11, 18) * 86_400 + 12 * 3600 + 34 * 60 + 56;
        assert_eq!(format_with_offset(t, 3600), "2026-11-18T13:34:56");
    }

    #[test]
    fn posix_rules_follow_summer_time() {
        let cet = PosixRule::parse("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        let winter = days_from_civil(2026, 1, 15) * 86_400;
        let summer = days_from_civil(2026, 7, 15) * 86_400;
        assert_eq!(cet.offset_at(winter), 3600);
        assert_eq!(cet.offset_at(summer), 7200);
        // Summer time starts on 2026-03-29 at 01:00 UTC.
        let start = days_from_civil(2026, 3, 29) * 86_400 + 3600;
        assert_eq!(cet.offset_at(start - 1), 3600);
        assert_eq!(cet.offset_at(start), 7200);
        let east = PosixRule::parse("EST5EDT,M3.2.0,M11.1.0").unwrap();
        assert_eq!(east.offset_at(winter), -5 * 3600);
        assert_eq!(east.offset_at(summer), -4 * 3600);
        assert_eq!(
            PosixRule::parse("<+0530>-5:30").unwrap().offset_at(0),
            19_800
        );
        assert_eq!(PosixRule::parse("UTC0").unwrap().offset_at(summer), 0);
    }

    #[test]
    fn the_system_zone_file_reads() {
        // Whatever the zone, reading it gives an offset within a day.
        for path in ["/etc/localtime", "/usr/share/zoneinfo/Europe/Helsinki"] {
            if let Ok(bytes) = fs::read(path) {
                let offset = tzif_offset(&bytes, 1_800_000_000).unwrap();
                assert!(offset.abs() < 86_400);
            }
        }
        if let Ok(bytes) = fs::read("/usr/share/zoneinfo/Europe/Helsinki") {
            let winter = days_from_civil(2040, 1, 15) * 86_400;
            let summer = days_from_civil(2040, 7, 15) * 86_400;
            assert_eq!(tzif_offset(&bytes, winter), Some(7200));
            assert_eq!(tzif_offset(&bytes, summer), Some(10_800));
            let past = days_from_civil(2000, 7, 15) * 86_400;
            assert_eq!(tzif_offset(&bytes, past), Some(10_800));
        }
    }
}
