// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Wall-clock times and durations as the database stores them: seconds since the Unix
//! epoch, and milliseconds.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `time` in whole seconds since the Unix epoch; negative before it.
pub(crate) fn to_unix_seconds(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_secs()).map_or(i64::MIN, |s| -s),
    }
}

/// The time `seconds` after the Unix epoch.
pub(crate) fn from_unix_seconds(seconds: i64) -> SystemTime {
    let magnitude = Duration::from_secs(seconds.unsigned_abs());
    if seconds >= 0 {
        UNIX_EPOCH.checked_add(magnitude).unwrap_or(UNIX_EPOCH)
    } else {
        UNIX_EPOCH.checked_sub(magnitude).unwrap_or(UNIX_EPOCH)
    }
}

/// `duration` in whole milliseconds.
pub(crate) fn to_millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// `millis` milliseconds; zero for a negative value.
pub(crate) fn from_millis(millis: i64) -> Duration {
    Duration::from_millis(u64::try_from(millis).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_round_trip() {
        for seconds in [0, 1, 1_763_337_600, -86_400] {
            assert_eq!(to_unix_seconds(from_unix_seconds(seconds)), seconds);
        }
    }

    #[test]
    fn millis_round_trip() {
        assert_eq!(to_millis(Duration::from_millis(1234)), 1234);
        assert_eq!(from_millis(1234), Duration::from_millis(1234));
        assert_eq!(from_millis(-5), Duration::ZERO);
    }
}
