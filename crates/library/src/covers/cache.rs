// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The thumbnail files on disk, their total size limit and least-recently-used
//! eviction.
//!
//! Layout: `<dir>/<size>/<hash>.<ext>`, where `<size>` is the longest side in
//! pixels, `<hash>` the cover's content hash in 64 hexadecimal digits and `<ext>`
//! `jpg` (opaque pictures) or `png`. A file is written under a temporary name in
//! its folder and renamed into place, so a reader never sees half a thumbnail.
//!
//! The cache keeps an index of its files in memory, built from the folders when it
//! opens. A file's modification time is when it was last used: the cache sets it when
//! a thumbnail is used again (at most once a minute per file), so the order of use
//! survives a restart. When the files together pass the size limit, the least
//! recently used ones are deleted until they fit.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use helikodu_meta::covers::CoverHash;

use super::sizes::ThumbnailSize;

/// The default limit of the thumbnail cache: 2 GiB.
pub const DEFAULT_CACHE_BYTES: u64 = 2 << 30;

/// How long after its last recorded use a file's time is set again.
const TOUCH_INTERVAL: Duration = Duration::from_secs(60);

/// The prefix of the temporary files.
const TEMP_PREFIX: &str = ".tmp-";

/// Where the thumbnail cache is and how large it may grow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailCacheOptions {
    /// The cache folder.
    pub dir: PathBuf,
    /// The most bytes all thumbnails together may take.
    pub max_bytes: u64,
}

impl ThumbnailCacheOptions {
    /// The cache in `dir`, with the default limit ([`DEFAULT_CACHE_BYTES`]).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            max_bytes: DEFAULT_CACHE_BYTES,
        }
    }

    /// `$XDG_CACHE_HOME/helikodu/thumbnails`, or `~/.cache/helikodu/thumbnails` when
    /// `XDG_CACHE_HOME` is unset, empty or not an absolute path; `None` when neither
    /// it nor `HOME` names a folder.
    pub fn default_dir() -> Option<PathBuf> {
        default_dir_in(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))
    }
}

fn default_dir_in(
    cache_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let absolute = |value: Option<std::ffi::OsString>| {
        value.map(PathBuf::from).filter(|path| path.is_absolute())
    };
    let base = absolute(cache_home).or_else(|| absolute(home).map(|home| home.join(".cache")))?;
    Some(base.join("helikodu").join("thumbnails"))
}

/// One thumbnail in the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Key {
    size: ThumbnailSize,
    hash: CoverHash,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    extension: &'static str,
    bytes: u64,
    /// When it was last used, in nanoseconds since the Unix epoch; unique.
    used: u64,
    /// The last use written to the file's modification time.
    recorded: u64,
}

#[derive(Debug, Default)]
struct Index {
    entries: HashMap<Key, Entry>,
    /// The entries by their last use.
    order: BTreeMap<u64, Key>,
    total: u64,
    /// The last use handed out.
    clock: u64,
}

impl Index {
    fn tick(&mut self) -> u64 {
        let now = nanos(SystemTime::now());
        self.clock = now.max(self.clock + 1);
        self.clock
    }

    fn add(&mut self, key: Key, mut entry: Entry) {
        self.remove(key);
        while self.order.contains_key(&entry.used) {
            entry.used += 1;
        }
        self.clock = self.clock.max(entry.used);
        self.order.insert(entry.used, key);
        self.total += entry.bytes;
        self.entries.insert(key, entry);
    }

    fn remove(&mut self, key: Key) -> Option<Entry> {
        let entry = self.entries.remove(&key)?;
        self.order.remove(&entry.used);
        self.total -= entry.bytes;
        Some(entry)
    }
}

/// What [`ThumbnailCache::remove_unreferenced`] and eviction removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Removed {
    /// The files removed.
    pub files: usize,
    /// Their size in bytes.
    pub bytes: u64,
}

/// The thumbnail files on disk. The module documentation explains more.
///
/// Shared between threads; every method takes `&self`.
#[derive(Debug)]
pub struct ThumbnailCache {
    dir: PathBuf,
    max_bytes: AtomicU64,
    index: Mutex<Index>,
    /// Numbers the temporary files of this process.
    temp: AtomicU64,
}

