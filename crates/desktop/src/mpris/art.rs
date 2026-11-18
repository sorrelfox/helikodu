// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The cover behind `mpris:artUrl`: a copy of the current cover thumbnail in a
//! folder that other processes can read, at the same path inside and outside a
//! Flatpak sandbox.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use super::file_uri;

/// The folder for the exported cover.
///
/// Outside a sandbox it is `$XDG_RUNTIME_DIR/helikodu`. Inside Flatpak the
/// runtime folder is private, except `$XDG_RUNTIME_DIR/app/$FLATPAK_ID`, which
/// the host sees at the same path, so that one is used. Without a runtime folder,
/// `$XDG_CACHE_HOME/helikodu/mpris` (the cache folder also has the same path on
/// both sides).
pub fn default_art_dir() -> Option<PathBuf> {
    let env = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    if let Some(runtime) = env("XDG_RUNTIME_DIR").map(PathBuf::from) {
        if let Some(app) = env("FLATPAK_ID") {
            return Some(runtime.join("app").join(app));
        }
        return Some(runtime.join("helikodu"));
    }
    let cache = env("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(cache.join("helikodu").join("mpris"))
}

/// Copies covers into one folder, keeping only the newest.
#[derive(Debug)]
pub(crate) struct ArtExport {
    dir: Option<PathBuf>,
    current: Option<PathBuf>,
}

impl ArtExport {
    pub(crate) fn new(dir: Option<PathBuf>) -> Self {
        Self { dir, current: None }
    }

    /// Exports `cover` and returns its `file://` URL; `None` removes the export.
    ///
    /// The copy is named after the source file, so the same cover keeps its URL
    /// (clients cache by URL) and another cover gets a new one.
    pub(crate) fn export(&mut self, cover: Option<&Path>) -> io::Result<Option<String>> {
        let Some(cover) = cover else {
            self.remove_current();
            return Ok(None);
        };
        let Some(dir) = &self.dir else {
            return Ok(None);
        };
        let name = cover.file_name().unwrap_or(OsStr::new("cover"));
        let mut file_name = OsStr::new("cover-").to_os_string();
        file_name.push(name);
        let target = dir.join(file_name);
        if self.current.as_deref() != Some(target.as_path()) || !target.exists() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
            // Copy to a temporary name first, so that no client reads half a file.
            let partial = target.with_extension("partial");
            fs::copy(cover, &partial)?;
            fs::rename(&partial, &target)?;
            if self.current.as_deref() != Some(target.as_path()) {
                self.remove_current();
            }
            self.current = Some(target.clone());
        }
        Ok(file_uri(&target))
    }

    fn remove_current(&mut self) {
        if let Some(old) = self.current.take() {
            let _ = fs::remove_file(old);
        }
    }
}

impl Drop for ArtExport {
    fn drop(&mut self) {
        self.remove_current();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "helikodu-desktop-art-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn covers_are_copied_and_replaced() {
        let dir = scratch("export");
        let source_a = dir.join("aa11.jpg");
        let source_b = dir.join("bb22.png");
        fs::write(&source_a, b"jpeg").unwrap();
        fs::write(&source_b, b"png").unwrap();
        let out = dir.join("out");
        let mut export = ArtExport::new(Some(out.clone()));

        let url = export.export(Some(&source_a)).unwrap().unwrap();
        let first = out.join("cover-aa11.jpg");
        assert_eq!(url, format!("file://{}", first.display()));
        assert_eq!(fs::read(&first).unwrap(), b"jpeg");
        // The same cover again keeps its URL.
        assert_eq!(export.export(Some(&source_a)).unwrap().unwrap(), url);

        let url = export.export(Some(&source_b)).unwrap().unwrap();
        assert!(url.ends_with("/cover-bb22.png"));
        assert!(!first.exists(), "the old cover is removed");

        assert_eq!(export.export(None).unwrap(), None);
        assert!(!out.join("cover-bb22.png").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn without_a_folder_there_is_no_url() {
        let mut export = ArtExport::new(None);
        assert_eq!(export.export(Some(Path::new("/x.jpg"))).unwrap(), None);
    }
}
