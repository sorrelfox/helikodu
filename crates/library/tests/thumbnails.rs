// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The thumbnail cache and the artwork pool: thumbnails made on request, reused,
//! evicted at the size limit and removed with their covers; the queue newest first,
//! bounded and cancellable.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[path = "support/tracks.rs"]
mod support;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helikodu_library::{
    ArtworkOptions, ArtworkPool, CoverHash, CoverLocation, CoverLocator, CoverOrigin,
    CoverReadError, CoverReport, Library, RootId, Thumbnail, ThumbnailCache, ThumbnailCacheOptions,
    ThumbnailEvent, ThumbnailSize, TrackCovers,
};
use helikodu_meta::covers::{CoverLimits, PictureFormat, RgbaPicture, decode_picture};
use support::{library, tagged};

const WAIT: Duration = Duration::from_secs(20);

/// A fresh, empty folder named `name` for one test.
fn folder(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("library-thumbnails")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// An encoded picture of `width` × `height` in one colour, PNG when `alpha` is not
/// 255, JPEG otherwise.
fn picture(width: u32, height: u32, rgb: [u8; 3], alpha: u8) -> Vec<u8> {
    let pixel = [rgb[0], rgb[1], rgb[2], alpha];
    let pixels = pixel.repeat((width * height) as usize);
    let picture = RgbaPicture::new(width, height, pixels).unwrap();
    if alpha == 255 {
        picture.encode_jpeg(90).unwrap()
    } else {
        picture.encode_png().unwrap()
    }
}

/// Writes `bytes` as the image file `path` and returns its report.
fn image_file(path: &Path, bytes: &[u8], width: u32, height: u32) -> CoverReport {
    fs::write(path, bytes).unwrap();
    CoverReport {
        hash: CoverHash::of(bytes),
        origin: CoverOrigin::Folder {
            path: path.to_path_buf(),
        },
        format: PictureFormat::sniff(bytes).unwrap(),
        width,
        height,
        byte_len: bytes.len() as u64,
    }
}

/// A track in `album` whose folder has the image `cover`.
fn with_folder_cover(
    root: RootId,
    path: &str,
    album: &str,
    cover: &CoverReport,
) -> helikodu_library::TrackRecord {
    let mut record = tagged(root, path, path, path, "Artist", album);
    record.covers = Some(TrackCovers {
        embedded: None,
        folder: Some(cover.clone()),
    });
    record
}

fn cache(dir: &Path, max_bytes: u64) -> Arc<ThumbnailCache> {
    Arc::new(
        ThumbnailCache::open(&ThumbnailCacheOptions {
            dir: dir.to_path_buf(),
            max_bytes,
        })
        .unwrap(),
    )
}

fn pool(
    cache: Arc<ThumbnailCache>,
    locator: Arc<dyn CoverLocator>,
    options: ArtworkOptions,
) -> (ArtworkPool, Receiver<ThumbnailEvent>) {
    let (sender, events) = mpsc::channel();
    let sender = Mutex::new(sender);
    let pool = ArtworkPool::start(cache, locator, options, move |event| {
        let _ = sender.lock().unwrap().send(event);
    })
    .unwrap();
    (pool, events)
}

#[test]
fn thumbnails_are_made_cached_and_reused() {
    let dir = folder("made");
    let images = dir.join("images");
    fs::create_dir_all(&images).unwrap();
    let (library, root) = library();
    let wide = image_file(
        &images.join("cover.jpg"),
        &picture(1000, 500, [200, 40, 40], 255),
        1000,
        500,
    );
    let clear = image_file(
        &images.join("folder.png"),
        &picture(300, 300, [0, 0, 255], 100),
        300,
        300,
    );
    library
        .upsert_tracks(vec![
            with_folder_cover(root, "/music/A/1.flac", "A", &wide),
            with_folder_cover(root, "/music/A/2.flac", "A", &wide),
            with_folder_cover(root, "/music/B/1.flac", "B", &clear),
        ])
        .unwrap();
    assert_eq!(
        library.cover_hashes().unwrap(),
        HashSet::from([wide.hash, clear.hash])
    );

    let cache = cache(&dir.join("cache"), 1 << 30);
    let library: Arc<Library> = Arc::new(library);
    let (pool, events) = pool(
        Arc::clone(&cache),
        library.clone(),
        ArtworkOptions::default(),
    );

    assert_eq!(
        pool.thumbnail(&wide.hash, ThumbnailSize::LARGE),
        Thumbnail::Scheduled
    );
    let event = events.recv_timeout(WAIT).unwrap();
    assert_eq!((event.hash, event.size), (wide.hash, ThumbnailSize::LARGE));
    let path = event.result.unwrap();
    assert_eq!(
        path,
        dir.join("cache/192")
            .join(format!("{}.jpg", wide.hash.to_hex()))
    );
    let thumbnail = decode_picture(&fs::read(&path).unwrap(), &CoverLimits::default()).unwrap();
    assert_eq!((thumbnail.width(), thumbnail.height()), (192, 96));

    // Reused from the cache, without another event.
    assert_eq!(
        pool.thumbnail(&wide.hash, ThumbnailSize::LARGE),
        Thumbnail::Ready(path.clone())
    );
    assert!(events.recv_timeout(Duration::from_millis(200)).is_err());

    // Transparency keeps PNG; a small picture is not enlarged.
    assert_eq!(
        pool.thumbnail(&clear.hash, ThumbnailSize::LARGE_2X),
        Thumbnail::Scheduled
    );
    let path = events.recv_timeout(WAIT).unwrap().result.unwrap();
    assert_eq!(path.extension().unwrap(), "png");
    let thumbnail = decode_picture(&fs::read(&path).unwrap(), &CoverLimits::default()).unwrap();
    assert_eq!((thumbnail.width(), thumbnail.height()), (300, 300));
    assert!(!thumbnail.is_opaque());

    // The album covers and the albums' summaries name the covers.
    let albums = library.albums(&Default::default()).unwrap();
    let covers: HashSet<Option<CoverHash>> = albums.items.iter().map(|a| a.cover).collect();
    assert_eq!(covers, HashSet::from([Some(wide.hash), Some(clear.hash)]));
    assert_eq!(cache.len(), 2);

    // A new cache over the same folder finds the files.
    drop(pool);
    let reopened = ThumbnailCache::open(&ThumbnailCacheOptions::new(dir.join("cache"))).unwrap();
    assert_eq!(reopened.len(), 2);
    assert_eq!(reopened.total_bytes(), cache.total_bytes());
}

#[test]
fn a_cover_that_is_gone_is_unavailable_until_failures_are_forgotten() {
    let dir = folder("gone");
    let (library, root) = library();
    let cover = image_file(
        &dir.join("cover.jpg"),
        &picture(64, 64, [1, 2, 3], 255),
        64,
        64,
    );
    library
        .upsert_track(with_folder_cover(root, "/music/A/1.flac", "A", &cover))
        .unwrap();
    // The picture changed on disk: its bytes no longer have the recorded hash.
    fs::write(dir.join("cover.jpg"), picture(64, 64, [9, 9, 9], 255)).unwrap();

    let cache = cache(&dir.join("cache"), 1 << 30);
    let (pool, events) = pool(cache, Arc::new(library), ArtworkOptions::default());
    assert_eq!(
        pool.thumbnail(&cover.hash, ThumbnailSize::SMALL),
        Thumbnail::Scheduled
    );
    let event = events.recv_timeout(WAIT).unwrap();
    assert!(
        matches!(event.result, Err(CoverReadError::NotFound)),
        "{event:?}"
    );
    assert_eq!(
        pool.thumbnail(&cover.hash, ThumbnailSize::SMALL),
        Thumbnail::Unavailable
    );
    pool.forget_failures();
    assert_eq!(
        pool.thumbnail(&cover.hash, ThumbnailSize::SMALL),
        Thumbnail::Scheduled
    );

    // A cover the library does not know fails too.
    let unknown = CoverHash::of(b"unknown");
    assert_eq!(
        pool.thumbnail(&unknown, ThumbnailSize::SMALL),
        Thumbnail::Scheduled
    );
    let mut failed = 0;
    while failed < 2 {
        let event = events.recv_timeout(WAIT).unwrap();
        assert!(event.result.is_err());
        failed += 1;
    }
}

#[test]
fn a_picture_that_cannot_be_decoded_fails_alone() {
    let dir = folder("hostile");
    // A PNG signature over garbage, and a picture over the pixel limit.
    let mut garbage = b"\x89PNG\r\n\x1a\n".to_vec();
    garbage.extend_from_slice(&[0x41; 300]);
    fs::write(dir.join("bad.png"), &garbage).unwrap();
    let large = picture(400, 400, [5, 5, 5], 255);
    fs::write(dir.join("large.jpg"), &large).unwrap();
    let locator = Arc::new(Files(vec![
        (CoverHash::of(&garbage), dir.join("bad.png")),
        (CoverHash::of(&large), dir.join("large.jpg")),
    ]));
    let options = ArtworkOptions {
        limits: CoverLimits {
            max_pixels: 100_000,
            ..CoverLimits::default()
        },
        ..ArtworkOptions::default()
    };
    let (pool, events) = pool(cache(&dir.join("cache"), 1 << 30), locator, options);
    pool.thumbnail(&CoverHash::of(&garbage), ThumbnailSize::SMALL);
    pool.thumbnail(&CoverHash::of(&large), ThumbnailSize::SMALL);
    for _ in 0..2 {
        let event = events.recv_timeout(WAIT).unwrap();
        assert!(
            matches!(event.result, Err(CoverReadError::Picture(_))),
            "{event:?}"
        );
    }
    assert!(pool.cache().is_empty());
}

/// Covers in image files, by hash.
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

/// Covers in image files; the first lookup waits until it is let go, and every
/// lookup is recorded.
struct Gate {
    files: Files,
    order: Mutex<Vec<CoverHash>>,
    release: Mutex<Option<Receiver<()>>>,
    started: Mutex<mpsc::Sender<()>>,
}

impl CoverLocator for Gate {
    fn locate(&self, hash: &CoverHash) -> Vec<CoverLocation> {
        self.order.lock().unwrap().push(*hash);
        let waiting = self.release.lock().unwrap().take();
        if let Some(release) = waiting {
            let _ = self.started.lock().unwrap().send(());
            let _ = release.recv_timeout(WAIT);
        }
        self.files.locate(hash)
    }
}

#[test]
fn the_newest_requests_go_first_and_cancelled_ones_never() {
    let dir = folder("order");
    let mut files = Vec::new();
    for (i, name) in ["a", "b", "c", "d", "e"].iter().enumerate() {
        let bytes = picture(32, 32, [i as u8 * 40, 0, 0], 255);
        let path = dir.join(format!("{name}.jpg"));
        fs::write(&path, &bytes).unwrap();
        files.push((CoverHash::of(&bytes), path));
    }
    let hashes: Vec<CoverHash> = files.iter().map(|(h, _)| *h).collect();
    let (release, gate) = mpsc::channel();
    let (started_sender, started) = mpsc::channel();
    let locator = Arc::new(Gate {
        files: Files(files),
        order: Mutex::new(Vec::new()),
        release: Mutex::new(Some(gate)),
        started: Mutex::new(started_sender),
    });
    let options = ArtworkOptions {
        threads: 1,
        queue_limit: 3,
        ..ArtworkOptions::default()
    };
    let (pool, events) = pool(cache(&dir.join("cache"), 1 << 30), locator.clone(), options);
    let [a, b, c, d, e] = hashes.as_slice() else {
        panic!()
    };
    let size = ThumbnailSize::SMALL;
    // The one thread is held on `a`.
    pool.thumbnail(a, size);
    started.recv_timeout(WAIT).unwrap();
    // `b` falls out of the queue of three; `d` is asked for again and becomes the
    // newest; `c` is cancelled.
    for hash in [b, c, d, e] {
        assert_eq!(pool.thumbnail(hash, size), Thumbnail::Scheduled);
    }
    pool.thumbnail(d, size);
    assert_eq!(pool.waiting(), 3);
    assert!(pool.cancel(c, size));
    assert!(!pool.cancel(b, size));
    pool.retain(|_, _| true);
    release.send(()).unwrap();
    let made: Vec<CoverHash> = (0..3)
        .map(|_| {
            let event = events.recv_timeout(WAIT).unwrap();
            event.result.unwrap();
            event.hash
        })
        .collect();
    assert_eq!(made, [*a, *d, *e]);
    assert_eq!(*locator.order.lock().unwrap(), [*a, *d, *e]);
    assert!(events.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn the_least_recently_used_thumbnails_are_evicted_at_the_limit() {
    let dir = folder("evict");
    let cache = cache(&dir, 3500);
    let hashes: Vec<CoverHash> = (0..5).map(|i: u8| CoverHash::of(&[i])).collect();
    let size = ThumbnailSize::SMALL;
    for hash in &hashes[..3] {
        cache.insert(hash, size, &[7; 1000], "jpg").unwrap();
    }
    assert_eq!(cache.total_bytes(), 3000);
    // Using the first makes the second the least recently used.
    assert!(cache.get(&hashes[0], size).is_some());
    cache.insert(&hashes[3], size, &[7; 1000], "jpg").unwrap();
    assert!(cache.total_bytes() <= 3500);
    assert!(cache.contains(&hashes[0], size));
    assert!(!cache.contains(&hashes[1], size));
    assert!(cache.contains(&hashes[2], size));
    assert!(cache.contains(&hashes[3], size));
    let file = |hash: &CoverHash| dir.join("48").join(format!("{}.jpg", hash.to_hex()));
    assert!(!file(&hashes[1]).exists());
    assert!(file(&hashes[0]).exists());

    // Lowering the limit evicts at once; a thumbnail larger than the limit is not
    // kept.
    let removed = cache.set_max_bytes(1500);
    assert_eq!(removed.files, 2);
    assert_eq!(cache.len(), 1);
    assert!(cache.contains(&hashes[3], size));
    let refused = cache
        .insert(&hashes[4], size, &[7; 2000], "jpg")
        .unwrap_err();
    assert_eq!(refused.kind(), std::io::ErrorKind::FileTooLarge);
    assert!(!cache.contains(&hashes[4], size));
    assert!(cache.contains(&hashes[3], size));
    assert!(cache.total_bytes() <= 1500);

    // A temporary file left behind is removed when the cache opens; the order of
    // use survives.
    fs::write(dir.join("48/.tmp-left-behind"), b"x").unwrap();
    fs::write(dir.join("48/not-a-thumbnail.txt"), b"x").unwrap();
    drop(cache);
    let reopened = ThumbnailCache::open(&ThumbnailCacheOptions {
        dir: dir.clone(),
        max_bytes: 1500,
    })
    .unwrap();
    assert_eq!(reopened.len(), 1);
    assert!(!dir.join("48/.tmp-left-behind").exists());
    assert!(dir.join("48/not-a-thumbnail.txt").exists());
}

#[test]
fn thumbnails_of_covers_no_longer_in_the_library_are_removed() {
    let dir = folder("unreferenced");
    let (library, root) = library();
    let one = image_file(
        &dir.join("one.jpg"),
        &picture(40, 40, [1, 1, 1], 255),
        40,
        40,
    );
    let two = image_file(
        &dir.join("two.jpg"),
        &picture(40, 40, [2, 2, 2], 255),
        40,
        40,
    );
    let mut embedded = tagged(root, "/music/C/1.flac", "c1", "T", "Artist", "C");
    embedded.covers = Some(TrackCovers {
        embedded: Some(CoverReport {
            origin: CoverOrigin::Embedded {
                tag: helikodu_meta::covers::EmbeddedTag::FlacPicture,
                picture_type: None,
                index: 0,
            },
            ..two.clone()
        }),
        folder: None,
    });
    library
        .upsert_tracks(vec![
            with_folder_cover(root, "/music/A/1.flac", "A", &one),
            embedded,
        ])
        .unwrap();
    let cache = cache(&dir.join("cache"), 1 << 30);
    for hash in [one.hash, two.hash] {
        for size in ThumbnailSize::ALL {
            cache.insert(&hash, size, b"thumbnail", "jpg").unwrap();
        }
    }
    let referenced = library.cover_hashes().unwrap();
    assert_eq!(cache.remove_unreferenced(&referenced).files, 0);

    // The embedded cover goes with its track; the folder image stays with its
    // folder.
    library.remove_file("/music/C/1.flac").unwrap();
    let referenced = library.cover_hashes().unwrap();
    assert_eq!(referenced, HashSet::from([one.hash]));
    let removed = cache.remove_unreferenced(&referenced);
    assert_eq!(removed.files, 4);
    assert_eq!(cache.len(), 4);
    assert!(!cache.contains(&two.hash, ThumbnailSize::SMALL));
    assert_eq!(cache.clear().files, 4);
    assert!(cache.is_empty());
}
