// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Walking a library folder: the files it holds, with what they looked like.
//!
//! Paths stay bytes throughout. Entries are visited in byte order, so a walk of the
//! same tree always gives the same result. Real folders are walked before folders
//! reached through symbolic links, and every folder and file is taken once, by its
//! device and inode number, which also stops link loops.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::filter::{FileClass, classify, matches_pattern};
use super::options::{ExcludeRule, ScanOptions, SymlinkPolicy};

/// What a file looked like when the walk found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub(crate) size: u64,
    pub(crate) mtime_ns: i64,
    pub(crate) inode: u64,
}

impl Stamp {
    fn of(metadata: &Metadata) -> Self {
        Self {
            size: metadata.len(),
            mtime_ns: metadata
                .mtime()
                .saturating_mul(1_000_000_000)
                .saturating_add(metadata.mtime_nsec()),
            inode: metadata.ino(),
        }
    }
}

/// A file the walk found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) path: PathBuf,
    pub(crate) stamp: Stamp,
    pub(crate) class: FileClass,
}

/// The result of walking one library folder.
#[derive(Debug, Default)]
pub(crate) struct Walk {
    /// The files, in walk order.
    pub(crate) files: Vec<Found>,
    /// Folders that could not be read. What the library has below them is not
    /// taken as missing.
    pub(crate) unreadable: Vec<PathBuf>,
}

/// The rules of a walk, prepared once for every root.
#[derive(Debug)]
pub(crate) struct Walker {
    exclude_paths: Vec<PathBuf>,
    exclude_names: Vec<Vec<u8>>,
    symlinks: SymlinkPolicy,
    include_hidden: bool,
    include_video: bool,
    ignore_marker: Option<OsString>,
    /// The roots with their links resolved, for [`SymlinkPolicy::WithinRoots`].
    resolved_roots: Vec<PathBuf>,
}

/// The device and inode of a file or folder.
type FileKey = (u64, u64);

impl Walker {
    pub(crate) fn new(options: &ScanOptions) -> Self {
        let mut exclude_paths = Vec::new();
        let mut exclude_names = Vec::new();
        for rule in &options.exclude {
            match rule {
                ExcludeRule::Path(path) => {
                    exclude_paths.push(std::path::absolute(path).unwrap_or_else(|_| path.clone()));
                }
                ExcludeRule::Name(pattern) => exclude_names.push(pattern.as_bytes().to_vec()),
            }
        }
        Self {
            exclude_paths,
            exclude_names,
            symlinks: options.symlinks,
            include_hidden: options.include_hidden,
            include_video: options.include_video,
            ignore_marker: options.ignore_marker.clone(),
            resolved_roots: options
                .roots
                .iter()
                .filter_map(|root| fs::canonicalize(root).ok())
                .collect(),
        }
    }

