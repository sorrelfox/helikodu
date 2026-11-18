// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Entry locations: from what a file says to an [`EntryLocation`], and back.
//!
//! Reading, without touching the disk:
//! - a URL with a scheme of two letters or more is [`EntryLocation::Remote`], except a
//!   `file:` URL. On this host (`file:///…`, `file://localhost/…`, `file:/…`) its
//!   percent-encoded path is decoded into bytes; on another host it is a network share,
//!   [`EntryLocation::Foreign`];
//! - Windows paths are recognised by a drive letter, a leading `\\`, or `\` used as the
//!   only separator. `\` becomes `/`; a path with a drive or a share, or rooted at the
//!   current drive, is [`EntryLocation::Foreign`];
//! - any other path is [`EntryLocation::Local`]; a relative one is joined to the
//!   playlist's folder, and `.` and `..` are resolved lexically. A path may leave the
//!   playlist's folder: a library matches entries to its own tracks only.
//!
//! In XSPF, locations are URI references, so a relative one is percent-decoded too.

use std::ffi::OsStr;
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use super::WriteOptions;
use super::entry::EntryLocation;

/// Why an entry's location was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocationProblem {
    /// The location is empty or names only a folder.
    Empty,
    /// The location holds a NUL byte, which no path can.
    ContainsNul,
}

impl fmt::Display for LocationProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "the location is empty",
            Self::ContainsNul => "the location holds a NUL byte",
        })
    }
}

/// How local paths are written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PathStyle {
    /// Relative to the playlist's folder, when the path shares more than the root
    /// folder with it; absolute otherwise.
    #[default]
    Relative,
    /// Absolute.
    Absolute,
}

/// How a location is written in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Syntax {
    /// A path or a URL, as in M3U and PLS.
    Path,
    /// A URI reference, as in XSPF.
    Uri,
}

/// The location `raw` stands for, in a playlist in the folder `base`.
pub(crate) fn resolve_location(
    raw: &[u8],
    base: Option<&Path>,
    syntax: Syntax,
) -> Result<EntryLocation, LocationProblem> {
    let raw = raw.trim_ascii();
    if raw.is_empty() {
        return Err(LocationProblem::Empty);
    }
    if let Some(colon) = scheme_end(raw) {
        let scheme = &raw[..colon];
        if !scheme.eq_ignore_ascii_case(b"file") {
            return Ok(EntryLocation::Remote(
                String::from_utf8_lossy(raw).into_owned(),
            ));
        }
        return file_url(&raw[colon + 1..]);
    }
    let decoded;
    let path = match syntax {
        Syntax::Path => raw,
        Syntax::Uri => {
            decoded = percent_decode(raw);
            &decoded
        }
    };
    if path.contains(&0) {
        return Err(LocationProblem::ContainsNul);
    }
    if let Some(windows) = windows_path(path, base) {
        return windows;
    }
    local(path, base)
}

/// A `file:` URL; `rest` follows the colon.
fn file_url(rest: &[u8]) -> Result<EntryLocation, LocationProblem> {
    let path = match rest.strip_prefix(b"//") {
        Some(authority_and_path) => {
            let slash = authority_and_path
                .iter()
                .position(|&b| b == b'/')
                .unwrap_or(authority_and_path.len());
            let host = &authority_and_path[..slash];
            if !host.is_empty() && !host.eq_ignore_ascii_case(b"localhost") {
                // A file on another machine: a network share, as `\\server\share`.
                let mut share = b"//".to_vec();
                share.extend_from_slice(&percent_decode(authority_and_path));
                if share.contains(&0) {
                    return Err(LocationProblem::ContainsNul);
                }
                return foreign_path(&share);
            }
            &authority_and_path[slash..]
        }
        None => rest,
    };
    // A query or fragment is not part of the path.
    let end = path
        .iter()
        .position(|&b| b == b'?' || b == b'#')
        .unwrap_or(path.len());
    let path = percent_decode(&path[..end]);
    if path.contains(&0) {
        return Err(LocationProblem::ContainsNul);
    }
    // `file:///C:/Music/a.mp3` and `file:///C|/Music/a.mp3`.
    let drive = path.strip_prefix(b"/").unwrap_or(&path);
    if drive.len() >= 2 && drive[0].is_ascii_alphabetic() && matches!(drive[1], b':' | b'|') {
        let mut foreign = drive.to_vec();
        foreign[1] = b':';
        return foreign_path(&foreign);
    }
    if !path.starts_with(b"/") {
        return Err(LocationProblem::Empty);
    }
    local(&path, None)
}