impl ThumbnailCache {
    /// Opens the cache in `options.dir`, creating the folder, removing temporary
    /// files left behind, and evicting files over the limit.
    pub fn open(options: &ThumbnailCacheOptions) -> io::Result<Self> {
        fs::create_dir_all(&options.dir)?;
        let cache = Self {
            dir: options.dir.clone(),
            max_bytes: AtomicU64::new(options.max_bytes),
            index: Mutex::new(Index::default()),
            temp: AtomicU64::new(0),
        };
        cache.load()?;
        cache.evict();
        Ok(cache)
    }

    /// The cache folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The size limit in bytes.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// Changes the size limit, evicting files if the cache is over the new one.
    pub fn set_max_bytes(&self, max_bytes: u64) -> Removed {
        self.max_bytes.store(max_bytes, Ordering::Relaxed);
        self.evict()
    }

    /// The size of all thumbnails in bytes.
    pub fn total_bytes(&self) -> u64 {
        self.lock().total
    }

    /// The number of thumbnails.
    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }

    /// Whether the cache holds no thumbnail.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the thumbnail of `hash` in `size` is there, without counting it as
    /// used.
    pub fn contains(&self, hash: &CoverHash, size: ThumbnailSize) -> bool {
        self.lock().entries.contains_key(&Key { size, hash: *hash })
    }

    /// The file of the thumbnail of `hash` in `size`, if it is there; counts as a
    /// use.
    pub fn get(&self, hash: &CoverHash, size: ThumbnailSize) -> Option<PathBuf> {
        let key = Key { size, hash: *hash };
        let mut index = self.lock();
        let mut entry = index.remove(key)?;
        entry.used = index.tick();
        let path = self.path_of(key, entry.extension);
        if entry.used.saturating_sub(entry.recorded) >= TOUCH_INTERVAL.as_nanos() as u64 {
            match touch(&path, entry.used) {
                Ok(()) => entry.recorded = entry.used,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    // Deleted behind the cache's back.
                    return None;
                }
                Err(_) => {}
            }
        }
        index.add(key, entry);
        Some(path)
    }

    /// Stores `bytes` as the thumbnail of `hash` in `size`, with the file extension
    /// `extension` (`jpg` or `png`), replacing one that is there; evicts the least
    /// recently used files if the cache is then over its limit. Returns the file.
    ///
    /// A thumbnail larger than the whole limit is refused
    /// ([`io::ErrorKind::FileTooLarge`]) rather than evicting everything else.
    pub fn insert(
        &self,
        hash: &CoverHash,
        size: ThumbnailSize,
        bytes: &[u8],
        extension: &'static str,
    ) -> io::Result<PathBuf> {
        if bytes.len() as u64 > self.max_bytes() {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "the thumbnail is larger than the cache limit",
            ));
        }
        let key = Key { size, hash: *hash };
        let folder = self.dir.join(size.to_string());
        fs::create_dir_all(&folder)?;
        let path = self.path_of(key, extension);
        let temp = folder.join(format!(
            "{TEMP_PREFIX}{}-{}-{}",
            hash.to_hex(),
            std::process::id(),
            self.temp.fetch_add(1, Ordering::Relaxed)
        ));
        let written = (|| {
            let mut file = File::create(&temp)?;
            file.write_all(bytes)?;
            file.sync_data()?;
            fs::rename(&temp, &path)
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&temp);
            return Err(e);
        }
        {
            let mut index = self.lock();
            if let Some(old) = index.remove(key)
                && old.extension != extension
            {
                let _ = fs::remove_file(self.path_of(key, old.extension));
            }
            let used = index.tick();
            index.add(
                key,
                Entry {
                    extension,
                    bytes: bytes.len() as u64,
                    used,
                    recorded: used,
                },
            );
        }
        self.evict();
        Ok(path)
    }

    /// Removes the thumbnails of every cover whose hash is not in `referenced`: the
    /// covers no track, folder or album has any more.
    pub fn remove_unreferenced(&self, referenced: &HashSet<CoverHash>) -> Removed {
        let mut index = self.lock();
        let gone: Vec<Key> = index
            .entries
            .keys()
            .filter(|key| !referenced.contains(&key.hash))
            .copied()
            .collect();
        let mut removed = Removed::default();
        for key in gone {
            if let Some(entry) = index.remove(key) {
                let _ = fs::remove_file(self.path_of(key, entry.extension));
                removed.files += 1;
                removed.bytes += entry.bytes;
            }
        }
        removed
    }

    /// Removes every thumbnail.
    pub fn clear(&self) -> Removed {
        self.remove_unreferenced(&HashSet::new())
    }

    /// Deletes the least recently used files until the cache fits its limit.
    fn evict(&self) -> Removed {
        let max = self.max_bytes();
        let mut index = self.lock();
        let mut removed = Removed::default();
        while index.total > max {
            let Some((_, key)) = index.order.first_key_value().map(|(u, k)| (*u, *k)) else {
                break;
            };
            if let Some(entry) = index.remove(key) {
                let _ = fs::remove_file(self.path_of(key, entry.extension));
                removed.files += 1;
                removed.bytes += entry.bytes;
            }
        }
        removed
    }

    fn path_of(&self, key: Key, extension: &str) -> PathBuf {
        self.dir
            .join(key.size.to_string())
            .join(format!("{}.{extension}", key.hash.to_hex()))
    }

    fn lock(&self) -> MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Builds the index from the folders.
    fn load(&self) -> io::Result<()> {
        let mut index = self.lock();
        for folder in fs::read_dir(&self.dir)? {
            let folder = folder?;
            let Some(size) = folder
                .file_name()
                .to_str()
                .and_then(|name| name.parse().ok())
                .and_then(ThumbnailSize::from_pixels)
            else {
                continue;
            };
            if !folder.file_type()?.is_dir() {
                continue;
            }
            for file in fs::read_dir(folder.path())? {
                let file = file?;
                let name = file.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if name.starts_with(TEMP_PREFIX) {
                    let _ = fs::remove_file(file.path());
                    continue;
                }
                let Some((hex, extension)) = name.split_once('.') else {
                    continue;
                };
                let extension = match extension {
                    "jpg" => "jpg",
                    "png" => "png",
                    _ => continue,
                };
                let Some(hash) = parse_hex(hex) else {
                    continue;
                };
                let metadata = file.metadata()?;
                if !metadata.is_file() {
                    continue;
                }
                let used = metadata.modified().map(nanos).unwrap_or(0);
                index.add(
                    Key { size, hash },
                    Entry {
                        extension,
                        bytes: metadata.len(),
                        used,
                        recorded: used,
                    },
                );
            }
        }
        Ok(())
    }
}

