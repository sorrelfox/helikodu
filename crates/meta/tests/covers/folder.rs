// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Folder images: name priority, case, disc folders, byte paths, laziness, and
//! choosing between embedded and folder covers.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use helikodu_meta::covers::{
    CoverHash, CoverLimits, CoverOptions, CoverPreference, FolderArtOptions, PictureFormat,
    SkipReason, find_cover, find_folder_covers,
};

use crate::media::{fresh_dir, jpeg, png, webp, with_attached_picture};

fn names(covers: &[helikodu_meta::Cover]) -> Vec<String> {
    covers
        .iter()
        .map(|c| {
            let path = c.source().path().expect("a folder image");
            String::from_utf8_lossy(path.file_name().expect("a name").as_bytes()).into_owned()
        })
        .collect()
}

fn touch(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, data).expect("write a file");
    path
}

#[test]
fn priority_and_case() {
    let dir = fresh_dir("folder-priority");
    let picture = jpeg(8, 8, [1, 2, 3]);
    for name in [
        "AlbumArtSmall.jpg",
        "AlbumArt_{0A1B}_Large.jpg",
        "AlbumArt_{0A1B}_Small.jpg",
        "Blue Train.png",
        "folder.png",
        "front.webp",
        "Cover.JPG",
        "cover.png",
        "random.jpg",
        "back.jpg",
        "cover.gif",
        "cover.txt",
    ] {
        touch(&dir, name, &picture);
    }
    fs::create_dir(dir.join("cover.jpeg")).expect("a folder named like a picture");
    let options = FolderArtOptions {
        album_title: Some("Blue Train".to_owned()),
        ..FolderArtOptions::default()
    };
    let found = find_folder_covers(&dir.join("01.flac"), &options, &CoverLimits::default());
    assert_eq!(
        names(&found.covers),
        [
            "Cover.JPG",
            "cover.png",
            "folder.png",
            "front.webp",
            "AlbumArt_{0A1B}_Large.jpg",
            "Blue Train.png",
            "AlbumArt_{0A1B}_Small.jpg",
            "AlbumArtSmall.jpg",
        ]
    );
    // Nothing was read.
    assert!(found.covers.iter().all(|c| !c.is_loaded()));
}

#[test]
fn configured_names() {
    let dir = fresh_dir("folder-configured");
    let picture = jpeg(8, 8, [1, 2, 3]);
    touch(&dir, "cover.jpg", &picture);
    touch(&dir, "Scan.jpg", &picture);
    let options = FolderArtOptions {
        names: vec!["scan".to_owned(), "cover".to_owned()],
        ..FolderArtOptions::default()
    };
    let found = find_folder_covers(&dir.join("01.flac"), &options, &CoverLimits::default());
    assert_eq!(names(&found.covers), ["Scan.jpg", "cover.jpg"]);
}

#[test]
fn disc_folders_also_search_the_parent() {
    let album = fresh_dir("folder-discs");
    let cd1 = album.join("CD1");
    let cd2 = album.join("Disc 2 (Bonus)");
    let extras = album.join("Extras");
    for d in [&cd1, &cd2, &extras] {
        fs::create_dir(d).expect("a disc folder");
    }
    let picture = jpeg(8, 8, [1, 2, 3]);
    touch(&album, "folder.jpg", &picture);
    touch(&cd2, "AlbumArtSmall.jpg", &picture);
    let limits = CoverLimits::default();
    let options = FolderArtOptions::default();

    let found = find_folder_covers(&cd1.join("01.flac"), &options, &limits);
    assert_eq!(names(&found.covers), ["folder.jpg"]);
    let path = found.covers[0].source().path().expect("a path");
    assert_eq!(path, album.join("folder.jpg"));

    // The disc's own image comes first, even with a worse name.
    let found = find_folder_covers(&cd2.join("01.flac"), &options, &limits);
    assert_eq!(names(&found.covers), ["AlbumArtSmall.jpg", "folder.jpg"]);

    // Not a disc folder: the parent is not searched.
    let found = find_folder_covers(&extras.join("01.flac"), &options, &limits);
    assert!(found.covers.is_empty());

    // Turned off.
    let off = FolderArtOptions {
        disc_parent: false,
        ..FolderArtOptions::default()
    };
    assert!(
        find_folder_covers(&cd1.join("01.flac"), &off, &limits)
            .covers
            .is_empty()
    );

    // No recursion into subfolders.
    let found = find_folder_covers(&album.join("01.flac"), &options, &limits);
    assert_eq!(names(&found.covers), ["folder.jpg"]);
}