/// A Windows path, as [`EntryLocation::Foreign`] or as a relative local path with `/`;
/// `None` for anything else.
fn windows_path(
    path: &[u8],
    base: Option<&Path>,
) -> Option<Result<EntryLocation, LocationProblem>> {
    let has_drive = path.len() >= 2 && path[0].is_ascii_alphabetic() && path[1] == b':';
    let backslashes = path.contains(&b'\\');
    if !has_drive && !backslashes {
        return None;
    }
    let share = path.starts_with(b"\\\\");
    let only_backslashes = backslashes && !path.contains(&b'/');
    if !has_drive && !share && !only_backslashes {
        // A POSIX name that happens to hold a backslash.
        return None;
    }
    let slashed: Vec<u8> = path
        .iter()
        .map(|&b| if b == b'\\' { b'/' } else { b })
        .collect();
    if has_drive || share || slashed.starts_with(b"/") {
        return Some(foreign_path(&slashed));
    }
    // A relative path with backslashes, as Windows players write: `Album\01.mp3`.
    Some(local(&slashed, base))
}

/// A foreign path with its `.` components dropped and repeated separators merged.
fn foreign_path(path: &[u8]) -> Result<EntryLocation, LocationProblem> {
    let lead = path.iter().take_while(|&&b| b == b'/').count().min(2);
    let mut out = vec![b'/'; lead];
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in path.split(|&b| b == b'/') {
        match part {
            b"" | b"." => {}
            b".." => {
                // Keep the drive or the share's server.
                if parts.len() > 1 {
                    parts.pop();
                }
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(LocationProblem::Empty);
    }
    out.extend_from_slice(&parts.join(&b'/'));
    Ok(EntryLocation::Foreign(out))
}

/// A local path: joined to `base` when relative, with `.` and `..` resolved.
fn local(path: &[u8], base: Option<&Path>) -> Result<EntryLocation, LocationProblem> {
    let path = Path::new(OsStr::from_bytes(path));
    let joined = match base {
        Some(base) if path.is_relative() => base.join(path),
        _ => path.to_owned(),
    };
    let normal = normalize(&joined);
    if normal.as_os_str().is_empty() || normal.file_name().is_none() {
        return Err(LocationProblem::Empty);
    }
    Ok(EntryLocation::Local(normal))
}

/// `path` with `.` dropped and `..` removing the component before it. A `..` at the
/// root stays at the root; leading `..` of a relative path are kept.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::RootDir => out.push("/"),
            Component::CurDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                if depth > 0 {
                    out.pop();
                    depth -= 1;
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            Component::Normal(part) => {
                out.push(part);
                depth += 1;
            }
        }
    }
    out
}

/// The position of the colon ending a URL scheme of two characters or more.
fn scheme_end(raw: &[u8]) -> Option<usize> {
    let colon = raw.iter().position(|&b| b == b':')?;
    let scheme = &raw[..colon];
    (scheme.len() >= 2
        && scheme[0].is_ascii_alphabetic()
        && scheme
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')))
    .then_some(colon)
}

/// `raw` with every `%XX` replaced by its byte. Malformed escapes stay as they are.
pub(crate) fn percent_decode(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && let (Some(high), Some(low)) = (
                raw.get(i + 1).and_then(|&b| hex(b)),
                raw.get(i + 2).and_then(|&b| hex(b)),
            )
        {
            out.push(high << 4 | low);
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    out
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// `bytes` with every byte outside the unreserved set and `/` percent-encoded, so that
/// any file name, in any charset, survives in a URI.
pub(crate) fn percent_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        }
    }
    out
}

/// The bytes a local path is written as: relative to the playlist's folder or
/// absolute, as the options ask.
pub(crate) fn path_for_writing(path: &Path, options: &WriteOptions) -> PathBuf {
    match (options.path_style, &options.base_dir) {
        (PathStyle::Relative, Some(base)) => {
            relative_to(path, base).unwrap_or_else(|| path.to_owned())
        }
        _ => path.to_owned(),
    }
}

