// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cover thumbnails: the cache on disk and the artwork pool that fills it.
//!
//! The scan records each cover once by its content hash ([`CoverHash`]), with where
//! it was found, but keeps no picture bytes. A thumbnail is made when the interface
//! first asks for it: [`ArtworkPool::thumbnail`] returns the file when the
//! [`ThumbnailCache`] has it, or schedules it. A worker thread then reads the
//! picture again from the audio file or image file ([`read_cover`], checked against
//! the hash), decodes it within the picture limits, scales it to the
//! [`ThumbnailSize`], and stores it as JPEG (PNG when it has transparency).
//!
//! The cache lives in `$XDG_CACHE_HOME/helikodu/thumbnails/<size>/<hash>.<ext>`
//! ([`ThumbnailCacheOptions::default_dir`]); it is safe to delete. Its total size is
//! limited (2 GiB by default) and the least recently used files are evicted first.
//! [`ThumbnailCache::remove_unreferenced`] with
//! [`Library::cover_hashes`](crate::Library::cover_hashes) removes the thumbnails of
//! covers the library no longer has.
//!
//! [`CoverHash`]: helikodu_meta::covers::CoverHash

mod cache;
mod load;
mod pool;
mod sizes;

pub use cache::{DEFAULT_CACHE_BYTES, Removed, ThumbnailCache, ThumbnailCacheOptions};
pub use load::{CoverReadError, read_cover, render_thumbnail};
pub use pool::{ArtworkOptions, ArtworkPool, CoverLocator, Thumbnail, ThumbnailEvent};
pub use sizes::ThumbnailSize;
