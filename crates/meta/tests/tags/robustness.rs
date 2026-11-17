// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Damaged input and file names that are not UTF-8.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;

use helikodu_meta::tags::{ReadOptions, read_tags};

use crate::media::{dir, encode, metadata};

#[test]
fn junk_and_empty_files_are_errors() {
    let junk = dir().join("junk.mp3");
    let bytes: Vec<u8> = (0..4096u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    fs::write(&junk, bytes).expect("write");
    assert!(read_tags(&junk, &ReadOptions::default()).is_err());

    let empty = dir().join("empty.flac");
    fs::write(&empty, b"").expect("write");
    assert!(read_tags(&empty, &ReadOptions::default()).is_err());

    assert!(read_tags(dir().join("does-not-exist.ogg"), &ReadOptions::default()).is_err());
}

#[test]
fn truncated_file_does_not_panic() {
    let Some(path) = encode(
        "truncated-src.flac",
        "flac",
        &metadata(&[("title", "Cut")]),
        "flac",
    ) else {
        return;
    };
    let bytes = fs::read(&path).expect("read");
    for len in [4, 42, 200, bytes.len() / 2] {
        let cut = dir().join(format!("truncated-{len}.flac"));
        fs::write(&cut, &bytes[..len.min(bytes.len())]).expect("write");
        // Either answer is fine; it must not panic.
        let _ = read_tags(&cut, &ReadOptions::default());
    }
}

#[test]
fn file_names_that_are_not_utf8() {
    let Some(path) = encode(
        "named.flac",
        "flac",
        &metadata(&[("title", "Named")]),
        "flac",
    ) else {
        return;
    };
    let odd = dir().join(OsStr::from_bytes(b"caf\xe9 \xff.flac"));
    fs::copy(&path, &odd).expect("copy");
    let file = read_tags(&odd, &ReadOptions::default()).expect("read");
    assert_eq!(file.tags.title.as_deref(), Some("Named"));
}
