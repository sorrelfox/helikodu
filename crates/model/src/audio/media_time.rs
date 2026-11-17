// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Exact times within a stream.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU32;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::SampleRate;

const NANOS_PER_SECOND: u32 = 1_000_000_000;

/// A position in a stream, or a length of one, as a whole number of ticks at a tick
/// rate.
///
/// The tick rate is usually the sample rate, so a time counts frames exactly. Other
/// rates keep other sources exact: milliseconds from a seek bar, the 75 frames per
/// second of a CUE sheet, or nanoseconds from a [`Duration`]. Times with different
/// tick rates compare by the time they stand for, and [`frames_at`](Self::frames_at)
/// converts any of them to frames.
///
/// Serialized as `{ "ticks": <u64>, "ticks_per_second": <u32> }`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MediaTime {
    ticks: u64,
    ticks_per_second: NonZeroU32,
}

impl MediaTime {
    /// The start of a stream; a length of nothing.
    pub const ZERO: Self = Self {
        ticks: 0,
        ticks_per_second: NonZeroU32::MIN,
    };

    /// `ticks` ticks at `ticks_per_second`.
    pub const fn new(ticks: u64, ticks_per_second: NonZeroU32) -> Self {
        Self {
            ticks,
            ticks_per_second,
        }
    }

    /// `frames` frames at the sample rate `rate`.
    pub const fn from_frames(frames: u64, rate: SampleRate) -> Self {
        Self::new(frames, rate.get())
    }

    /// A whole number of milliseconds.
    pub const fn from_millis(millis: u64) -> Self {
        Self::new(millis, NonZeroU32::new(1000).expect("not zero"))
    }

    /// A time in CUE sheet frames, of which there are 75 per second.
    pub const fn from_cue_frames(cue_frames: u64) -> Self {
        Self::new(cue_frames, NonZeroU32::new(75).expect("not zero"))
    }

    /// The time of a [`Duration`], to the nanosecond. Saturates after about 584 years.
    pub fn from_duration(duration: Duration) -> Self {
        let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        Self::new(nanos, NonZeroU32::new(NANOS_PER_SECOND).expect("not zero"))
    }

    /// The number of ticks.
    pub const fn ticks(self) -> u64 {
        self.ticks
    }

    /// The number of ticks per second.
    pub const fn ticks_per_second(self) -> NonZeroU32 {
        self.ticks_per_second
    }

    /// Whether this is the start of a stream.
    pub const fn is_zero(self) -> bool {
        self.ticks == 0
    }

    /// The time as a number of frames at `rate`, rounded down when it falls between
    /// two frames.
    pub fn frames_at(self, rate: SampleRate) -> u64 {
        self.scaled(rate.hz())
    }

    /// The same time with the tick rate `rate`, rounded down to a whole frame.
    pub fn at_rate(self, rate: SampleRate) -> Self {
        Self::from_frames(self.frames_at(rate), rate)
    }

    /// The time as a [`Duration`], rounded down to the nanosecond.
    pub fn as_duration(self) -> Duration {
        let rate = u64::from(self.ticks_per_second.get());
        let seconds = self.ticks / rate;
        let rest = self.ticks % rate;
        // `rest` < `rate` ≤ u32::MAX, so the product fits and the quotient is below one second.
        let nanos = rest * u64::from(NANOS_PER_SECOND) / rate;
        Duration::new(
            seconds,
            u32::try_from(nanos).unwrap_or(NANOS_PER_SECOND - 1),
        )
    }

    /// The time in seconds, approximately.
    pub fn as_secs_f64(self) -> f64 {
        self.as_duration().as_secs_f64()
    }

    /// The number of ticks at `rate` ticks per second, rounded down, saturating.
    fn scaled(self, rate: u32) -> u64 {
        let exact =
            u128::from(self.ticks) * u128::from(rate) / u128::from(self.ticks_per_second.get());
        u64::try_from(exact).unwrap_or(u64::MAX)
    }

    /// `self.ticks / self.rate` and `other.ticks / other.rate` brought to a common
    /// denominator.
    fn cross(self, other: Self) -> (u128, u128) {
        (
            u128::from(self.ticks) * u128::from(other.ticks_per_second.get()),
            u128::from(other.ticks) * u128::from(self.ticks_per_second.get()),
        )
    }
}

impl Default for MediaTime {
    fn default() -> Self {
        Self::ZERO
    }
}

impl PartialEq for MediaTime {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = self.cross(*other);
        a == b
    }
}

impl Eq for MediaTime {}

impl PartialOrd for MediaTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MediaTime {
    fn cmp(&self, other: &Self) -> Ordering {
        let (a, b) = self.cross(*other);
        a.cmp(&b)
    }
}

