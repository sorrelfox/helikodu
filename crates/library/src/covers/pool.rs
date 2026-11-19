// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The artwork pool: worker threads that make the thumbnails the interface asks
//! for.
//!
//! The queue is bounded and newest first: the covers asked for last are the ones on
//! screen now. When it is full, the oldest request is dropped; the interface asks
//! again for a cover still on screen. A request that is no longer wanted (its tile
//! scrolled away) is cancelled with [`ArtworkPool::cancel`] or
//! [`ArtworkPool::retain`]. A cover that cannot be had is remembered as such until
//! [`ArtworkPool::forget_failures`], so it is not tried again on every scroll.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use helikodu_meta::covers::{CoverHash, CoverLimits};

use super::cache::ThumbnailCache;
use super::load::{CoverReadError, read_cover, render_thumbnail};
use super::sizes::ThumbnailSize;
use crate::Library;
use crate::catalog::CoverLocation;

/// Finds where the bytes of a cover can be read: the library, or a stand-in in
/// tests.
pub trait CoverLocator: Send + Sync {
    /// The places that may hold the cover with `hash`, best first.
    fn locate(&self, hash: &CoverHash) -> Vec<CoverLocation>;
}

impl CoverLocator for Library {
    fn locate(&self, hash: &CoverHash) -> Vec<CoverLocation> {
        self.cover_locations(hash).unwrap_or_default()
    }
}

/// How the artwork pool works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtworkOptions {
    /// The worker threads: by default one per processor, at least two and at most
    /// four, so that the interface and the audio keep their share. Two threads make
    /// about 120 thumbnails of 192 pixels a second from 1200-pixel JPEG covers.
    pub threads: usize,
    /// The most requests waiting; beyond it the oldest is dropped. 256 by default,
    /// about the tiles a large grid keeps bound.
    pub queue_limit: usize,
    /// The limits on every picture decoded.
    pub limits: CoverLimits,
}

impl Default for ArtworkOptions {
    fn default() -> Self {
        Self {
            threads: thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4)),
            queue_limit: 256,
            limits: CoverLimits::default(),
        }
    }
}

/// What [`ArtworkPool::thumbnail`] has for a cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Thumbnail {
    /// The thumbnail file is there.
    Ready(PathBuf),
    /// It is being made; a [`ThumbnailEvent`] follows.
    Scheduled,
    /// It cannot be made: the picture is gone or cannot be decoded.
    Unavailable,
}

/// A thumbnail the pool made, or failed to make.
#[derive(Debug)]
pub struct ThumbnailEvent {
    /// The cover.
    pub hash: CoverHash,
    /// The size asked for.
    pub size: ThumbnailSize,
    /// The thumbnail file, or why there is none.
    pub result: Result<PathBuf, CoverReadError>,
}

type Key = (CoverHash, ThumbnailSize);

#[derive(Debug, Default)]
struct Queue {
    /// Waiting requests, the newest at the back.
    waiting: VecDeque<Key>,
    /// Requests a worker is on.
    working: HashSet<Key>,
    /// Covers that could not be had.
    failed: HashSet<Key>,
    stopping: bool,
}

struct Shared {
    cache: Arc<ThumbnailCache>,
    locator: Arc<dyn CoverLocator>,
    options: ArtworkOptions,
    queue: Mutex<Queue>,
    wake: Condvar,
    notify: Box<dyn Fn(ThumbnailEvent) + Send + Sync>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Makes thumbnails on worker threads. The module documentation explains more.
pub struct ArtworkPool {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for ArtworkPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtworkPool")
            .field("cache", &self.shared.cache.dir())
            .field("options", &self.shared.options)
            .finish_non_exhaustive()
    }
}