fn nanos(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// Sets the modification time of `path` to `used` (ns since the epoch).
fn touch(path: &Path, used: u64) -> io::Result<()> {
    let file = File::options().append(true).open(path)?;
    file.set_modified(UNIX_EPOCH + Duration::from_nanos(used))
}

fn parse_hex(hex: &str) -> Option<CoverHash> {
    if hex.len() != 64 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    let hash = CoverHash::from_bytes(&bytes)?;
    // Only the lower-case spelling the cache writes.
    (hash.to_hex() == hex).then_some(hash)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn the_default_folder() {
        let dir = |cache: Option<&str>, home: Option<&str>| {
            default_dir_in(cache.map(OsString::from), home.map(OsString::from))
        };
        assert_eq!(
            dir(Some("/c"), Some("/h")),
            Some(PathBuf::from("/c/helikodu/thumbnails"))
        );
        assert_eq!(
            dir(Some("relative"), Some("/h")),
            Some(PathBuf::from("/h/.cache/helikodu/thumbnails"))
        );
        assert_eq!(
            dir(Some(""), Some("/h")),
            Some(PathBuf::from("/h/.cache/helikodu/thumbnails"))
        );
        assert_eq!(dir(None, None), None);
    }

    #[test]
    fn hex_names() {
        let hash = CoverHash::of(b"x");
        assert_eq!(parse_hex(&hash.to_hex()), Some(hash));
        assert_eq!(parse_hex(&hash.to_hex().to_uppercase()), None);
        assert_eq!(parse_hex("abc"), None);
        assert_eq!(parse_hex(&"g".repeat(64)), None);
    }
}
