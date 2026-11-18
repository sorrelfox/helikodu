// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing a playlist in every format and reading it back from disk.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::Duration;

use helikodu_meta::playlists::{
    EntryHints, EntryLocation, ParseOptions, PathStyle, Playlist, PlaylistEntry, PlaylistKind,
    WriteOptions, WriteWarningKind, read_playlist, write_playlist,
};

use crate::{folder, write};

fn playlist(dir: &std::path::Path) -> Playlist {
    let mut first = PlaylistEntry::local(dir.join("Artist/Album/01 First.flac"));
    first.hints = EntryHints {
        title: Some("First".into()),
        artist: Some("Artist".into()),
        album: Some("Album".into()),
        duration: Some(Duration::from_secs(200)),
        track_number: Some(1),
    };
    let elsewhere = PlaylistEntry::local("/srv/other/Ünïcödé — 02.mp3");
    let latin1 = PlaylistEntry::local(PathBuf::from(OsStr::from_bytes(
        &[dir.as_os_str().as_bytes(), b"/Bj\xf6rk/J\xf3ga.flac"].concat(),
    )));
    Playlist {
        title: Some("Round trip".into()),
        entries: vec![
            first,
            elsewhere,
            latin1,
            PlaylistEntry::new(EntryLocation::Remote("http://radio.invalid/live".into())),
            PlaylistEntry::new(EntryLocation::Foreign(b"D:/Music/x.mp3".to_vec())),
        ],
    }
}

#[test]
fn every_format_round_trips() {
    let dir = folder("round-trip");
    let original = playlist(&dir);
    for kind in PlaylistKind::ALL {
        for style in [PathStyle::Relative, PathStyle::Absolute] {
            let options = WriteOptions {
                path_style: style,
                base_dir: Some(dir.clone()),
            };
            let written = write_playlist(&original, kind, &options);
            let path = write(
                &dir,
                &format!("list-{style:?}.{}", kind.extension()),
                &written.bytes,
            );
            let back = read_playlist(&path, &ParseOptions::default()).unwrap();
            assert_eq!(back.kind, kind);
            let entries = &back.playlist.entries;
            assert_eq!(entries.len(), original.entries.len(), "{kind} {style:?}");
            for (index, (read, wrote)) in entries.iter().zip(&original.entries).enumerate() {
                if kind == PlaylistKind::M3u8 && index == 2 {
                    // M3U8 cannot carry the Latin-1 bytes and says so.
                    assert_eq!(written.warnings[0].entry, 2);
                    assert_eq!(written.warnings[0].kind, WriteWarningKind::NotUtf8);
                    continue;
                }
                assert!(
                    read.local_paths()
                        .any(|p| Some(p) == wrote.location.local_path())
                        || read.location == wrote.location,
                    "{kind} {style:?}: {:?} read back as {:?}",
                    wrote.location,
                    read.location
                );
            }
            let hints = &entries[0].hints;
            assert_eq!(hints.title.as_deref(), Some("First"), "{kind}");
            assert_eq!(hints.duration, Some(Duration::from_secs(200)), "{kind}");
            assert_eq!(hints.artist.as_deref(), Some("Artist"), "{kind}");
            // PLS has neither an album nor a playlist title.
            if kind != PlaylistKind::Pls {
                assert_eq!(hints.album.as_deref(), Some("Album"), "{kind}");
                assert_eq!(back.playlist.title.as_deref(), Some("Round trip"), "{kind}");
            }
        }
    }
}

#[test]
fn relative_paths_are_relative() {
    let dir = folder("relative");
    let original = playlist(&dir);
    let written = write_playlist(
        &original,
        PlaylistKind::M3u8,
        &WriteOptions {
            path_style: PathStyle::Relative,
            base_dir: Some(dir.clone()),
        },
    );
    let text = String::from_utf8_lossy(&written.bytes);
    assert!(text.contains("\nArtist/Album/01 First.flac\n"), "{text}");
    assert!(text.contains("\n/srv/other/Ünïcödé — 02.mp3\n"), "{text}");
    let written = write_playlist(&original, PlaylistKind::M3u8, &WriteOptions::default());
    let text = String::from_utf8_lossy(&written.bytes);
    assert!(text.contains(&format!("\n{}/Artist/Album/01 First.flac\n", dir.display())));
}
