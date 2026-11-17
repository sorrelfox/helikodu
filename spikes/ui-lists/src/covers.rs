// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Synthetic cover images on disk, and a thumbnail loader that decodes and
//! scales them on worker threads. The newest request is served first, so the
//! tiles that just scrolled into view win over those that already left it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use crate::catalog::Rng;

pub const COVER_SIZE: u32 = 600;

pub fn cover_path(dir: &Path, file: u32) -> PathBuf {
    dir.join(format!("cover-{file:05}.jpg"))
}

/// Writes `count` JPEG covers (600 x 600, gradients, shapes and grain, so they
/// compress like real artwork rather than like flat colour). Skips existing files.
pub fn write_covers(dir: &Path, count: u32) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let missing: Vec<u32> = (0..count)
        .filter(|i| !cover_path(dir, *i).exists())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let started = Instant::now();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunks: Vec<Vec<u32>> = missing
        .chunks(missing.len().div_ceil(threads))
        .map(<[u32]>::to_vec)
        .collect();
    std::thread::scope(|s| {
        for chunk in chunks {
            s.spawn(move || {
                for i in chunk {
                    let img = synth(i);
                    let _ = img.save_with_format(cover_path(dir, i), image::ImageFormat::Jpeg);
                }
            });
        }
    });
    eprintln!(
        "wrote {} covers in {:.1} s",
        missing.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn synth(seed: u32) -> image::RgbImage {
    let mut rng = Rng::new(u64::from(seed) * 31 + 5);
    let s = COVER_SIZE;
    let c1 = [
        rng.below(256) as f32,
        rng.below(256) as f32,
        rng.below(256) as f32,
    ];
    let c2 = [
        rng.below(256) as f32,
        rng.below(256) as f32,
        rng.below(256) as f32,
    ];
    let circles: Vec<(f32, f32, f32, [u8; 3])> = (0..4)
        .map(|_| {
            (
                rng.unit() as f32 * s as f32,
                rng.unit() as f32 * s as f32,
                30.0 + rng.unit() as f32 * 200.0,
                [
                    rng.below(256) as u8,
                    rng.below(256) as u8,
                    rng.below(256) as u8,
                ],
            )
        })
        .collect();
    let mut img = image::RgbImage::new(s, s);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let t = (x + y) as f32 / (2 * s) as f32;
        let mut c = [0u8; 3];
        for k in 0..3 {
            c[k] = (c1[k] * (1.0 - t) + c2[k] * t) as u8;
        }
        for (cx, cy, r, col) in &circles {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            if d < *r {
                c = *col;
            }
        }
        let grain = (rng.below(25) as i16) - 12;
        *px = image::Rgb(c.map(|v| (i16::from(v) + grain).clamp(0, 255) as u8));
    }
    img
}

/// A request for one thumbnail.
#[derive(Debug, Clone)]
struct Job {
    album: u32,
    file: u32,
    px: u32,
}

/// A decoded, scaled thumbnail, ready to become a texture on the main thread.
#[derive(Debug)]
pub struct Thumb {
    pub album: u32,
    pub px: u32,
    pub rgba: Vec<u8>,
    pub decode_ms: f64,
}

#[derive(Debug, Default)]
struct Queue {
    jobs: VecDeque<Job>,
    in_flight: HashSet<u32>,
    shutdown: bool,
}

/// Counters shared with the workers.
#[derive(Debug, Default)]
pub struct LoaderStats {
    pub requested: u64,
    pub cancelled: u64,
    pub dropped_oldest: u64,
    pub requeued_visible: u64,
    pub decoded: u64,
}

#[derive(Debug)]
pub struct Loader {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    pub stats: Arc<Mutex<LoaderStats>>,
    max_pending: usize,
}

