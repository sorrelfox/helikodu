// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Covers and loudness values through the scanner, with real worker processes over
//! files made with ffmpeg: embedded pictures and folder images found, stored once
//! per picture and given to the albums; hostile pictures passed over; thumbnails
//! made from what the scan found; the ReplayGain columns filled; and a worker of
//! another protocol version refused.

#![forbid(unsafe_code)]
// The helpers unwrap like the tests: a panic is how a test fails.
#![allow(clippy::unwrap_used)]

#[allow(dead_code)]
#[path = "support/scanning.rs"]
mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use helikodu_library::{
    AlbumQuery, ArtworkOptions, ArtworkPool, CancelToken, CoverHash, CoverLocation, DatabaseConfig,
    Library, ScanError, ScanOptions, ScanSummary, Thumbnail, ThumbnailCache, ThumbnailCacheOptions,
    ThumbnailSize,
};
use helikodu_meta::covers::RgbaPicture;
use support::{flac, folder, have_ffmpeg, worker_program, write};

fn options(root: &Path) -> ScanOptions {
    let mut options = ScanOptions::new([root]);
    options.workers.program = Some(worker_program());
    options.workers.count = 2;
    options
}

fn scan(library: &Library, options: &ScanOptions) -> ScanSummary {
    library.scan(options, None, &CancelToken::new()).unwrap()
}

fn png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let pixels = [rgb[0], rgb[1], rgb[2], 255].repeat((width * height) as usize);
    RgbaPicture::new(width, height, pixels)
        .unwrap()
        .encode_png()
        .unwrap()
}

/// The CRC-32 of PNG chunks.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A well-formed PNG header that declares `width` × `height` pixels, and no pixels.
fn png_claiming(width: u32, height: u32) -> Vec<u8> {
    let mut chunk = b"IHDR".to_vec();
    chunk.extend_from_slice(&width.to_be_bytes());
    chunk.extend_from_slice(&height.to_be_bytes());
    chunk.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend_from_slice(&13_u32.to_be_bytes());
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&crc32(&chunk).to_be_bytes());
    out
}

/// Adds a FLAC `PICTURE` block (a front cover) holding `picture` to the FLAC file at
/// `path`, after its `STREAMINFO` block.
fn embed_picture(path: &Path, picture: &[u8]) {
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[..4], b"fLaC");
    let streaminfo_len = u32::from_be_bytes([0, bytes[5], bytes[6], bytes[7]]) as usize;
    let first_end = 8 + streaminfo_len;
    let streaminfo_last = bytes[4] & 0x80 != 0;

    let mut body = Vec::new();
    body.extend_from_slice(&3_u32.to_be_bytes()); // front cover
    let mime = b"image/png";
    body.extend_from_slice(&(mime.len() as u32).to_be_bytes());
    body.extend_from_slice(mime);
    body.extend_from_slice(&0_u32.to_be_bytes()); // no description
    for value in [0_u32, 0, 0, 0] {
        body.extend_from_slice(&value.to_be_bytes());
    }
    body.extend_from_slice(&(picture.len() as u32).to_be_bytes());
    body.extend_from_slice(picture);
    let len = (body.len() as u32).to_be_bytes();

    let mut out = b"fLaC".to_vec();
    out.push(bytes[4] & 0x7f);
    out.extend_from_slice(&bytes[5..first_end]);
    out.push(6 | if streaminfo_last { 0x80 } else { 0 });
    out.extend_from_slice(&len[1..]);
    out.extend_from_slice(&body);
    out.extend_from_slice(&bytes[first_end..]);
    fs::write(path, out).unwrap();
}

fn count(library: &Library, sql: &'static str) -> i64 {
    library
        .database()
        .read(|c| c.query_row(sql, [], |r| r.get(0)))
        .unwrap()
}

