// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared pieces of the output spike: the render thread that fills a lock-free
//! ring buffer, the statistics the real-time callback records without
//! allocating or locking, and small helpers.

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Number of callback intervals kept. At 48 kHz and 256 frames per period this
/// is about 21 s of history; later callbacks are counted but not stored.
pub const MAX_SAMPLES: usize = 4096;

/// Statistics written by the real-time callback and read by the main thread.
///
/// Everything is preallocated and uses relaxed atomics, so the callback never
/// allocates, locks or makes a system call to record them.
#[derive(Debug)]
pub struct CallbackStats {
    epoch: Instant,
    pub callbacks: AtomicU64,
    /// Callbacks that could not take a full period from the ring buffer.
    pub underruns: AtomicU64,
    /// Frames filled with zeros because the ring buffer was empty.
    pub frames_missing: AtomicU64,
    pub frames_delivered: AtomicU64,
    /// Frames requested in the most recent callback.
    pub last_period: AtomicU32,
    /// Nanoseconds since `epoch` of the previous callback start.
    last_start_ns: AtomicU64,
    /// Nanoseconds since `epoch` of the first callback.
    pub first_ns: AtomicU64,
    /// Callback intervals in microseconds.
    intervals_us: Box<[AtomicU32]>,
    /// Time spent inside the callback in nanoseconds.
    busy_ns: Box<[AtomicU32]>,
    next: AtomicUsize,
}

impl CallbackStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            epoch: Instant::now(),
            callbacks: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            frames_missing: AtomicU64::new(0),
            frames_delivered: AtomicU64::new(0),
            last_period: AtomicU32::new(0),
            last_start_ns: AtomicU64::new(0),
            first_ns: AtomicU64::new(0),
            intervals_us: (0..MAX_SAMPLES).map(|_| AtomicU32::new(0)).collect(),
            busy_ns: (0..MAX_SAMPLES).map(|_| AtomicU32::new(0)).collect(),
            next: AtomicUsize::new(0),
        })
    }

    /// Call at the start of a callback. Returns the start time for [`Self::end`].
    pub fn begin(&self, period_frames: u32) -> Instant {
        let now = Instant::now();
        let ns = now.duration_since(self.epoch).as_nanos() as u64;
        let prev = self.last_start_ns.swap(ns, Ordering::Relaxed);
        let n = self.callbacks.fetch_add(1, Ordering::Relaxed);
        if n == 0 {
            self.first_ns.store(ns, Ordering::Relaxed);
        } else {
            let i = self.next.fetch_add(1, Ordering::Relaxed);
            if i < MAX_SAMPLES {
                self.intervals_us[i].store(((ns - prev) / 1000) as u32, Ordering::Relaxed);
            }
        }
        self.last_period.store(period_frames, Ordering::Relaxed);
        now
    }

    /// Call at the end of a callback with the number of frames delivered and
    /// the number that had to be filled with silence.
    pub fn end(&self, start: Instant, delivered: u64, missing: u64) {
        self.frames_delivered
            .fetch_add(delivered, Ordering::Relaxed);
        if missing > 0 {
            self.underruns.fetch_add(1, Ordering::Relaxed);
            self.frames_missing.fetch_add(missing, Ordering::Relaxed);
        }
        let i = self.next.load(Ordering::Relaxed);
        if i > 0 && i <= MAX_SAMPLES {
            let busy = start.elapsed().as_nanos().min(u32::MAX as u128) as u32;
            self.busy_ns[i - 1].store(busy, Ordering::Relaxed);
        }
    }

    pub fn intervals_us(&self) -> Vec<u32> {
        let n = self.next.load(Ordering::Relaxed).min(MAX_SAMPLES);
        self.intervals_us[..n]
            .iter()
            .map(|v| v.load(Ordering::Relaxed))
            .collect()
    }

    pub fn busy_ns(&self) -> Vec<u32> {
        let n = self.next.load(Ordering::Relaxed).min(MAX_SAMPLES);
        self.busy_ns[..n]
            .iter()
            .map(|v| v.load(Ordering::Relaxed))
            .collect()
    }

    /// A one-paragraph summary for the report.
    pub fn report(&self, label: &str, rate: u32) -> String {
        let iv = self.intervals_us();
        let busy = self.busy_ns();
        let period = self.last_period.load(Ordering::Relaxed);
        let expected_us = if rate > 0 {
            period as f64 * 1e6 / rate as f64
        } else {
            0.0
        };
        format!(
            "[{label}] callbacks={} period={} frames (expected interval {:.0} us at {} Hz)\n\
             [{label}] interval us: {}\n\
             [{label}] time in callback ns: {}\n\
             [{label}] underruns={} frames_missing={} frames_delivered={}",
            self.callbacks.load(Ordering::Relaxed),
            period,
            expected_us,
            rate,
            Summary::of(&iv),
            Summary::of(&busy),
            self.underruns.load(Ordering::Relaxed),
            self.frames_missing.load(Ordering::Relaxed),
            self.frames_delivered.load(Ordering::Relaxed),
        )
    }
}

