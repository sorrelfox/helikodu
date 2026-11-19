// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Locations as players store them (`file://` URIs, Windows and Unix paths) turned
//! into local paths, and the path mappings for a library that moved.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

/// A path prefix of another player's library and the folder it is now, for a
/// library that moved or came from another computer.
///
/// `from` is matched at the start of each location after it is decoded: a
/// `file://` URI or a path, with `\` read as `/`. A prefix with a drive letter
/// (`C:\Music`) is matched without regard to case, as Windows does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMapping {
    /// The prefix as the other player has it.
    pub from: String,
    /// The local folder it stands for.
    pub to: PathBuf,
}

impl PathMapping {
    /// A mapping of `from` to `to`.
    pub fn new(from: impl Into<String>, to: impl Into<PathBuf>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }

    /// Reads `FROM=TO`, as the command line takes it.
    pub fn parse(text: &str) -> Option<Self> {
        let (from, to) = text.split_once('=')?;
        (!from.is_empty() && !to.is_empty()).then(|| Self::new(from, to))
    }

    /// The local path for `location` (decoded), if it starts with `from`.
    fn apply(&self, location: &[u8]) -> Option<Vec<u8>> {
        let from = decode(&self.from);
        let from = trim_slashes(&from);
        if from.is_empty() || location.len() < from.len() {
            return None;
        }
        let (head, rest) = location.split_at(from.len());
        let same = if is_windows(from) {
            head.eq_ignore_ascii_case(from)
        } else {
            head == from
        };
        if !same || !(rest.is_empty() || rest[0] == b'/') {
            return None;
        }
        let mut path = self.to.clone().into_os_string().into_vec();
        while path.len() > 1 && path.last() == Some(&b'/') {
            path.pop();
        }
        path.extend_from_slice(rest);
        Some(path)
    }
}

fn trim_slashes(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 1 && bytes[end - 1] == b'/' {
        end -= 1;
    }
    &bytes[..end]
}

/// Whether a decoded path starts with a drive letter (`C:/`) or is a network
/// path (`//server/`).
fn is_windows(path: &[u8]) -> bool {
    (path.len() >= 2 && path[0].is_ascii_alphabetic() && path[1] == b':') || path.starts_with(b"//")
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).copied().and_then(hex),
                bytes.get(i + 2).copied().and_then(hex),
            )
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// The path a location names, as bytes, with `/` as the separator: a `file://`
/// URI is decoded (`file://localhost/C:/x` and `file:///C:/x` give `C:/x`);
/// anything else is taken as a path, `\` read as `/` when it looks like a
/// Windows path.
pub(super) fn decode(location: &str) -> Vec<u8> {
    let location = location.trim();
    let lower = location.get(..7).map(str::to_ascii_lowercase);
    if lower.as_deref() == Some("file://") {
        let rest = &location[7..];
        // The host part: empty or `localhost`.
        let rest = match rest.find('/') {
            Some(slash) => &rest[slash..],
            None => rest,
        };
        let mut path = percent_decode(rest);
        // `/C:/x` is `C:/x`.
        if path.len() >= 3 && path[0] == b'/' && path[1].is_ascii_alphabetic() && path[2] == b':' {
            path.remove(0);
        }
        return path;
    }
    let mut path = location.as_bytes().to_vec();
    let windows = (path.len() >= 2 && path[0].is_ascii_alphabetic() && path[1] == b':')
        || path.starts_with(b"\\\\");
    if windows {
        for b in &mut path {
            if *b == b'\\' {
                *b = b'/';
            }
        }
    }
    path
}

/// Whether `location` names something other than a local file (a stream).
pub(super) fn is_remote(location: &str) -> bool {
    let location = location.trim();
    match location.find("://") {
        Some(at) => !location[..at].eq_ignore_ascii_case("file"),
        None => false,
    }
}

/// The local path of `location` under `mappings`: the first mapping that fits,
/// or the decoded path itself.
pub(super) fn local_path(location: &str, mappings: &[PathMapping]) -> Option<PathBuf> {
    if location.trim().is_empty() || is_remote(location) {
        return None;
    }
    let decoded = decode(location);
    let bytes = mappings
        .iter()
        .find_map(|m| m.apply(&decoded))
        .unwrap_or(decoded);
    Some(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(location: &str, mappings: &[PathMapping]) -> Option<String> {
        local_path(location, mappings).map(|p| p.to_string_lossy().into_owned())
    }

    #[test]
    fn uris_are_decoded() {
        assert_eq!(
            local("file:///home/me/Music/a%20b%C3%A9.mp3", &[]).as_deref(),
            Some("/home/me/Music/a bé.mp3")
        );
        assert_eq!(
            local("file://localhost/C:/Music/x.mp3", &[]).as_deref(),
            Some("C:/Music/x.mp3")
        );
        assert_eq!(
            local("/plain/path.flac", &[]).as_deref(),
            Some("/plain/path.flac")
        );
        assert_eq!(local("http://radio/stream", &[]), None);
        // Bytes that are not UTF-8 survive.
        let path = local_path("file:///m/%FF.mp3", &[]).unwrap();
        assert_eq!(path.into_os_string().into_vec(), b"/m/\xff.mp3");
    }

    #[test]
    fn mappings_replace_a_prefix() {
        let windows = [PathMapping::new(r"C:\Music", "/home/me/Music")];
        assert_eq!(
            local(r"c:\music\Album\01.mp3", &windows).as_deref(),
            Some("/home/me/Music/Album/01.mp3")
        );
        assert_eq!(
            local("file://localhost/C:/Music/Album/01.mp3", &windows).as_deref(),
            Some("/home/me/Music/Album/01.mp3")
        );
        // Only at a folder boundary.
        assert_eq!(
            local(r"C:\Musical\01.mp3", &windows).as_deref(),
            Some("C:/Musical/01.mp3")
        );
        let unix = [PathMapping::new("/old/Music/", "/new/Music")];
        assert_eq!(
            local("file:///old/Music/a.flac", &unix).as_deref(),
            Some("/new/Music/a.flac")
        );
        // Case matters on Unix.
        assert_eq!(
            local("/old/music/a.flac", &unix).as_deref(),
            Some("/old/music/a.flac")
        );
        assert_eq!(
            PathMapping::parse("C:/Music=/home/me/Music"),
            Some(PathMapping::new("C:/Music", "/home/me/Music"))
        );
        assert_eq!(PathMapping::parse("nothing"), None);
    }
}
