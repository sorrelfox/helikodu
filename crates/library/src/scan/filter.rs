// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which files a scan takes: the kinds of files by extension, and name patterns.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// What a file found by the walk is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FileClass {
    /// An audio file.
    Audio,
    /// A video file, scanned for its audio when the options say so.
    Video,
    /// A CUE sheet, which splits the audio files of its folder into tracks.
    CueSheet,
}

/// Extensions of audio files, in lower case.
const AUDIO: &[&[u8]] = &[
    b"aac", b"ac3", b"adts", b"aif", b"aifc", b"aiff", b"alac", b"amr", b"ape", b"asf", b"au",
    b"caf", b"dff", b"dsf", b"dts", b"eac3", b"flac", b"m4a", b"m4b", b"mka", b"mp+", b"mp1",
    b"mp2", b"mp3", b"mpa", b"mpc", b"oga", b"ogg", b"ofr", b"ofs", b"opus", b"spx", b"tak",
    b"tta", b"w64", b"wav", b"weba", b"wma", b"wv",
];

/// Extensions of video files, in lower case.
const VIDEO: &[&[u8]] = &[
    b"3gp", b"avi", b"divx", b"f4v", b"flv", b"m2ts", b"m4v", b"mkv", b"mov", b"mp4", b"mpeg",
    b"mpg", b"mts", b"ogv", b"ts", b"vob", b"webm", b"wmv",
];

/// The class of the file named `name`, or `None` for a file the library does not
/// take (pictures, text, playlists, …).
pub(crate) fn classify(name: &OsStr) -> Option<FileClass> {
    let extension = Path::new(name).extension()?.as_bytes();
    let is = |list: &[&[u8]]| list.iter().any(|e| e.eq_ignore_ascii_case(extension));
    if extension.eq_ignore_ascii_case(b"cue") {
        Some(FileClass::CueSheet)
    } else if is(AUDIO) {
        Some(FileClass::Audio)
    } else if is(VIDEO) {
        Some(FileClass::Video)
    } else {
        None
    }
}

/// Whether the file named `name` is an audiobook by its format (`.m4b`).
pub(crate) fn is_audiobook(name: &OsStr) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|e| e.as_bytes().eq_ignore_ascii_case(b"m4b"))
}

/// Whether `name` matches `pattern`: `*` matches any run of bytes, `?` one byte.
pub(crate) fn matches_pattern(pattern: &[u8], name: &[u8]) -> bool {
    // Iterative matching with one backtrack point: linear in practice and never
    // exponential, whatever the pattern.
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some(b'*') => {
                star = Some((p, n));
                p += 1;
            }
            Some(&c) if c == b'?' || c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                Some((sp, sn)) => {
                    p = sp + 1;
                    n = sn + 1;
                    star = Some((sp, sn + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_by_extension() {
        assert_eq!(classify(OsStr::new("a.FLAC")), Some(FileClass::Audio));
        assert_eq!(classify(OsStr::new("a.m4b")), Some(FileClass::Audio));
        assert_eq!(classify(OsStr::new("a.Cue")), Some(FileClass::CueSheet));
        assert_eq!(classify(OsStr::new("a.mkv")), Some(FileClass::Video));
        assert_eq!(classify(OsStr::new("cover.jpg")), None);
        assert_eq!(classify(OsStr::new("flac")), None);
        assert_eq!(
            classify(OsStr::from_bytes(b"\xff\xfe.mp3")),
            Some(FileClass::Audio)
        );
        assert!(is_audiobook(OsStr::new("Book.M4B")));
        assert!(!is_audiobook(OsStr::new("Song.m4a")));
    }

    #[test]
    fn patterns() {
        let m = |p: &str, n: &str| matches_pattern(p.as_bytes(), n.as_bytes());
        assert!(m("Podcasts", "Podcasts"));
        assert!(!m("Podcasts", "podcasts"));
        assert!(m("*.part", "song.flac.part"));
        assert!(!m("*.part", "song.flac"));
        assert!(m("Sample?", "Sample1"));
        assert!(!m("Sample?", "Sample"));
        assert!(m("*", ""));
        assert!(m("a*b*c", "aXXbYYc"));
        assert!(!m("a*b*c", "aXXbYY"));
        assert!(m("**x", "x"));
        assert!(matches_pattern(b"*\xff*", b"ab\xffcd"));
        let long = "a".repeat(200);
        assert!(!m(&"*a".repeat(30).replace("aa", "a"), &format!("{long}b")));
    }
}
