// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The checks shared by the playlist targets.

use std::path::{Component, Path};

use helikodu_meta::playlists::{
    ParseOptions, ParsedPlaylist, PathStyle, PlaylistKind, PlaylistLimits, WriteOptions,
    parse_playlist, write_playlist,
};

const CHARSETS: [Option<&str>; 6] = [
    None,
    Some("windows-1251"),
    Some("shift_jis"),
    Some("gbk"),
    Some("koi8-r"),
    Some("windows-1252"),
];

const LIMITS: PlaylistLimits = PlaylistLimits {
    max_bytes: 1024 * 1024,
    max_entries: 1000,
    max_line_bytes: 4096,
};

/// Parses `data` (after the charset byte) without saying its format, as the
/// automatic import does, then writes it back in the format it was read as.
#[allow(dead_code)]
pub(crate) fn run_sniffed(data: &[u8]) {
    let Some((&choice, body)) = data.split_first() else {
        return;
    };
    let options = options(choice);
    let Ok(parsed) = parse_playlist(body, None, &options) else {
        return;
    };
    check(&parsed);
    let kind = PlaylistKind::sniff(body);
    let written = write_playlist(
        &parsed.playlist,
        kind,
        &WriteOptions {
            path_style: PathStyle::Absolute,
            base_dir: Some(Path::new(BASE).to_owned()),
        },
    );
    if let Ok(again) = parse_playlist(&written.bytes, Some(kind), &options) {
        check(&again);
    }
}

const BASE: &str = "/music/lists";

fn options(choice: u8) -> ParseOptions {
    ParseOptions {
        base_dir: Some(Path::new(BASE).to_owned()),
        legacy_charset: CHARSETS[usize::from(choice >> 1) % CHARSETS.len()].map(str::to_owned),
        limits: LIMITS,
    }
}

#[allow(dead_code)]
pub(crate) fn run(kind: PlaylistKind, data: &[u8]) {
    let Some((&choice, body)) = data.split_first() else {
        return;
    };
    // M3U and M3U8 share a target: the low bit of the first byte picks one.
    let kind = match kind {
        PlaylistKind::M3u if choice & 1 == 1 => PlaylistKind::M3u8,
        other => other,
    };
    let base = Path::new(BASE);
    let options = options(choice);
    let Ok(parsed) = parse_playlist(body, Some(kind), &options) else {
        return;
    };
    check(&parsed);
    for style in [PathStyle::Relative, PathStyle::Absolute] {
        let written = write_playlist(
            &parsed.playlist,
            kind,
            &WriteOptions {
                path_style: style,
                base_dir: Some(base.to_owned()),
            },
        );
        if let Ok(again) = parse_playlist(&written.bytes, Some(kind), &options) {
            check(&again);
        }
    }
}

fn check(parsed: &ParsedPlaylist) {
    assert!(parsed.playlist.entries.len() <= LIMITS.max_entries);
    assert!(parsed.warnings.len() <= 257);
    for entry in &parsed.playlist.entries {
        for path in entry.local_paths() {
            assert!(path.is_absolute(), "{path:?}");
            assert!(
                path.components()
                    .all(|c| matches!(c, Component::RootDir | Component::Normal(_))),
                "{path:?}"
            );
        }
    }
    for warning in &parsed.warnings {
        let _ = warning.to_string();
    }
}
