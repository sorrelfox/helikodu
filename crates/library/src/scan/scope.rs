// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parts of a library folder to scan, for rescans of what changed.

use std::path::{Path, PathBuf};

/// A part of a library folder to scan.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Scope {
    /// The whole library folder.
    Root,
    /// A folder at or below the library folder, and everything below it.
    Subtree(PathBuf),
    /// The files directly in a folder, without its subfolders.
    Folder(PathBuf),
}

/// What to scan of one library folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScanTarget {
    /// The library folder.
    pub(crate) root: PathBuf,
    /// The parts of it.
    pub(crate) scopes: Vec<Scope>,
}

/// The smallest set of scopes of `root` that covers `scopes`: no two overlap, and
/// scopes outside `root` are left out.
pub(crate) fn normalize(root: &Path, scopes: impl IntoIterator<Item = Scope>) -> Vec<Scope> {
    let mut subtrees = Vec::new();
    let mut folders = Vec::new();
    for scope in scopes {
        match scope {
            Scope::Root => return vec![Scope::Root],
            Scope::Subtree(dir) if dir == root => return vec![Scope::Root],
            Scope::Subtree(dir) if dir.starts_with(root) => subtrees.push(dir),
            Scope::Folder(dir) if dir.starts_with(root) => folders.push(dir),
            Scope::Subtree(_) | Scope::Folder(_) => {}
        }
    }
    // Component order puts a folder before everything below it.
    subtrees.sort();
    subtrees.dedup();
    let mut kept: Vec<PathBuf> = Vec::new();
    for dir in subtrees {
        if !kept.iter().any(|top| dir.starts_with(top)) {
            kept.push(dir);
        }
    }
    folders.sort();
    folders.dedup();
    folders.retain(|dir| !kept.iter().any(|top| dir.starts_with(top)));
    kept.into_iter()
        .map(Scope::Subtree)
        .chain(folders.into_iter().map(Scope::Folder))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(path: &str) -> Scope {
        Scope::Subtree(path.into())
    }

    fn folder(path: &str) -> Scope {
        Scope::Folder(path.into())
    }

    #[test]
    fn overlapping_scopes_are_merged() {
        let root = Path::new("/r");
        let scopes = normalize(
            root,
            [
                folder("/r/a"),
                sub("/r/a/b"),
                folder("/r/a/b/c"),
                sub("/r/a/b/c"),
                folder("/r/x"),
                folder("/r/x"),
                sub("/r/ab"),
                folder("/elsewhere"),
                folder("/r"),
            ],
        );
        assert_eq!(
            scopes,
            [
                sub("/r/a/b"),
                sub("/r/ab"),
                folder("/r"),
                folder("/r/a"),
                folder("/r/x")
            ]
        );
    }

    #[test]
    fn the_root_covers_everything() {
        let root = Path::new("/r");
        assert_eq!(normalize(root, [folder("/r/a"), sub("/r")]), [Scope::Root]);
        assert_eq!(normalize(root, [Scope::Root, sub("/r/a")]), [Scope::Root]);
        assert!(normalize(root, []).is_empty());
    }
}
