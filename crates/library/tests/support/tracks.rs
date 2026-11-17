// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Building test libraries through the public API.

#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::time::Duration;

use helikodu_library::{
    AudioProperties, DatabaseConfig, FileStamp, Library, RootId, TrackRecord, TrackTags,
};

/// An empty library in memory with one root, `/music`.
pub fn library() -> (Library, RootId) {
    let library = Library::open(&DatabaseConfig::in_memory()).unwrap();
    let root = library.add_root("/music").unwrap();
    (library, root)
}

/// A track at `path` whose file content is identified by `content`.
pub fn record(root: RootId, path: &str, content: &str) -> TrackRecord {
    let mut record = TrackRecord::new(
        root,
        path,
        FileStamp {
            size: content.len() as u64,
            mtime_ns: Some(1),
            inode: None,
            quick_hash: content.as_bytes().to_vec(),
        },
    );
    record.audio = AudioProperties {
        duration: Some(Duration::from_secs(200)),
        sample_rate: Some(44_100),
        channels: Some(2),
        ..AudioProperties::default()
    };
    record
}

/// A track with a title, one artist and an album.
pub fn tagged(
    root: RootId,
    path: &str,
    content: &str,
    title: &str,
    artist: &str,
    album: &str,
) -> TrackRecord {
    let mut record = record(root, path, content);
    record.tags = TrackTags {
        title: Some(title.to_owned()),
        artists: vec![artist.to_owned()],
        album: Some(album.to_owned()),
        ..TrackTags::default()
    };
    record
}
