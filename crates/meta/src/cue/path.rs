// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The paths in `FILE` lines, which must stay inside the sheet's folder.
//!
//! A path is taken as bytes and checked lexically, without touching the disk:
//! absolute paths (`/…`, `\…`, `C:\…`, `\\server\…`) and URLs are rejected, `\` is
//! a separator as on Windows, `.` is dropped and `..` removes the previous
//! component; a `..` that would leave the sheet's folder rejects the path. The
//! result is a relative path of plain components. Symbolic links inside the folder
//! are the library's concern: it confines every file to its library roots.

use std::ffi::OsStr;
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// Why a `FILE` path was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRejection {
    /// The path is empty or names only the folder itself.
    Empty,
    /// The path holds a NUL byte.
    ContainsNul,
    /// The path is absolute or names a drive or a network share.
    Absolute,
    /// The path is a URL. URLs are never fetched.
    Url,
    /// The path leaves the sheet's folder through `..`.
    EscapesFolder,
}

impl fmt::Display for PathRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "the path is empty",
            Self::ContainsNul => "the path holds a NUL byte",
            Self::Absolute => "absolute paths are not followed",
            Self::Url => "URLs are not followed",
            Self::EscapesFolder => "the path leaves the sheet's folder",
        })
    }
}

/// The relative path `name` stands for, inside the sheet's folder.
pub fn relative_path(name: &[u8]) -> Result<PathBuf, PathRejection> {
    let name = name.trim_ascii();
    if name.is_empty() {
        return Err(PathRejection::Empty);
    }
    if name.contains(&0) {
        return Err(PathRejection::ContainsNul);
    }
    if is_url(name) {
        return Err(PathRejection::Url);
    }
    if matches!(name[0], b'/' | b'\\')
        || (name.len() >= 2 && name[0].is_ascii_alphabetic() && name[1] == b':')
    {
        return Err(PathRejection::Absolute);
    }
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in name.split(|&b| b == b'/' || b == b'\\') {
        match part {
            b"" | b"." => {}
            b".." => {
                if parts.pop().is_none() {
                    return Err(PathRejection::EscapesFolder);
                }
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(PathRejection::Empty);
    }
    Ok(parts.iter().map(|p| OsStr::from_bytes(p)).collect())
}

/// Whether `name` starts with a URL scheme of two or more characters followed by
/// `:` (`http:`, `file:`, `smb:`). One letter and a colon is a drive.
fn is_url(name: &[u8]) -> bool {
    let Some(colon) = name.iter().position(|&b| b == b':') else {
        return false;
    };
    let scheme = &name[..colon];
    scheme.len() >= 2
        && scheme[0].is_ascii_alphabetic()
        && scheme
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(name: &[u8]) -> Vec<u8> {
        relative_path(name)
            .expect("accepted")
            .as_os_str()
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn plain_names_are_kept_as_bytes() {
        assert_eq!(ok(b"Album.flac"), b"Album.flac");
        assert_eq!(ok(b"  Album.flac "), b"Album.flac");
        assert_eq!(ok(b"CD1\\01.wav"), b"CD1/01.wav");
        assert_eq!(ok(b"./CD1//01.wav"), b"CD1/01.wav");
        assert_eq!(ok(b"CD1/../CD2/01.wav"), b"CD2/01.wav");
        assert_eq!(ok(b"Caf\xe9.wav"), b"Caf\xe9.wav");
        assert_eq!(ok(b"..foo.wav"), b"..foo.wav");
        assert_eq!(ok(b"01 Intro: Part 1.wav"), b"01 Intro: Part 1.wav");
    }

    #[test]
    fn hostile_names_are_rejected() {
        use PathRejection::*;
        let cases: &[(&[u8], PathRejection)] = &[
            (b"", Empty),
            (b"   ", Empty),
            (b".", Empty),
            (b"a/..", Empty),
            (b"/etc/passwd", Absolute),
            (b"\\Windows\\win.ini", Absolute),
            (b"\\\\server\\share\\a.wav", Absolute),
            (b"C:\\Music\\a.wav", Absolute),
            (b"c:a.wav", Absolute),
            (b"../a.wav", EscapesFolder),
            (b"..\\..\\a.wav", EscapesFolder),
            (b"CD1/../../a.wav", EscapesFolder),
            (b"http://example.org/a.mp3", Url),
            (b"file:///etc/passwd", Url),
            (b"smb://nas/a.flac", Url),
            (b"a\0.wav", ContainsNul),
        ];
        for (name, reason) in cases {
            assert_eq!(
                relative_path(name),
                Err(*reason),
                "{}",
                String::from_utf8_lossy(name)
            );
        }
    }
}