    /// Walks `root`. Fails only when the root itself cannot be read: then nothing
    /// is known about its files, and none of them may be taken as missing.
    pub(crate) fn walk(&self, root: &Path, cancelled: &dyn Fn() -> bool) -> io::Result<Walk> {
        let metadata = fs::metadata(root)?;
        if !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "the library folder is not a folder",
            ));
        }
        // Reading it once here reports an unreadable root as an error.
        fs::read_dir(root)?;

        let mut state = State::default();
        state.seen_dirs.insert((metadata.dev(), metadata.ino()));
        let mut real = vec![root.to_path_buf()];
        let mut linked: Vec<PathBuf> = Vec::new();
        loop {
            while let Some(dir) = real.pop() {
                if cancelled() {
                    return Ok(state.walk);
                }
                self.read_folder(&dir, &mut state, &mut real, &mut linked);
            }
            // Links last, in the order found, so the real paths have been taken.
            if linked.is_empty() {
                break;
            }
            for link in std::mem::take(&mut linked) {
                self.follow(&link, &mut state, &mut real);
            }
        }
        Ok(state.walk)
    }

    /// Walks the files directly in `dir`, without its subfolders; links to files are
    /// followed as the policy allows. A folder that cannot be read is reported in
    /// [`Walk::unreadable`].
    pub(crate) fn walk_folder(&self, dir: &Path) -> Walk {
        let mut state = State::default();
        let (mut folders, mut links) = (Vec::new(), Vec::new());
        self.read_folder(dir, &mut state, &mut folders, &mut links);
        for link in links {
            // Linked folders are left out like the real ones.
            self.follow(&link, &mut state, &mut Vec::new());
        }
        state.walk
    }

    /// Whether a walk of all of `root` would enter `dir`, a folder at or below it:
    /// no folder on the way, nor `dir` itself, is hidden, excluded or holds the
    /// ignore marker. The marker in `dir` itself is checked by the walk.
    pub(crate) fn admits(&self, root: &Path, dir: &Path) -> bool {
        let Ok(relative) = dir.strip_prefix(root) else {
            return false;
        };
        let mut current = root.to_path_buf();
        for component in relative.components() {
            if self.holds_marker(&current) {
                return false;
            }
            current.push(component);
            if !self.takes_name(component.as_os_str().as_bytes()) || self.excluded_path(&current) {
                return false;
            }
        }
        true
    }

    fn holds_marker(&self, dir: &Path) -> bool {
        self.ignore_marker
            .as_ref()
            .is_some_and(|marker| fs::symlink_metadata(dir.join(marker)).is_ok())
    }

    fn read_folder(
        &self,
        dir: &Path,
        state: &mut State,
        folders: &mut Vec<PathBuf>,
        links: &mut Vec<PathBuf>,
    ) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => {
                state.walk.unreadable.push(dir.to_path_buf());
                return;
            }
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        if let Some(marker) = &self.ignore_marker
            && entries.iter().any(|e| e.file_name() == *marker)
        {
            return;
        }
        entries.sort_by_key(fs::DirEntry::file_name);
        let mut subfolders = Vec::new();
        for entry in entries {
            let name = entry.file_name();
            if !self.takes_name(name.as_bytes()) {
                continue;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                if self.symlinks != SymlinkPolicy::Ignore && !self.excluded_path(&path) {
                    links.push(path);
                }
            } else if kind.is_dir() {
                if !self.excluded_path(&path) {
                    subfolders.push(path);
                }
            } else if kind.is_file()
                && let Ok(metadata) = entry.metadata()
            {
                self.take_file(path, &metadata, state);
            }
        }
        // The stack pops the last first; reversed, folders are walked in name order.
        folders.extend(subfolders.into_iter().rev());
    }

    /// Follows the link at `link`, if the policy allows.
    fn follow(&self, link: &Path, state: &mut State, folders: &mut Vec<PathBuf>) {
        let Ok(metadata) = fs::metadata(link) else {
            return; // A dangling link.
        };
        if self.symlinks == SymlinkPolicy::WithinRoots {
            let Ok(target) = fs::canonicalize(link) else {
                return;
            };
            if !self
                .resolved_roots
                .iter()
                .any(|root| target.starts_with(root))
            {
                return;
            }
        }
        if metadata.is_dir() {
            if state.seen_dirs.insert((metadata.dev(), metadata.ino())) {
                folders.push(link.to_path_buf());
            }
        } else if metadata.is_file() {
            self.take_file(link.to_path_buf(), &metadata, state);
        }
    }

    fn take_file(&self, path: PathBuf, metadata: &Metadata, state: &mut State) {
        let Some(name) = path.file_name() else {
            return;
        };
        let Some(class) = classify(name) else {
            return;
        };
        if class == FileClass::Video && !self.include_video {
            return;
        }
        if self.excluded_path(&path) {
            return;
        }
        if !state.seen_files.insert((metadata.dev(), metadata.ino())) {
            return;
        }
        state.walk.files.push(Found {
            path,
            stamp: Stamp::of(metadata),
            class,
        });
    }

    fn takes_name(&self, name: &[u8]) -> bool {
        if !self.include_hidden && name.first() == Some(&b'.') {
            return false;
        }
        !self
            .exclude_names
            .iter()
            .any(|pattern| matches_pattern(pattern, name))
    }

    fn excluded_path(&self, path: &Path) -> bool {
        self.exclude_paths.iter().any(|ex| path.starts_with(ex))
    }
}

#[derive(Debug, Default)]
struct State {
    walk: Walk,
    seen_dirs: HashSet<FileKey>,
    seen_files: HashSet<FileKey>,
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .join(format!("hk-walk-{}", std::process::id()))
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    fn names(walk: &Walk, root: &Path) -> Vec<String> {
        walk.files
            .iter()
            .map(|f| f.path.strip_prefix(root).unwrap().display().to_string())
            .collect()
    }