/// `path` relative to the folder `base`, when both are absolute and share more than
/// the root.
fn relative_to(path: &Path, base: &Path) -> Option<PathBuf> {
    if !path.is_absolute() || !base.is_absolute() {
        return None;
    }
    let path = normalize(path);
    let base = normalize(base);
    let ours: Vec<_> = path.components().collect();
    let theirs: Vec<_> = base.components().collect();
    let common = ours.iter().zip(&theirs).take_while(|(a, b)| a == b).count();
    // Only the root in common: an absolute path reads better than a row of `..`.
    if common <= 1 {
        return None;
    }
    let mut relative = PathBuf::new();
    for _ in common..theirs.len() {
        relative.push("..");
    }
    for part in &ours[common..] {
        relative.push(part);
    }
    (!relative.as_os_str().is_empty()).then_some(relative)
}

/// How a location is written in XSPF: a `file:` URI for an absolute path, a
/// percent-encoded relative reference for a relative one, the URL itself for a
/// remote entry.
pub(crate) fn uri_for_writing(location: &EntryLocation, options: &WriteOptions) -> String {
    match location {
        EntryLocation::Local(path) => {
            let written = path_for_writing(path, options);
            let bytes = written.as_os_str().as_bytes();
            if written.is_absolute() {
                format!("file://{}", percent_encode(bytes))
            } else {
                percent_encode(bytes)
            }
        }
        EntryLocation::Foreign(path) => {
            if path.starts_with(b"//") {
                // A network share: `file://server/share/…`.
                format!("file:{}", percent_encode(path))
            } else if path.starts_with(b"/") {
                format!("file://{}", percent_encode(path))
            } else {
                // Keep the drive's colon readable: `file:///C:/…`.
                let (drive, rest) = path.split_at(path.len().min(2));
                format!(
                    "file:///{}{}",
                    String::from_utf8_lossy(drive),
                    percent_encode(rest)
                )
            }
        }
        EntryLocation::Remote(url) => url.clone(),
    }
}