impl Loader {
    pub fn start(
        dir: PathBuf,
        workers: usize,
        max_pending: usize,
        tx: async_channel::Sender<Thumb>,
    ) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let stats = Arc::new(Mutex::new(LoaderStats::default()));
        for _ in 0..workers {
            let queue = queue.clone();
            let stats = stats.clone();
            let tx = tx.clone();
            let dir = dir.clone();
            std::thread::spawn(move || worker(&queue, &stats, &tx, &dir));
        }
        Self {
            queue,
            stats,
            max_pending,
        }
    }

    /// Queues a request. The newest request is served first.
    pub fn request(&self, album: u32, file: u32, px: u32) {
        let (lock, cv) = &*self.queue;
        let Ok(mut q) = lock.lock() else { return };
        q.jobs.retain(|j| j.album != album);
        q.jobs.push_back(Job { album, file, px });
        let mut dropped = 0;
        while q.jobs.len() > self.max_pending {
            q.jobs.pop_front();
            dropped += 1;
        }
        drop(q);
        cv.notify_one();
        if let Ok(mut s) = self.stats.lock() {
            s.requested += 1;
            s.dropped_oldest += dropped;
        }
    }

    /// Withdraws a request whose tile was recycled before it was served.
    pub fn cancel(&self, album: u32) {
        let (lock, _) = &*self.queue;
        let Ok(mut q) = lock.lock() else { return };
        let before = q.jobs.len();
        q.jobs.retain(|j| j.album != album);
        let n = (before - q.jobs.len()) as u64;
        drop(q);
        if n > 0
            && let Ok(mut s) = self.stats.lock()
        {
            s.cancelled += n;
        }
    }

    /// Moves the given requests to the front of the line (served next),
    /// re-queuing any that were dropped. Used for tiles that are on screen.
    pub fn promote(&self, jobs: &[(u32, u32, u32)]) {
        if jobs.is_empty() {
            return;
        }
        let (lock, cv) = &*self.queue;
        let Ok(mut q) = lock.lock() else { return };
        let mut requeued = 0;
        for &(album, file, px) in jobs {
            if q.in_flight.contains(&album) {
                continue;
            }
            let before = q.jobs.len();
            q.jobs.retain(|j| j.album != album);
            if q.jobs.len() == before {
                requeued += 1;
            }
            q.jobs.push_back(Job { album, file, px });
        }
        drop(q);
        cv.notify_all();
        if let Ok(mut s) = self.stats.lock() {
            s.requeued_visible += requeued;
        }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        if let Ok(mut q) = self.queue.0.lock() {
            q.shutdown = true;
        }
        self.queue.1.notify_all();
    }
}

fn worker(
    queue: &(Mutex<Queue>, Condvar),
    stats: &Mutex<LoaderStats>,
    tx: &async_channel::Sender<Thumb>,
    dir: &Path,
) {
    let (lock, cv) = queue;
    loop {
        let job = {
            let Ok(mut q) = lock.lock() else { return };
            loop {
                if q.shutdown {
                    return;
                }
                if let Some(j) = q.jobs.pop_back() {
                    q.in_flight.insert(j.album);
                    break j;
                }
                q = match cv.wait(q) {
                    Ok(q) => q,
                    Err(_) => return,
                };
            }
        };
        let t = Instant::now();
        let img = image::open(cover_path(dir, job.file));
        let thumb = img
            .ok()
            .map(|img| image::imageops::thumbnail(&img.to_rgba8(), job.px, job.px));
        let decode_ms = t.elapsed().as_secs_f64() * 1000.0;
        if let Ok(mut q) = lock.lock() {
            q.in_flight.remove(&job.album);
        }
        let Some(thumb) = thumb else { continue };
        if let Ok(mut s) = stats.lock() {
            s.decoded += 1;
        }
        if tx
            .send_blocking(Thumb {
                album: job.album,
                px: job.px,
                rgba: thumb.into_raw(),
                decode_ms,
            })
            .is_err()
        {
            return;
        }
    }
}

/// Least-recently-used cache, small and simple: a map plus a use counter.
#[derive(Debug)]
pub struct Lru<V> {
    cap: usize,
    tick: u64,
    map: HashMap<u32, (u64, V)>,
}

impl<V: Clone> Lru<V> {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            tick: 0,
            map: HashMap::new(),
        }
    }

    pub fn get(&mut self, k: u32) -> Option<V> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(&k).map(|e| {
            e.0 = t;
            e.1.clone()
        })
    }

    pub fn put(&mut self, k: u32, v: V) {
        self.tick += 1;
        self.map.insert(k, (self.tick, v));
        if self.map.len() > self.cap
            && let Some(oldest) = self
                .map
                .iter()
                .min_by_key(|(_, (t, _))| *t)
                .map(|(k, _)| *k)
        {
            self.map.remove(&oldest);
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}
