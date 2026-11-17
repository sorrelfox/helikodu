// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cover images in the album folder.
//!
//! The folder of the track is searched, then, when that folder is a disc
//! folder of a multi-disc album (`CD1`, `Disc 2`, `disk_03 - Bonus`, …), its
//! parent. No other folder is looked at, and no folder is searched recursively.
//!
//! A file is a candidate when its name, ignoring case, is one of the
//! [`FolderArtOptions::names`], the album title, or a Windows Media Player
//! `AlbumArt_{…}_Large`/`_Small` file, with the extension `jpg`, `jpeg`, `png`
//! or `webp`. Candidates are ordered by:
//! 1. the folder: the track's folder before the disc folder's parent;
//! 2. the name, in this order: the names in [`FolderArtOptions::names`] in
//!    their order (by default `cover`, `folder`, `front`, `albumart`, `album`,
//!    `artwork`, `art`), then `AlbumArt_{…}_Large`, then the album title, then
//!    `AlbumArt_{…}_Small`, then `AlbumArtSmall`;
//! 3. the extension: `jpg`, `jpeg`, `png`, `webp`;
//! 4. the file name's bytes, so that the order never depends on the order the
//!    file system lists the files in.
//!
//! The files are not read; a [`Cover`] reads its bytes when they are needed.

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::embedded::{SkipReason, SkippedPicture};
use super::limits::CoverLimits;
use super::source::{Cover, CoverSource};

/// Where and under which names folder images are looked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderArtOptions {
    /// The file names (without extension) to look for, best first. Compared
    /// ignoring case.
    pub names: Vec<String>,
    /// The album title, also looked for as a file name after the names above.
    /// Characters that cannot be in a file name (`/`) and those often replaced
    /// when a title becomes a file name (`\ : * ? " < > |`) match `_` or
    /// nothing.
    pub album_title: Option<String>,
    /// Whether the parent of a disc folder (`CD1`, `Disc 2`, …) is searched too.
    pub disc_parent: bool,
    /// The most directory entries looked at in one folder.
    pub max_entries: usize,
}

impl FolderArtOptions {
    /// The default names, best first.
    pub const DEFAULT_NAMES: [&str; 7] = [
        "cover", "folder", "front", "albumart", "album", "artwork", "art",
    ];
}

impl Default for FolderArtOptions {
    fn default() -> Self {
        Self {
            names: Self::DEFAULT_NAMES.iter().map(|&n| n.to_owned()).collect(),
            album_title: None,
            disc_parent: true,
            max_entries: 10_000,
        }
    }
}

/// The folder images found for one track.
#[derive(Debug, Clone, Default)]
pub struct FolderCovers {
    /// The candidates, best first. Their bytes are not read yet.
    pub covers: Vec<Cover>,
    /// Candidates left out because they are larger than the byte limit.
    pub skipped: Vec<SkippedPicture>,
}

/// The cover images beside the track at `track` (and in the parent of its disc
/// folder), best first. See the module documentation for the order.
///
/// Folders that cannot be read are passed over; files are only listed, not
/// read.
pub fn find_folder_covers(
    track: &Path,
    options: &FolderArtOptions,
    limits: &CoverLimits,
) -> FolderCovers {
    let mut out = FolderCovers::default();
    let Some(folder) = track.parent() else {
        return out;
    };
    let folder = if folder.as_os_str().is_empty() {
        Path::new(".")
    } else {
        folder
    };
    let matcher = Matcher::new(options);
    search(folder, &matcher, options, limits, &mut out);
    if options.disc_parent
        && folder
            .file_name()
            .is_some_and(|n| is_disc_folder(n.as_bytes()))
        && let Some(parent) = folder.parent()
    {
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        search(parent, &matcher, options, limits, &mut out);
    }
    out
}

/// Whether a folder name names one disc of a multi-disc album: `cd`, `disc`,
/// `disk` or `dvd`, in any case, then optionally a space, `_`, `-` or `.`, then
/// a number, then nothing or anything that does not continue the number or a
/// word (`CD1`, `Disc 2`, `disk_03`, `CD 1 - Bonus`, `Disc 2 (Live)`).
pub fn is_disc_folder(name: &[u8]) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some(rest) = ["disc", "disk", "cd", "dvd"]
        .iter()
        .find_map(|p| lower.strip_prefix(p.as_bytes()))
    else {
        return false;
    };
    let rest = match rest.first() {
        Some(b' ' | b'_' | b'-' | b'.') => &rest[1..],
        _ => rest,
    };
    let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    digits > 0 && rest.get(digits).is_none_or(|b| !b.is_ascii_alphanumeric())
}

/// Ranks file names.
struct Matcher {
    names: Vec<String>,
    album_title: Option<String>,
}

const EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

