// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Counters of what the engine's outputs did, readable from any thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::output::StreamTiming;

/// The engine's output counters, summed over every stream it has opened.
///
/// A handle from [`Engine::stats`](super::Engine::stats); clones share the counters.
/// The engine thread adds what each open stream reports every time it wakes up while
/// playing, and once more when it closes the stream, so the numbers lag by a few
/// milliseconds at most. Reading them is lock-free.
#[derive(Debug, Clone, Default)]
pub struct EngineStats(Arc<Counters>);

#[derive(Debug, Default)]
struct Counters {
    streams_opened: AtomicU64,
    frames_delivered: AtomicU64,
    underruns: AtomicU64,
    silent_frames: AtomicU64,
}

/// A snapshot of [`EngineStats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatsSnapshot {
    /// Streams opened since the engine started.
    pub streams_opened: u64,
    /// Frames the devices have taken from the ring buffers, at each stream's rate.
    pub frames_delivered: u64,
    /// Device periods or callbacks that found a ring buffer short
    /// ([`StreamTiming::underruns`]).
    pub underruns: u64,
    /// Frames of silence played in place of missing samples.
    pub silent_frames: u64,
}

impl EngineStats {
    /// The counters now.
    pub fn snapshot(&self) -> StatsSnapshot {
        let c = &self.0;
        StatsSnapshot {
            streams_opened: c.streams_opened.load(Ordering::Relaxed),
            frames_delivered: c.frames_delivered.load(Ordering::Relaxed),
            underruns: c.underruns.load(Ordering::Relaxed),
            silent_frames: c.silent_frames.load(Ordering::Relaxed),
        }
    }

    pub(super) fn stream_opened(&self) {
        self.0.streams_opened.fetch_add(1, Ordering::Relaxed);
    }
}

/// What one stream has reported so far, so that only the growth is added.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Reported {
    frames_delivered: u64,
    underruns: u64,
    silent_frames: u64,
}

impl Reported {
    /// Adds what `timing` shows beyond what was reported before to `stats`.
    pub(super) fn publish(&mut self, timing: &StreamTiming, stats: &EngineStats) {
        let c = &stats.0;
        let add = |counter: &AtomicU64, seen: &mut u64, now: u64| {
            if now > *seen {
                counter.fetch_add(now - *seen, Ordering::Relaxed);
                *seen = now;
            }
        };
        add(
            &c.frames_delivered,
            &mut self.frames_delivered,
            timing.frames_delivered,
        );
        add(&c.underruns, &mut self.underruns, timing.underruns);
        add(
            &c.silent_frames,
            &mut self.silent_frames,
            timing.silent_frames,
        );
    }
}

#[cfg(test)]
mod tests {
    use helikodu_model::audio::SampleRate;

    use super::*;

    fn timing(delivered: u64, underruns: u64, silent: u64) -> StreamTiming {
        StreamTiming {
            rate: SampleRate::HZ_48000,
            frames_written: delivered,
            frames_delivered: delivered,
            frames_discarded: 0,
            delay_frames: 0,
            underruns,
            silent_frames: silent,
            device_rate: None,
        }
    }

    #[test]
    fn only_the_growth_of_each_stream_is_added() {
        let stats = EngineStats::default();
        let mut first = Reported::default();
        first.publish(&timing(100, 1, 10), &stats);
        first.publish(&timing(250, 1, 10), &stats);
        let mut second = Reported::default();
        stats.stream_opened();
        second.publish(&timing(50, 2, 5), &stats);
        assert_eq!(
            stats.clone().snapshot(),
            StatsSnapshot {
                streams_opened: 1,
                frames_delivered: 300,
                underruns: 3,
                silent_frames: 15,
            }
        );
    }
}
