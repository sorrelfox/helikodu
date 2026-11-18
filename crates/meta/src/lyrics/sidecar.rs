// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lyrics files beside the track: the same base name with `.lrc` or `.txt`.
//!
//! The name is taken as bytes, so a track whose name is not UTF-8 finds its
//! file too. The text is decoded as CUE sheets are: a byte order mark, then
//! UTF-8, then the legacy charset the user chose, or else a guessed one.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use encoding_rs::Encoding;

use crate::cue::text::decode;

use super::{Lyrics, LyricsError, LyricsSource, SidecarKind};

/// The largest lyrics file read.
pub const MAX_SIDECAR_BYTES: u64 = 1 << 20;

/// The extensions looked for, in order, with the kind each gives.
const EXTENSIONS: [(&str, SidecarKind); 6] = [
    ("lrc", SidecarKind::Lrc),
    ("LRC", SidecarKind::Lrc),
    ("Lrc", SidecarKind::Lrc),
    ("txt", SidecarKind::Text),
    ("TXT", SidecarKind::Text),
    ("Txt", SidecarKind::Text),
];

/// The lyrics files beside `track` that exist: `.lrc` before `.txt`.
pub fn sidecar_paths(track: &Path) -> Vec<(PathBuf, SidecarKind)> {
    let Some(stem) = track.file_stem() else {
        return Vec::new();
    };
    let mut found: Vec<(PathBuf, SidecarKind)> = Vec::new();
    for (extension, kind) in EXTENSIONS {
        let mut name = OsString::from(stem);
        name.push(".");
        name.push(extension);
        let path = track.with_file_name(name);
        if path == track || !path.is_file() {
            continue;
        }
        // On a case-insensitive file system the spellings find one file.
        if found
            .iter()
            .any(|(p, _)| same_file(p, &path).unwrap_or(false))
        {
            continue;
        }
        found.push((path, kind));
    }
    found
}

#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let (a, b) = (a.metadata()?, b.metadata()?);
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}

#[cfg(not(unix))]
fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    Ok(a == b)
}

/// Reads the lyrics file at `path`; `legacy_charset` replaces the guessed
/// charset of text that is not UTF-8. `None` for a file without lines.
pub fn read_sidecar(
    path: &Path,
    kind: SidecarKind,
    legacy_charset: Option<&'static Encoding>,
) -> Result<Option<Lyrics>, LyricsError> {
    let file = File::open(path)?;
    if file.metadata()?.len() > MAX_SIDECAR_BYTES {
        return Err(LyricsError::TooLarge(path.to_owned()));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SIDECAR_BYTES).read_to_end(&mut bytes)?;
    let (text, encoding) = decode(&bytes, legacy_charset);
    let source = LyricsSource::Sidecar {
        path: path.to_owned(),
        kind,
        charset: encoding.name(),
    };
    Ok(Lyrics::from_text(&text, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn names_that_are_not_utf8_find_their_file() {
        use std::os::unix::ffi::OsStrExt;
        let dir =
            std::env::temp_dir().join(format!("helikodu-lyrics-sidecar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("folder");
        // "Кино" in cp1251, not valid UTF-8.
        let stem: &[u8] = &[0xCA, 0xE8, 0xED, 0xEE];
        let mut track = stem.to_vec();
        track.extend_from_slice(b".flac");
        let mut lrc = stem.to_vec();
        lrc.extend_from_slice(b".lrc");
        let track = dir.join(std::ffi::OsStr::from_bytes(&track));
        std::fs::write(&track, b"").expect("track");
        std::fs::write(
            dir.join(std::ffi::OsStr::from_bytes(&lrc)),
            b"[00:01.00]hi\n",
        )
        .expect("lrc");
        let found = sidecar_paths(&track);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, SidecarKind::Lrc);
        let lyrics = read_sidecar(&found[0].0, found[0].1, None)
            .expect("read")
            .expect("lines");
        assert!(lyrics.synced);
        assert_eq!(lyrics.lines[0].text, "hi");
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