impl Matcher {
    fn new(options: &FolderArtOptions) -> Self {
        Self {
            names: options.names.iter().map(|n| fold(n)).collect(),
            album_title: options
                .album_title
                .as_deref()
                .map(fold)
                .filter(|t| !t.is_empty()),
        }
    }

    /// The rank of a file name (lower is better), or `None` when it is no
    /// candidate.
    fn rank(&self, file_name: &[u8]) -> Option<(usize, usize)> {
        let dot = file_name.iter().rposition(|&b| b == b'.')?;
        let (stem, ext) = (&file_name[..dot], &file_name[dot + 1..]);
        let ext_rank = EXTENSIONS
            .iter()
            .position(|e| e.as_bytes().eq_ignore_ascii_case(ext))?;
        if stem.is_empty() {
            return None;
        }
        let stem = fold(&String::from_utf8_lossy(stem));
        let n = self.names.len();
        let name_rank = if let Some(i) = self.names.iter().position(|name| *name == stem) {
            i
        } else if is_wmp_art(&stem, "_large") {
            n
        } else if self.album_title.as_deref() == Some(stem.as_str()) {
            n + 1
        } else if is_wmp_art(&stem, "_small") {
            n + 2
        } else if stem == "albumartsmall" {
            n + 3
        } else {
            return None;
        };
        Some((name_rank, ext_rank))
    }
}

/// Windows Media Player's `AlbumArt_{GUID}_Large` and `_Small` (already folded).
fn is_wmp_art(stem: &str, suffix: &str) -> bool {
    stem.strip_prefix("albumart_")
        .and_then(|s| s.strip_suffix(suffix))
        .is_some_and(|guid| !guid.is_empty())
}

/// Lower case, with the characters that file names cannot hold or commonly lose
/// turned into `_`.
fn fold(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

fn search(
    folder: &Path,
    matcher: &Matcher,
    options: &FolderArtOptions,
    limits: &CoverLimits,
    out: &mut FolderCovers,
) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    let mut found: Vec<((usize, usize), Vec<u8>, PathBuf)> = Vec::new();
    for entry in entries.take(options.max_entries).flatten() {
        let name = entry.file_name();
        let Some(rank) = matcher.rank(name.as_bytes()) else {
            continue;
        };
        found.push((rank, name.as_bytes().to_vec(), folder.join(&name)));
    }
    found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    for (_, _, path) in found {
        // Follows symbolic links; only regular files count.
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if meta.len() > limits.max_bytes as u64 {
            out.skipped.push(SkippedPicture {
                source: CoverSource::Folder { path },
                byte_len: meta.len(),
                reason: SkipReason::TooLarge,
            });
            continue;
        }
        out.covers.push(Cover::folder(path, meta.len(), limits));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disc_folders() {
        for name in [
            "CD1",
            "cd2",
            "CD 1",
            "Disc 2",
            "disc_03",
            "Disk-4",
            "CD1 - Bonus",
            "Disc 2 (Live)",
            "DVD1",
            "cd.1",
        ] {
            assert!(is_disc_folder(name.as_bytes()), "{name}");
        }
        for name in [
            "CD",
            "Discography",
            "cdx1",
            "Disc",
            "CD1a",
            "Album",
            "Discs 1",
        ] {
            assert!(!is_disc_folder(name.as_bytes()), "{name}");
        }
    }

    #[test]
    fn name_ranks() {
        let options = FolderArtOptions {
            album_title: Some("Kind of Blue: Legacy".to_owned()),
            ..FolderArtOptions::default()
        };
        let m = Matcher::new(&options);
        let rank = |n: &str| m.rank(n.as_bytes());
        assert_eq!(rank("cover.jpg"), Some((0, 0)));
        assert_eq!(rank("COVER.JPG"), Some((0, 0)));
        assert_eq!(rank("Cover.webp"), Some((0, 3)));
        assert_eq!(rank("folder.png"), Some((1, 2)));
        assert!(rank("front.jpeg") < rank("albumart.jpg"));
        assert!(rank("art.jpg") < rank("AlbumArt_{ABC}_Large.jpg"));
        assert!(rank("AlbumArt_{ABC}_Large.jpg") < rank("kind of blue_ legacy.jpg"));
        assert!(rank("Kind of Blue_ Legacy.jpg") < rank("AlbumArt_{ABC}_Small.jpg"));
        assert!(rank("AlbumArt_{ABC}_Small.jpg") < rank("AlbumArtSmall.jpg"));
        assert_eq!(rank("cover.gif"), None);
        assert_eq!(rank("cover"), None);
        assert_eq!(rank(".jpg"), None);
        assert_eq!(rank("cover.jpg.txt"), None);
        assert_eq!(rank("backcover.jpg"), None);
        assert_eq!(rank("AlbumArt__Large.jpg"), None);
        assert_eq!(m.rank(b"cov\xffer.jpg"), None);
    }
}