/// The bytes a foreign path is written as in M3U and PLS: with `\`, as Windows
/// players wrote it.
pub(crate) fn foreign_for_writing(path: &[u8]) -> Vec<u8> {
    path.iter()
        .map(|&b| if b == b'/' { b'\\' } else { b })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(raw: &[u8], syntax: Syntax) -> Result<EntryLocation, LocationProblem> {
        resolve_location(raw, Some(Path::new("/music/lists")), syntax)
    }

    fn local_bytes(raw: &[u8], syntax: Syntax) -> Vec<u8> {
        match resolve(raw, syntax) {
            Ok(EntryLocation::Local(path)) => path.as_os_str().as_bytes().to_vec(),
            other => panic!("{} gave {other:?}", String::from_utf8_lossy(raw)),
        }
    }

    fn foreign(raw: &[u8]) -> Vec<u8> {
        match resolve(raw, Syntax::Path) {
            Ok(EntryLocation::Foreign(path)) => path,
            other => panic!("{} gave {other:?}", String::from_utf8_lossy(raw)),
        }
    }

    #[test]
    fn relative_paths_join_the_folder() {
        use Syntax::Path as P;
        assert_eq!(local_bytes(b"a.mp3", P), b"/music/lists/a.mp3");
        assert_eq!(local_bytes(b"./x/../a.mp3", P), b"/music/lists/a.mp3");
        assert_eq!(local_bytes(b"../Album/01.flac", P), b"/music/Album/01.flac");
        assert_eq!(local_bytes(b"../../../../a.mp3", P), b"/a.mp3");
        assert_eq!(
            local_bytes(b"Album\\01.flac", P),
            b"/music/lists/Album/01.flac"
        );
        assert_eq!(local_bytes(b"Caf\xe9.mp3", P), b"/music/lists/Caf\xe9.mp3");
        assert_eq!(local_bytes(b"  /abs/a b.mp3 ", P), b"/abs/a b.mp3");
        assert_eq!(local_bytes(b"a%20b.mp3", P), b"/music/lists/a%20b.mp3");
        assert_eq!(
            local_bytes(b"a%20b.mp3", Syntax::Uri),
            b"/music/lists/a b.mp3"
        );
        assert_eq!(
            local_bytes(b"01 Intro: Part 1.mp3", P),
            b"/music/lists/01 Intro: Part 1.mp3"
        );
    }

    #[test]
    fn without_a_folder_relative_paths_stay_relative() {
        let location = resolve_location(b"../a/./b.mp3", None, Syntax::Path);
        assert_eq!(location, Ok(EntryLocation::Local("../a/b.mp3".into())));
    }

    #[test]
    fn file_urls_are_decoded() {
        use Syntax::Path as P;
        assert_eq!(
            local_bytes(b"file:///music/Bj%C3%B6rk/a%20b.mp3", P),
            "/music/Björk/a b.mp3".as_bytes()
        );
        assert_eq!(
            local_bytes(b"FILE://localhost/a/%E9.mp3", P),
            b"/a/\xe9.mp3"
        );
        assert_eq!(local_bytes(b"file:/a/b.mp3", P), b"/a/b.mp3");
        assert_eq!(local_bytes(b"file:///a/b.mp3#t=10", P), b"/a/b.mp3");
        assert_eq!(foreign(b"file:///C:/Music/a%20b.mp3"), b"C:/Music/a b.mp3");
        assert_eq!(foreign(b"file:///d|/x.mp3"), b"d:/x.mp3");
        assert_eq!(
            resolve(b"file:///a%00b.mp3", P),
            Err(LocationProblem::ContainsNul)
        );
    }

    #[test]
    fn urls_are_remote() {
        for url in [
            &b"http://example.org/a.mp3"[..],
            b"HTTPS://example.org/stream",
            b"smb://nas/music/a.flac",
            b"rtsp://x/y",
        ] {
            assert!(
                matches!(resolve(url, Syntax::Path), Ok(EntryLocation::Remote(_))),
                "{}",
                String::from_utf8_lossy(url)
            );
        }
    }

    #[test]
    fn windows_paths_are_foreign() {
        assert_eq!(
            foreign(b"C:\\Music\\Artist\\01.mp3"),
            b"C:/Music/Artist/01.mp3"
        );
        assert_eq!(foreign(b"c:/Music/./x/../01.mp3"), b"c:/Music/01.mp3");
        assert_eq!(
            foreign(b"\\\\server\\share\\a.mp3"),
            b"//server/share/a.mp3"
        );
        assert_eq!(foreign(b"\\Music\\a.mp3"), b"/Music/a.mp3");
        assert_eq!(foreign(b"D:a.mp3"), b"D:a.mp3");
        assert_eq!(
            foreign(b"file://nas/share/a%20b.mp3"),
            b"//nas/share/a b.mp3"
        );
    }

    #[test]
    fn bad_locations_are_rejected() {
        assert_eq!(resolve(b"   ", Syntax::Path), Err(LocationProblem::Empty));
        assert_eq!(
            resolve(b"a\0b", Syntax::Path),
            Err(LocationProblem::ContainsNul)
        );
        assert_eq!(
            resolve(b"a%00b", Syntax::Uri),
            Err(LocationProblem::ContainsNul)
        );
        assert_eq!(
            resolve_location(b"/", None, Syntax::Path),
            Err(LocationProblem::Empty)
        );
        assert_eq!(
            resolve_location(b"..", None, Syntax::Path),
            Err(LocationProblem::Empty)
        );
    }

    #[test]
    fn relative_paths_for_writing() {
        let options = WriteOptions {
            path_style: PathStyle::Relative,
            base_dir: Some("/music/lists".into()),
        };
        let written = |p: &str| path_for_writing(Path::new(p), &options);
        assert_eq!(written("/music/lists/a.mp3"), Path::new("a.mp3"));
        assert_eq!(written("/music/A/01.flac"), Path::new("../A/01.flac"));
        assert_eq!(written("/other/a.mp3"), Path::new("/other/a.mp3"));
        let absolute = WriteOptions {
            path_style: PathStyle::Absolute,
            ..options
        };
        assert_eq!(
            path_for_writing(Path::new("/music/lists/a.mp3"), &absolute),
            Path::new("/music/lists/a.mp3")
        );
    }

    #[test]
    fn uris_keep_every_byte() {
        let options = WriteOptions {
            path_style: PathStyle::Absolute,
            base_dir: None,
        };
        let path = PathBuf::from(OsStr::from_bytes(b"/m/Caf\xe9 #1.mp3"));
        let uri = uri_for_writing(&EntryLocation::Local(path.clone()), &options);
        assert_eq!(uri, "file:///m/Caf%E9%20%231.mp3");
        assert_eq!(
            resolve_location(uri.as_bytes(), None, Syntax::Uri),
            Ok(EntryLocation::Local(path))
        );
        let drive = uri_for_writing(&EntryLocation::Foreign(b"C:/M/a b.mp3".to_vec()), &options);
        assert_eq!(drive, "file:///C:/M/a%20b.mp3");
        let share = uri_for_writing(&EntryLocation::Foreign(b"//nas/s/a.mp3".to_vec()), &options);
        assert_eq!(share, "file://nas/s/a.mp3");
    }
}
