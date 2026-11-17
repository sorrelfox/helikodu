// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The throughput of the artwork pool: thumbnails made per second from typical
//! covers, and lookups of thumbnails already in the cache.
//!
//! ```sh
//! cargo bench -p helikodu-library --bench thumbnails [-- <covers> [--side PIXELS]]
//! ```
//!
//! Writes `<covers>` JPEG pictures (200 by default) of `--side` pixels (1200 by
//! default) with a gradient and noise under `target/thumbnail-bench/`, then makes
//! their 192-pixel and 384-pixel thumbnails with the pool at two threads and at one
//! thread per processor, into an empty cache each time, and prints the time and the
//! thumbnails per second.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helikodu_library::{
    ArtworkOptions, ArtworkPool, CoverHash, CoverLocation, CoverLocator, Thumbnail, ThumbnailCache,
    ThumbnailCacheOptions, ThumbnailSize,
};
use helikodu_meta::covers::RgbaPicture;

struct Files(Vec<(CoverHash, PathBuf)>);

impl CoverLocator for Files {
    fn locate(&self, hash: &CoverHash) -> Vec<CoverLocation> {
        self.0
            .iter()
            .filter(|(h, _)| h == hash)
            .map(|(_, path)| CoverLocation::File { path: path.clone() })
            .collect()
    }
}

fn fail(message: &str) -> ! {
    eprintln!("thumbnail bench: {message}");
    std::process::exit(2);
}

/// A picture with a gradient and some noise, so that it compresses like a photo.
fn picture(side: u32, seed: u32) -> Vec<u8> {
    let mut state = seed.wrapping_mul(2_654_435_761).max(1);
    let mut pixels = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let noise = (state & 0x1f) as u8;
            let r = ((x * 255 / side) as u8).wrapping_add(noise);
            let g = ((y * 255 / side) as u8).wrapping_add(noise);
            let b = (((x + y + seed * 37) % 256) as u8).wrapping_add(noise);
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    }
    RgbaPicture::new(side, side, pixels)
        .and_then(|p| p.encode_jpeg(90).ok())
        .unwrap_or_else(|| fail("cannot encode a picture"))
}

fn run(
    files: &Arc<Files>,
    cache_dir: &Path,
    threads: usize,
    size: ThumbnailSize,
) -> (Duration, usize) {
    let _ = fs::remove_dir_all(cache_dir);
    let cache = Arc::new(
        ThumbnailCache::open(&ThumbnailCacheOptions::new(cache_dir))
            .unwrap_or_else(|e| fail(&format!("cannot open the cache: {e}"))),
    );
    let (sender, events) = mpsc::channel();
    let sender = Mutex::new(sender);
    let options = ArtworkOptions {
        threads,
        queue_limit: files.0.len(),
        ..ArtworkOptions::default()
    };
    let locator: Arc<dyn CoverLocator> = files.clone();
    let pool = ArtworkPool::start(cache, locator, options, move |event| {
        if let Ok(sender) = sender.lock() {
            let _ = sender.send(event.result.is_ok());
        }
    })
    .unwrap_or_else(|e| fail(&format!("cannot start the pool: {e}")));
    let started = Instant::now();
    for (hash, _) in &files.0 {
        if pool.thumbnail(hash, size) != Thumbnail::Scheduled {
            fail("a thumbnail was not scheduled");
        }
    }
    let mut made = 0;
    for _ in &files.0 {
        match events.recv_timeout(Duration::from_secs(120)) {
            Ok(true) => made += 1,
            Ok(false) => {}
            Err(_) => fail("the pool stopped answering"),
        }
    }
    let elapsed = started.elapsed();
    // Lookups of thumbnails in the cache.
    let lookups = Instant::now();
    let rounds = 20;
    for _ in 0..rounds {
        for (hash, _) in &files.0 {
            if !matches!(pool.thumbnail(hash, size), Thumbnail::Ready(_)) {
                fail("a thumbnail is not in the cache");
            }
        }
    }
    let per_lookup = lookups.elapsed() / u32::try_from(rounds * files.0.len()).unwrap_or(1);
    println!(
        "{size:>3} px, {threads:>2} threads: {made} thumbnails in {:.2} s, {:.0} per second; \
         a cached lookup {:.1} µs; cache {} KiB",
        elapsed.as_secs_f64(),
        made as f64 / elapsed.as_secs_f64(),
        per_lookup.as_secs_f64() * 1e6,
        pool.cache().total_bytes() / 1024,
    );
    (elapsed, made)
}

fn main() {
    let mut covers = 200_usize;
    let mut side = 1200_u32;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bench" => {}
            "--side" => {
                side = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or_else(|| fail("--side needs a number"));
            }
            n => {
                covers = n
                    .parse()
                    .unwrap_or_else(|_| fail(&format!("not a number: {n}")))
            }
        }
    }
    let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .map_or_else(|| PathBuf::from("target"), Path::to_path_buf);
    let base = target.join("thumbnail-bench");
    let pictures = base.join("pictures");
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&pictures).unwrap_or_else(|e| fail(&e.to_string()));
    let started = Instant::now();
    let mut files = Vec::with_capacity(covers);
    let mut bytes = 0;
    for i in 0..covers {
        let data = picture(side, u32::try_from(i).unwrap_or(0) + 1);
        bytes += data.len();
        let path = pictures.join(format!("{i}.jpg"));
        fs::write(&path, &data).unwrap_or_else(|e| fail(&e.to_string()));
        files.push((CoverHash::of(&data), path));
    }
    eprintln!(
        "{covers} pictures of {side}×{side} ({} KiB on average) written in {:.1} s",
        bytes / covers.max(1) / 1024,
        started.elapsed().as_secs_f64()
    );
    let files = Arc::new(Files(files));
    let all = std::thread::available_parallelism().map_or(4, |n| n.get());
    for size in [ThumbnailSize::LARGE, ThumbnailSize::LARGE_2X] {
        for threads in [2, all] {
            run(&files, &base.join("cache"), threads, size);
        }
    }
    let _ = fs::remove_dir_all(&base);
}