impl ArtworkPool {
    /// Starts the threads (named `helikodu-artwork`). Thumbnails go to `cache`; the
    /// covers are found through `locator` (normally the [`Library`]); `notify` is
    /// called on a worker thread for every thumbnail made or failed.
    pub fn start(
        cache: Arc<ThumbnailCache>,
        locator: Arc<dyn CoverLocator>,
        options: ArtworkOptions,
        notify: impl Fn(ThumbnailEvent) + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let threads = options.threads.max(1);
        let shared = Arc::new(Shared {
            cache,
            locator,
            options,
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
            notify: Box::new(notify),
        });
        let mut pool = Self {
            shared,
            threads: Vec::with_capacity(threads),
        };
        for _ in 0..threads {
            let shared = Arc::clone(&pool.shared);
            pool.threads.push(
                thread::Builder::new()
                    .name("helikodu-artwork".to_owned())
                    .spawn(move || work(&shared))?,
            );
        }
        Ok(pool)
    }

    /// The cache the pool fills.
    pub fn cache(&self) -> &Arc<ThumbnailCache> {
        &self.shared.cache
    }

    /// The thumbnail of `hash` in `size`: the file when it is in the cache, or else
    /// scheduled (as the newest request), or unavailable when it failed before.
    pub fn thumbnail(&self, hash: &CoverHash, size: ThumbnailSize) -> Thumbnail {
        if let Some(path) = self.shared.cache.get(hash, size) {
            return Thumbnail::Ready(path);
        }
        let key = (*hash, size);
        let mut queue = self.shared.lock();
        if queue.failed.contains(&key) {
            return Thumbnail::Unavailable;
        }
        if !queue.working.contains(&key) {
            if let Some(at) = queue.waiting.iter().position(|k| *k == key) {
                queue.waiting.remove(at);
            }
            queue.waiting.push_back(key);
            while queue.waiting.len() > self.shared.options.queue_limit.max(1) {
                queue.waiting.pop_front();
            }
            self.shared.wake.notify_one();
        }
        Thumbnail::Scheduled
    }

    /// Cancels the request for `hash` in `size`, if it is still waiting. Returns
    /// whether it was.
    pub fn cancel(&self, hash: &CoverHash, size: ThumbnailSize) -> bool {
        let mut queue = self.shared.lock();
        let before = queue.waiting.len();
        queue.waiting.retain(|k| *k != (*hash, size));
        queue.waiting.len() != before
    }

    /// Keeps only the waiting requests for which `keep` is true, such as the covers
    /// still visible.
    pub fn retain(&self, mut keep: impl FnMut(&CoverHash, ThumbnailSize) -> bool) {
        self.shared
            .lock()
            .waiting
            .retain(|(hash, size)| keep(hash, *size));
    }

    /// The number of waiting requests.
    pub fn waiting(&self) -> usize {
        self.shared.lock().waiting.len()
    }

    /// Forgets the covers that could not be had, after a scan may have changed them.
    pub fn forget_failures(&self) {
        self.shared.lock().failed.clear();
    }
}

impl Drop for ArtworkPool {
    fn drop(&mut self) {
        self.shared.lock().stopping = true;
        self.shared.wake.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// A worker thread: takes the newest request, makes its thumbnail, says so.
fn work(shared: &Shared) {
    loop {
        let key = {
            let mut queue = shared.lock();
            loop {
                if queue.stopping {
                    return;
                }
                if let Some(key) = queue.waiting.pop_back() {
                    queue.working.insert(key);
                    break key;
                }
                queue = shared
                    .wake
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let (hash, size) = key;
        let result = match shared.cache.get(&hash, size) {
            Some(path) => Ok(path),
            None => make(shared, &hash, size),
        };
        {
            let mut queue = shared.lock();
            queue.working.remove(&key);
            if result.is_err() {
                queue.failed.insert(key);
            }
        }
        (shared.notify)(ThumbnailEvent { hash, size, result });
    }
}

fn make(shared: &Shared, hash: &CoverHash, size: ThumbnailSize) -> Result<PathBuf, CoverReadError> {
    let locations = shared.locator.locate(hash);
    let bytes = read_cover(hash, &locations, &shared.options.limits)?;
    let (thumbnail, extension) = render_thumbnail(&bytes, size, &shared.options.limits)?;
    Ok(shared.cache.insert(hash, size, &thumbnail, extension)?)
}