impl Hash for MediaTime {
    /// Hashes the time in lowest terms, so that equal times hash alike.
    fn hash<H: Hasher>(&self, state: &mut H) {
        let rate = u64::from(self.ticks_per_second.get());
        let divisor = greatest_common_divisor(self.ticks, rate);
        (self.ticks / divisor, rate / divisor).hash(state);
    }
}

fn greatest_common_divisor(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl From<Duration> for MediaTime {
    fn from(duration: Duration) -> Self {
        Self::from_duration(duration)
    }
}

impl fmt::Display for MediaTime {
    /// `m:ss.mmm`, or `h:mm:ss.mmm` from one hour on, rounded down to the millisecond.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let millis = self.scaled(1000);
        let (seconds, millis) = (millis / 1000, millis % 1000);
        let (minutes, seconds) = (seconds / 60, seconds % 60);
        let (hours, minutes) = (minutes / 60, minutes % 60);
        if hours > 0 {
            write!(f, "{hours}:{minutes:02}:{seconds:02}.{millis:03}")
        } else {
            write!(f, "{minutes}:{seconds:02}.{millis:03}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(hz: u32) -> SampleRate {
        SampleRate::new(hz).unwrap()
    }

    #[test]
    fn cue_times_convert_exactly() {
        // 03:25:12 in a CUE sheet.
        let start = MediaTime::from_cue_frames(3 * 60 * 75 + 25 * 75 + 12);
        assert_eq!(
            start.frames_at(rate(44_100)),
            (3 * 60 + 25) * 44_100 + 12 * 588
        );
        assert_eq!(
            start.frames_at(rate(48_000)),
            (3 * 60 + 25) * 48_000 + 12 * 640
        );
    }

    #[test]
    fn conversions_round_down() {
        // One millisecond is 44.1 frames at 44.1 kHz.
        assert_eq!(MediaTime::from_millis(1).frames_at(rate(44_100)), 44);
        // One frame at 44.1 kHz is 22 675.736… ns.
        let frame = MediaTime::from_frames(1, rate(44_100));
        assert_eq!(frame.as_duration(), Duration::from_nanos(22_675));
        assert_eq!(
            MediaTime::from_frames(44_100, rate(44_100)).as_duration(),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn duration_round_trip() {
        let duration = Duration::new(3_725, 123_456_789);
        assert_eq!(MediaTime::from_duration(duration).as_duration(), duration);
    }

    #[test]
    fn comparison_is_by_time_across_rates() {
        let a = MediaTime::from_frames(48_000, rate(48_000));
        let b = MediaTime::from_millis(1000);
        let c = MediaTime::from_frames(44_101, rate(44_100));
        assert_eq!(a, b);
        assert!(c > a);
        assert!(MediaTime::ZERO < c);
        assert_eq!(MediaTime::ZERO, MediaTime::from_frames(0, rate(96_000)));
    }

    #[test]
    fn equal_times_hash_alike() {
        use std::collections::hash_map::DefaultHasher;
        let hash = |time: MediaTime| {
            let mut hasher = DefaultHasher::new();
            time.hash(&mut hasher);
            hasher.finish()
        };
        assert_eq!(
            hash(MediaTime::from_millis(1500)),
            hash(MediaTime::from_frames(72_000, rate(48_000)))
        );
        assert_eq!(
            hash(MediaTime::ZERO),
            hash(MediaTime::from_frames(0, rate(44_100)))
        );
    }

    #[test]
    fn rescaling_keeps_whole_frames() {
        let at_44 = MediaTime::from_frames(441, rate(44_100));
        let at_48 = at_44.at_rate(rate(48_000));
        assert_eq!(at_48.ticks(), 480);
        assert_eq!(at_48, at_44);
    }

    #[test]
    fn large_values_saturate() {
        let huge = MediaTime::new(u64::MAX, NonZeroU32::MIN);
        assert_eq!(huge.frames_at(rate(192_000)), u64::MAX);
    }

    #[test]
    fn display() {
        assert_eq!(MediaTime::from_millis(62_345).to_string(), "1:02.345");
        assert_eq!(MediaTime::from_millis(3_723_004).to_string(), "1:02:03.004");
        assert_eq!(MediaTime::ZERO.to_string(), "0:00.000");
    }

    #[test]
    fn serde_rejects_a_zero_rate() {
        let time: MediaTime = serde_json::from_str(r#"{"ticks":5,"ticks_per_second":75}"#).unwrap();
        assert_eq!(time, MediaTime::from_cue_frames(5));
        assert!(serde_json::from_str::<MediaTime>(r#"{"ticks":5,"ticks_per_second":0}"#).is_err());
    }
}
