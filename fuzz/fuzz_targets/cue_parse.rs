// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parses arbitrary bytes as a CUE sheet and resolves its virtual tracks. The
//! first byte picks the legacy charset the user is taken to have chosen; the rest
//! is the sheet. Any panic is a bug, and so is a track whose file leaves the
//! sheet's folder or whose span ends before it starts.

#![no_main]

use std::path::Path;

use helikodu_meta::cue::{CueOptions, CueSheet};
use libfuzzer_sys::fuzz_target;

const CHARSETS: [Option<&str>; 6] = [
    None,
    Some("windows-1251"),
    Some("shift_jis"),
    Some("gbk"),
    Some("koi8-r"),
    Some("windows-1252"),
];

fuzz_target!(|data: &[u8]| {
    let Some((&choice, sheet)) = data.split_first() else {
        return;
    };
    let options = CueOptions {
        legacy_charset: CHARSETS[usize::from(choice) % CHARSETS.len()].map(str::to_owned),
    };
    let Ok(sheet) = CueSheet::parse(sheet, &options) else {
        return;
    };
    let folder = Path::new("/music/album");
    let resolution = sheet.resolve(folder);
    for track in &resolution.tracks {
        assert!(track.source.path.starts_with(folder));
        assert!(track.source.path.components().count() > folder.components().count());
        if let Some(end) = track.end() {
            assert!(end > track.start());
        }
        if let Some(pregap) = track.pregap_start {
            assert!(pregap < track.start());
        }
    }
    for warning in sheet.warnings.iter().chain(&resolution.warnings) {
        let _ = warning.to_string();
    }
    // Already-decoded text takes the same path as a `CUESHEET` tag.
    if let Ok(text) = std::str::from_utf8(data)
        && let Ok(sheet) = CueSheet::parse_text(text)
    {
        let _ = sheet.resolve(folder);
    }
});