#[test]
fn covers_are_found_stored_once_and_given_to_albums() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("covers");
    let front = png(600, 600, [180, 30, 30]);
    let other = png(500, 400, [30, 180, 30]);
    let folder_art = png(300, 300, [30, 30, 180]);

    // Album A: three tracks with the same embedded front cover, one with another.
    for (n, picture) in [(1, &front), (2, &front), (3, &other), (4, &front)] {
        let path = root.join(format!("A/{n:02}.flac"));
        flac(
            &path,
            440,
            1,
            &[
                ("title", &format!("A{n}")),
                ("artist", "X"),
                ("album", "Album A"),
                ("track", &n.to_string()),
            ],
        );
        embed_picture(&path, picture);
    }
    // Album B: no embedded pictures, a folder image beside discs in their folders.
    for disc in [1, 2] {
        flac(
            root.join(format!("B/CD{disc}/01.flac")),
            440,
            1,
            &[
                ("title", "B"),
                ("artist", "Y"),
                ("album", "Album B"),
                ("disc", &disc.to_string()),
            ],
        );
    }
    write(root.join("B/folder.png"), &folder_art);
    // Album C: the same front cover as album A, as a folder image.
    flac(
        root.join("C/01.flac"),
        440,
        1,
        &[("title", "C"), ("artist", "Z"), ("album", "Album C")],
    );
    write(root.join("C/cover.png"), &front);

    let library = Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap());
    let summary = scan(&library, &options(&root));
    assert_eq!(summary.failed, 0, "{summary:?}");
    assert_eq!(summary.added, 7);

    // Three pictures, each stored once.
    assert_eq!(count(&library, "SELECT count(*) FROM covers"), 3);
    let hashes = library.cover_hashes().unwrap();
    for picture in [&front, &other, &folder_art] {
        assert!(hashes.contains(&CoverHash::of(picture)));
    }
    assert_eq!(
        count(
            &library,
            "SELECT count(*) FROM tracks WHERE cover_id IS NOT NULL"
        ),
        4
    );
    let (width, format): (i64, String) = library
        .database()
        .read(|c| {
            c.query_row(
                "SELECT width, format FROM covers WHERE source = 0 AND width = 500",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .unwrap();
    assert_eq!((width, format.as_str()), (500, "png"));

    // The album covers: the most common embedded one; the folder image of the
    // album above its disc folders; a folder image equal to another album's.
    let albums = library.albums(&AlbumQuery::default()).unwrap();
    let cover_of = |title: &str| {
        albums
            .items
            .iter()
            .find(|a| a.title.as_deref() == Some(title))
            .unwrap()
            .cover
    };
    assert_eq!(cover_of("Album A"), Some(CoverHash::of(&front)));
    assert_eq!(cover_of("Album B"), Some(CoverHash::of(&folder_art)));
    assert_eq!(cover_of("Album C"), Some(CoverHash::of(&front)));
    let third = library
        .track_at(&root.join("A/03.flac"), 0)
        .unwrap()
        .unwrap();
    assert_eq!(
        library.track_cover(third).unwrap().unwrap().hash,
        CoverHash::of(&other)
    );
    let disc_two = library
        .track_at(&root.join("B/CD2/01.flac"), 0)
        .unwrap()
        .unwrap();
    let cover = library.track_cover(disc_two).unwrap().unwrap();
    assert_eq!(cover.hash, CoverHash::of(&folder_art));
    assert_eq!((cover.width, cover.height), (Some(300), Some(300)));

    // Where the front cover can be read: embedded in album A's files, and the
    // folder image of album C.
    let locations = library.cover_locations(&CoverHash::of(&front)).unwrap();
    assert!(locations.contains(&CoverLocation::File {
        path: root.join("C/cover.png")
    }));
    assert!(
        locations
            .iter()
            .any(|l| matches!(l, CoverLocation::Embedded { file, index: 0 } if file.starts_with(root.join("A"))))
    );

    // A thumbnail from an embedded picture, read again from its file.
    let cache_dir = folder("covers-cache");
    let cache = Arc::new(ThumbnailCache::open(&ThumbnailCacheOptions::new(&cache_dir)).unwrap());
    let (sender, events) = mpsc::channel();
    let sender = std::sync::Mutex::new(sender);
    let pool = ArtworkPool::start(
        cache,
        library.clone(),
        ArtworkOptions::default(),
        move |e| {
            let _ = sender.lock().unwrap().send(e);
        },
    )
    .unwrap();
    let hash = CoverHash::of(&other);
    assert_eq!(
        pool.thumbnail(&hash, ThumbnailSize::SMALL),
        Thumbnail::Scheduled
    );
    let event = events.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(event.result.unwrap().is_file());

    // An unchanged rescan reads nothing; after album A's front pictures are
    // removed, the other picture becomes its cover and the front cover stays for
    // album C's folder.
    assert_eq!(scan(&library, &options(&root)).read, 0);
    for n in [1, 2, 4] {
        let path = root.join(format!("A/{n:02}.flac"));
        flac(
            &path,
            440,
            1,
            &[
                ("title", &format!("A{n}")),
                ("artist", "X"),
                ("album", "Album A"),
                ("track", &n.to_string()),
            ],
        );
    }
    scan(&library, &options(&root));
    let albums = library.albums(&AlbumQuery::default()).unwrap();
    let a = albums
        .items
        .iter()
        .find(|a| a.title.as_deref() == Some("Album A"))
        .unwrap();
    assert_eq!(a.cover, Some(CoverHash::of(&other)));
    assert_eq!(count(&library, "SELECT count(*) FROM covers"), 3);
}

#[test]
fn hostile_pictures_do_not_break_the_scan() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("hostile-covers");
    let mut garbage = b"\x89PNG\r\n\x1a\n".to_vec();
    garbage.extend_from_slice(&[0x42; 4000]);
    let cases: [(&str, Vec<u8>); 3] = [
        ("garbage", garbage),
        ("huge", png_claiming(100_000, 100_000)),
        ("gif", b"GIF89a\x01\x00\x01\x00\x00\x00\x00;".to_vec()),
    ];
    for (name, picture) in &cases {
        let path = root.join(format!("{name}/01.flac"));
        flac(&path, 440, 1, &[("title", name), ("album", name)]);
        embed_picture(&path, picture);
    }
    // A truncated JPEG as the folder image, with a good one after it.
    write(
        root.join("garbage/cover.jpg"),
        b"\xff\xd8\xff\xe0\x00\x10JFIF",
    );
    let good = png(16, 16, [1, 2, 3]);
    write(root.join("garbage/folder.png"), &good);
    write(root.join("huge/cover.png"), png_claiming(70_000, 70_000));

    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let summary = scan(&library, &options(&root));
    assert_eq!(summary.failed, 0, "{summary:?}");
    assert_eq!(summary.added, 3);
    assert_eq!(
        count(
            &library,
            "SELECT count(*) FROM tracks WHERE cover_id IS NOT NULL"
        ),
        0
    );
    let hashes = library.cover_hashes().unwrap();
    assert_eq!(hashes.len(), 1);
    assert!(hashes.contains(&CoverHash::of(&good)));
}

#[test]
fn covers_are_not_looked_for_when_turned_off() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("no-covers");
    let path = root.join("A/01.flac");
    flac(&path, 440, 1, &[("title", "T"), ("album", "A")]);
    embed_picture(&path, &png(8, 8, [1, 1, 1]));
    write(root.join("A/cover.png"), png(8, 8, [2, 2, 2]));
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let mut options = options(&root);
    options.covers = false;
    scan(&library, &options);
    assert_eq!(count(&library, "SELECT count(*) FROM covers"), 0);
}

