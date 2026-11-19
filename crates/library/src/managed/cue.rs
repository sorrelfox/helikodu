// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The FILE lines of a CUE sheet: which audio files it names, and the sheet with
//! those names changed. The sheet is handled as bytes, line by line, so that
//! everything but the names stays as it was (its encoding, its line ends).

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

/// The parts of one FILE line: what comes before the name, the name, and what
/// comes after it (the file type and the line end).
struct FileLine<'a> {
    before: &'a [u8],
    name: &'a [u8],
    after: &'a [u8],
}

fn parse_line(line: &[u8]) -> Option<FileLine<'_>> {
    let start = line
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(line.len());
    // A byte order mark before the first line.
    let start = if line[start..].starts_with(b"\xEF\xBB\xBF") {
        start + 3
    } else {
        start
    };
    let rest = &line[start..];
    if rest.len() < 5 || !rest[..4].eq_ignore_ascii_case(b"FILE") || !rest[4].is_ascii_whitespace()
    {
        return None;
    }
    let after_keyword = start
        + 4
        + rest[4..]
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(rest.len() - 4);
    let tail = &line[after_keyword..];
    if let Some(quoted) = tail.strip_prefix(b"\"") {
        let end = quoted.iter().position(|&b| b == b'"')?;
        Some(FileLine {
            before: &line[..after_keyword],
            name: &quoted[..end],
            after: &quoted[end + 1..],
        })
    } else {
        let end = tail
            .iter()
            .position(u8::is_ascii_whitespace)
            .unwrap_or(tail.len());
        if end == 0 {
            return None;
        }
        Some(FileLine {
            before: &line[..after_keyword],
            name: &tail[..end],
            after: &tail[end..],
        })
    }
}

/// The names of the FILE lines of `sheet`, in order.
pub(crate) fn file_names(sheet: &[u8]) -> Vec<Vec<u8>> {
    sheet
        .split(|&b| b == b'\n')
        .filter_map(parse_line)
        .map(|line| line.name.to_vec())
        .collect()
}

/// `sheet` with the name of each FILE line for which `rename` gives a new name
/// replaced by it (quoted).
pub(crate) fn rewrite(sheet: &[u8], rename: &dyn Fn(&[u8]) -> Option<Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::with_capacity(sheet.len() + 64);
    for (i, line) in sheet.split(|&b| b == b'\n').enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        match parse_line(line).and_then(|parts| rename(parts.name).map(|new| (parts, new))) {
            Some((parts, new)) => {
                out.extend_from_slice(parts.before);
                out.push(b'"');
                out.extend_from_slice(&new);
                out.push(b'"');
                out.extend_from_slice(parts.after);
            }
            None => out.extend_from_slice(line),
        }
    }
    out
}

/// The last part of a name in a FILE line, which may hold folders written with
/// either slash.
pub(crate) fn base_name(name: &[u8]) -> &[u8] {
    let start = name
        .iter()
        .rposition(|&b| b == b'/' || b == b'\\')
        .map_or(0, |i| i + 1);
    &name[start..]
}

fn stem(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b'.') {
        Some(0) | None => name,
        Some(i) => &name[..i],
    }
}

/// Which of `candidates` (audio files beside the sheet) the FILE name `name`
/// means: the same name, the same name in other capitals, or the same name with
/// another extension (a sheet written for a WAV file that was then compressed).
pub(crate) fn matching<'a>(name: &[u8], candidates: &'a [PathBuf]) -> Option<&'a PathBuf> {
    let wanted = base_name(name);
    fn file_name(p: &Path) -> &[u8] {
        p.file_name().map(OsStrExt::as_bytes).unwrap_or_default()
    }
    candidates
        .iter()
        .find(|c| file_name(c) == wanted)
        .or_else(|| {
            candidates
                .iter()
                .find(|c| file_name(c).eq_ignore_ascii_case(wanted))
        })
        .or_else(|| {
            let wanted = stem(wanted);
            candidates
                .iter()
                .find(|c| stem(file_name(c)).eq_ignore_ascii_case(wanted))
        })
}

/// The path of `file` relative to the folder `from`, for a FILE line; both are
/// absolute. Folders up are written as `..`.
pub(crate) fn relative(from: &Path, file: &Path) -> PathBuf {
    let from: Vec<Component<'_>> = from.components().collect();
    let to: Vec<Component<'_>> = file.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for part in &to[common..] {
        out.push(part.as_os_str());
    }
    out
}

/// The bytes of `path`.
pub(crate) fn bytes(path: &Path) -> Vec<u8> {
    OsStr::as_bytes(path.as_os_str()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &[u8] = b"\xEF\xBB\xBFREM GENRE Jazz\r\nPERFORMER \"Okto\"\r\nFILE \"01 Night.wav\" WAVE\r\n  TRACK 01 AUDIO\r\n    INDEX 01 00:00:00\r\nfile second.flac WAVE\r\n";

    #[test]
    fn file_lines_are_found_quoted_or_not() {
        assert_eq!(
            file_names(SHEET),
            vec![b"01 Night.wav".to_vec(), b"second.flac".to_vec()]
        );
        assert!(file_names(b"FILES are not lines\nFILE\n").is_empty());
    }

    #[test]
    fn only_the_names_change() {
        let out = rewrite(SHEET, &|name| {
            (name == b"01 Night.wav").then(|| b"Okto/01 Night Ferry.flac".to_vec())
        });
        assert_eq!(
            out,
            b"\xEF\xBB\xBFREM GENRE Jazz\r\nPERFORMER \"Okto\"\r\nFILE \"Okto/01 Night Ferry.flac\" WAVE\r\n  TRACK 01 AUDIO\r\n    INDEX 01 00:00:00\r\nfile second.flac WAVE\r\n"
        );
        assert_eq!(rewrite(SHEET, &|_| None), SHEET);
    }

    #[test]
    fn names_match_by_name_then_capitals_then_stem() {
        let candidates = vec![
            PathBuf::from("/a/01 Night.flac"),
            PathBuf::from("/a/Second.FLAC"),
        ];
        assert_eq!(matching(b"01 Night.wav", &candidates), Some(&candidates[0]));
        assert_eq!(
            matching(b"CD1\\second.flac", &candidates),
            Some(&candidates[1])
        );
        assert_eq!(matching(b"third.flac", &candidates), None);
    }

    #[test]
    fn relative_paths_go_up_and_down() {
        assert_eq!(
            relative(Path::new("/m/A/B"), Path::new("/m/A/B/x.flac")),
            PathBuf::from("x.flac")
        );
        assert_eq!(
            relative(Path::new("/m/A/B"), Path::new("/m/A/C/x.flac")),
            PathBuf::from("../C/x.flac")
        );
    }
}