    #[test]
    fn rules_and_order() {
        let root = tree("rules");
        for name in [
            "b/2.flac",
            "b/1.mp3",
            "a/x.cue",
            "a/cover.jpg",
            ".hidden/h.flac",
            "a/.h.flac",
            "skip/s.flac",
            "Podcasts/p.mp3",
            "nomedia/n.flac",
            "nomedia/.nomedia",
            "v.mkv",
            "c.part",
        ] {
            touch(&root.join(name));
        }
        let mut options = ScanOptions::new([&root]);
        options.exclude = vec![
            ExcludeRule::Path(root.join("skip")),
            ExcludeRule::Name("Podcasts".into()),
            ExcludeRule::Name("*.part".into()),
        ];
        let walk = Walker::new(&options).walk(&root, &|| false).unwrap();
        assert_eq!(names(&walk, &root), ["a/x.cue", "b/1.mp3", "b/2.flac"]);

        options.include_hidden = true;
        options.include_video = true;
        options.ignore_marker = None;
        let walk = Walker::new(&options).walk(&root, &|| false).unwrap();
        assert_eq!(
            names(&walk, &root),
            [
                "v.mkv",
                ".hidden/h.flac",
                "a/.h.flac",
                "a/x.cue",
                "b/1.mp3",
                "b/2.flac",
                "nomedia/n.flac"
            ]
        );
    }

    #[test]
    fn links_loops_and_duplicates() {
        let root = tree("links");
        let outside = tree("links-outside");
        touch(&root.join("real/a.flac"));
        touch(&outside.join("o.flac"));
        symlink(&root, root.join("real/loop")).unwrap();
        symlink(root.join("real"), root.join("again")).unwrap();
        symlink(root.join("real/a.flac"), root.join("alias.flac")).unwrap();
        symlink(&outside, root.join("out")).unwrap();
        symlink(root.join("nowhere"), root.join("dangling")).unwrap();

        let mut options = ScanOptions::new([&root]);
        let walk = Walker::new(&options).walk(&root, &|| false).unwrap();
        assert_eq!(names(&walk, &root), ["real/a.flac"]);

        options.symlinks = SymlinkPolicy::Follow;
        let walk = Walker::new(&options).walk(&root, &|| false).unwrap();
        assert_eq!(names(&walk, &root), ["real/a.flac", "out/o.flac"]);

        options.symlinks = SymlinkPolicy::Ignore;
        let walk = Walker::new(&options).walk(&root, &|| false).unwrap();
        assert_eq!(names(&walk, &root), ["real/a.flac"]);
    }

    #[test]
    fn single_folders_and_admitted_subtrees() {
        let root = tree("parts");
        for name in [
            "a/1.flac",
            "a/sub/2.flac",
            ".hidden/h.flac",
            "skip/s.flac",
            "marked/.nomedia",
            "marked/deep/m.flac",
        ] {
            touch(&root.join(name));
        }
        let mut options = ScanOptions::new([&root]);
        options.exclude = vec![ExcludeRule::Path(root.join("skip"))];
        let walker = Walker::new(&options);
        assert_eq!(
            names(&walker.walk_folder(&root.join("a")), &root),
            ["a/1.flac"]
        );
        assert!(walker.walk_folder(&root.join("marked")).files.is_empty());
        let gone = walker.walk_folder(&root.join("gone"));
        assert_eq!(gone.unreadable, [root.join("gone")]);

        assert!(walker.admits(&root, &root));
        assert!(walker.admits(&root, &root.join("a/sub")));
        assert!(!walker.admits(&root, &root.join(".hidden")));
        assert!(!walker.admits(&root, &root.join("skip/inner")));
        assert!(!walker.admits(&root, &root.join("marked/deep")));
        assert!(!walker.admits(&root, Path::new("/elsewhere")));
    }

    #[test]
    fn a_missing_root_is_an_error() {
        let root = tree("gone").join("not-there");
        let options = ScanOptions::new([&root]);
        assert!(Walker::new(&options).walk(&root, &|| false).is_err());
    }
}