/// Min, median, 99th percentile and maximum of a sample set.
#[derive(Debug, Clone, Copy)]
pub struct Summary {
    pub n: usize,
    pub min: u32,
    pub median: u32,
    pub p99: u32,
    pub max: u32,
}

impl Summary {
    pub fn of(values: &[u32]) -> Self {
        let mut v = values.to_vec();
        v.sort_unstable();
        let at = |q: f64| -> u32 {
            if v.is_empty() {
                0
            } else {
                v[((v.len() - 1) as f64 * q).round() as usize]
            }
        };
        Self {
            n: v.len(),
            min: at(0.0),
            median: at(0.5),
            p99: at(0.99),
            max: at(1.0),
        }
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "n={} min={} median={} p99={} max={}",
            self.n, self.min, self.median, self.p99, self.max
        )
    }
}

/// Settings of the render thread.
#[derive(Debug, Clone, Copy)]
pub struct RenderConfig {
    /// Samples (not frames) written per render step.
    pub chunk_samples: usize,
    /// After this long, the render thread stalls once, to provoke underruns.
    pub stall_after: Option<Duration>,
    pub stall_for: Duration,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            chunk_samples: 1024,
            stall_after: None,
            stall_for: Duration::ZERO,
        }
    }
}

/// The render side of the pattern: a normal-priority thread that "decodes and
/// processes" (here: produces digital silence) and keeps the ring buffer full.
/// The real-time callback only copies out of the ring buffer.
pub fn spawn_render(
    mut producer: rtrb::Producer<f32>,
    cfg: RenderConfig,
    stop: Arc<AtomicBool>,
) -> JoinHandle<u64> {
    std::thread::Builder::new()
        .name("render".into())
        .spawn(move || {
            let start = Instant::now();
            let mut stalled = false;
            let mut written: u64 = 0;
            while !stop.load(Ordering::Relaxed) {
                if let Some(after) = cfg.stall_after
                    && !stalled
                    && start.elapsed() >= after
                {
                    stalled = true;
                    std::thread::sleep(cfg.stall_for);
                }
                let free = producer.slots();
                if free < cfg.chunk_samples {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                if let Ok(chunk) = producer.write_chunk_uninit(cfg.chunk_samples) {
                    // Digital silence only: this spike must never make sound.
                    written += chunk.fill_from_iter(std::iter::repeat(0.0f32)) as u64;
                }
            }
            written
        })
        .expect("spawn render thread")
}

/// Copies up to `out.len()` samples out of the ring buffer and fills the rest
/// with silence. Returns the number of samples that were missing.
/// Real-time safe: no allocation, no locks, no system calls.
pub fn pull(consumer: &mut rtrb::Consumer<f32>, out: &mut [f32]) -> usize {
    let available = consumer.slots().min(out.len());
    if let Ok(chunk) = consumer.read_chunk(available) {
        let (a, b) = chunk.as_slices();
        out[..a.len()].copy_from_slice(a);
        out[a.len()..a.len() + b.len()].copy_from_slice(b);
        chunk.commit_all();
    }
    out[available..].fill(0.0);
    out.len() - available
}

/// Runs a command and returns its standard output, or the error as text.
pub fn run(cmd: &str, args: &[&str]) -> String {
    match Command::new(cmd).args(args).output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            if !o.status.success() {
                s.push_str(&String::from_utf8_lossy(&o.stderr));
            }
            s
        }
        Err(e) => format!("failed to run {cmd}: {e}"),
    }
}

/// Seconds since `t0`, for log lines.
pub fn ts(t0: Instant) -> String {
    format!("{:8.3}s", t0.elapsed().as_secs_f64())
}