#[test]
fn the_loudness_columns_are_filled_from_the_tags() {
    if !have_ffmpeg() {
        return;
    }
    let root = folder("loudness");
    flac(
        root.join("rg.flac"),
        440,
        1,
        &[
            ("title", "RG"),
            ("REPLAYGAIN_TRACK_GAIN", "-6.50 dB"),
            ("REPLAYGAIN_TRACK_PEAK", "0.912345"),
            ("REPLAYGAIN_ALBUM_GAIN", "-7.25 dB"),
            ("REPLAYGAIN_ALBUM_PEAK", "0.990000"),
        ],
    );
    flac(root.join("none.flac"), 440, 1, &[("title", "None")]);
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    scan(&library, &options(&root));
    let values = |name: &str| -> [Option<f64>; 4] {
        let track = library.track_at(&root.join(name), 0).unwrap().unwrap();
        let id = i64::try_from(track.get()).unwrap();
        library
            .database()
            .read(|c| {
                c.query_row(
                    "SELECT rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak
                     FROM tracks WHERE id = ?1",
                    [id],
                    |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?]),
                )
            })
            .unwrap()
    };
    assert_eq!(
        values("rg.flac"),
        [Some(-6.5), Some(0.912345), Some(-7.25), Some(0.99)]
    );
    assert_eq!(values("none.flac"), [None; 4]);
}

#[test]
fn a_worker_of_another_protocol_version_is_refused() {
    let root = folder("old-worker");
    write(root.join("a.flac"), [0_u8; 64]);
    // A worker that greets with version 1 and then waits.
    let program = folder("old-worker-program").join("worker");
    write(
        &program,
        "#!/bin/sh\nprintf '\\013\\000\\000\\000\\001HKSW\\001\\000\\000\\000\\000\\000'\ncat >/dev/null\n",
    );
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let mut options = ScanOptions::new([&root]);
    options.workers.program = Some(program);
    options.workers.count = 1;
    match library.scan(&options, None, &CancelToken::new()) {
        Err(ScanError::WorkerVersion { found, expected }) => {
            assert_eq!(found, 1);
            assert_eq!(expected, helikodu_meta::worker::PROTOCOL_VERSION);
            assert_eq!(expected, 3);
        }
        other => panic!("{other:?}"),
    }
}
