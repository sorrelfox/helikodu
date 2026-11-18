// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Time as the service sees it: a monotonic clock for intervals and the wall clock
//! for the times it records. Tests use [`FakeClock`], which moves only when told.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

/// The time source of the service.
pub trait Clock: Send + Sync + fmt::Debug {
    /// Monotonic time since an arbitrary start, for intervals: throttling, debouncing.
    fn elapsed(&self) -> Duration;
    /// The wall-clock time, for the times recorded in the library.
    fn now(&self) -> SystemTime;
}

/// A clock that can be shared between threads.
pub type SharedClock = Arc<dyn Clock>;

/// The real clocks.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    /// A clock whose monotonic time starts now.
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// The real clocks, shared.
    pub fn shared() -> SharedClock {
        Arc::new(Self::new())
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// A clock for tests: both times stand still until [`advance`](Self::advance) moves
/// them. Clones share the time.
#[derive(Debug, Clone)]
pub struct FakeClock {
    time: Arc<Mutex<(Duration, SystemTime)>>,
}

impl FakeClock {
    /// A clock at monotonic time zero and wall-clock time `wall`.
    pub fn new(wall: SystemTime) -> Self {
        Self {
            time: Arc::new(Mutex::new((Duration::ZERO, wall))),
        }
    }

    /// Moves both times forward by `by`.
    pub fn advance(&self, by: Duration) {
        let mut time = self.time.lock().unwrap_or_else(PoisonError::into_inner);
        time.0 += by;
        time.1 += by;
    }

    /// The same clock, shared.
    pub fn shared(&self) -> SharedClock {
        Arc::new(self.clone())
    }
}

impl Clock for FakeClock {
    fn elapsed(&self) -> Duration {
        self.time.lock().unwrap_or_else(PoisonError::into_inner).0
    }

    fn now(&self) -> SystemTime {
        self.time.lock().unwrap_or_else(PoisonError::into_inner).1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_moves_when_told() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let clock = FakeClock::new(start);
        let shared = clock.shared();
        assert_eq!(shared.elapsed(), Duration::ZERO);
        clock.advance(Duration::from_secs(3));
        assert_eq!(shared.elapsed(), Duration::from_secs(3));
        assert_eq!(shared.now(), start + Duration::from_secs(3));
    }
}