#[test]
fn paths_are_bytes() {
    let base = fresh_dir("folder-bytes");
    // A folder name in Latin-1, not valid UTF-8.
    let album = base.join(OsStr::from_bytes(b"Caf\xe9 Tacvba"));
    fs::create_dir(&album).expect("a non-UTF-8 folder");
    let picture = png(8, 8, [1, 2, 3]);
    touch(&album, "cover.png", &picture);
    fs::write(album.join(OsStr::from_bytes(b"cov\xe9r.jpg")), &picture).expect("write");
    let found = find_folder_covers(
        &album.join("01.flac"),
        &FolderArtOptions::default(),
        &CoverLimits::default(),
    );
    assert_eq!(found.covers.len(), 1);
    let path = found.covers[0].source().path().expect("a path");
    assert!(
        path.as_os_str()
            .as_bytes()
            .ends_with(b"Caf\xe9 Tacvba/cover.png")
    );
    assert_eq!(found.covers[0].bytes().expect("bytes"), &picture[..]);
}

#[test]
fn bytes_are_loaded_lazily_and_bounded() {
    let dir = fresh_dir("folder-lazy");
    let picture = webp(16, 16, [9, 8, 7]);
    let big = jpeg(256, 256, [50, 60, 70]);
    touch(&dir, "cover.webp", &picture);
    touch(&dir, "folder.jpg", &big);
    let limits = CoverLimits {
        max_bytes: picture.len(),
        ..CoverLimits::default()
    };
    let found = find_folder_covers(&dir.join("01.flac"), &FolderArtOptions::default(), &limits);
    assert_eq!(names(&found.covers), ["cover.webp"]);
    assert_eq!(found.skipped.len(), 1);
    assert_eq!(found.skipped[0].reason, SkipReason::TooLarge);
    assert_eq!(found.skipped[0].byte_len, big.len() as u64);

    let cover = &found.covers[0];
    assert_eq!(cover.byte_len(), picture.len() as u64);
    assert_eq!(cover.format().expect("format"), PictureFormat::WebP);
    assert!(!cover.is_loaded(), "sniffing reads only the first bytes");
    assert_eq!(cover.hash().expect("hash"), CoverHash::of(&picture));
    assert!(cover.is_loaded());

    // The file grew past the limit after it was found.
    let grown = find_folder_covers(&dir.join("01.flac"), &FolderArtOptions::default(), &limits);
    touch(&dir, "cover.webp", &big);
    assert!(grown.covers[0].bytes().is_err());
}

#[test]
fn find_cover_passes_over_files_that_are_not_pictures() {
    let dir = fresh_dir("folder-not-pictures");
    touch(&dir, "cover.jpg", b"<html>404</html>");
    touch(&dir, "folder.jpg", crate::media::GIF);
    let real = png(12, 12, [3, 3, 3]);
    touch(&dir, "front.jpg", &real); // a PNG with a .jpg name
    let cover = find_cover(&dir.join("missing.flac"), &CoverOptions::default()).expect("a cover");
    assert_eq!(names(std::slice::from_ref(&cover)), ["front.jpg"]);
    assert_eq!(cover.format().expect("format"), PictureFormat::Png);
}

#[test]
fn embedded_or_folder_first() {
    let dir = fresh_dir("folder-preference");
    let embedded = jpeg(32, 32, [200, 0, 0]);
    let folder = jpeg(32, 32, [0, 200, 0]);
    let source = touch(&dir, "source.jpeg", &embedded);
    touch(&dir, "cover.jpg", &folder);
    let Some(track) = with_attached_picture(
        &dir,
        "track.mp3",
        "libmp3lame",
        &source,
        Some("Cover (front)"),
    ) else {
        return;
    };
    let mut options = CoverOptions::default();
    let cover = find_cover(&track, &options).expect("a cover");
    assert!(cover.source().is_embedded());
    assert_eq!(cover.hash().expect("hash"), CoverHash::of(&embedded));

    options.prefer = CoverPreference::Folder;
    let cover = find_cover(&track, &options).expect("a cover");
    assert!(!cover.source().is_embedded());
    assert_eq!(cover.hash().expect("hash"), CoverHash::of(&folder));

    // The same picture embedded and in the folder has the same hash.
    touch(&dir, "cover.jpg", &embedded);
    let cover = find_cover(&track, &options).expect("a cover");
    assert!(!cover.source().is_embedded());
    assert_eq!(cover.hash().expect("hash"), CoverHash::of(&embedded));
}
