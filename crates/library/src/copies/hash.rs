// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The hash of a whole file, which confirms that two files with the same size and
//! quick hash are copies.

use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use sha2::{Digest, Sha256};

use crate::scan::CancelToken;

/// How much is read at a time.
const CHUNK: usize = 1 << 20;

/// A file's content hash and its stamp while it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hashed {
    /// The SHA-256 of every byte of the file.
    pub(crate) hash: Vec<u8>,
    pub(crate) size: u64,
    pub(crate) mtime_ns: i64,
    pub(crate) dev: u64,
    pub(crate) inode: u64,
}

/// The modification time of `metadata` in nanoseconds, as the walk records it.
pub(crate) fn mtime_ns(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .mtime()
        .saturating_mul(1_000_000_000)
        .saturating_add(metadata.mtime_nsec())
}

/// Hashes every byte of the file at `path`. Fails if the file changed while it was
/// read (its size or modification time), or when `cancel` is set.
pub(crate) fn hash_file(path: &Path, cancel: &CancelToken) -> io::Result<Hashed> {
    let mut file = File::open(path)?;
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a file"));
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; CHUNK];
    let mut read_total: u64 = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buffer[..n]);
        read_total += n as u64;
    }
    let after = file.metadata()?;
    if after.len() != before.len()
        || mtime_ns(&after) != mtime_ns(&before)
        || read_total != after.len()
    {
        return Err(io::Error::other("the file changed while it was read"));
    }
    Ok(Hashed {
        hash: hasher.finalize().to_vec(),
        size: after.len(),
        mtime_ns: mtime_ns(&after),
        dev: after.dev(),
        inode: after.ino(),
    })
}

/// Hashes `paths` on up to `threads` threads; the results are in the order of
/// `paths`.
pub(crate) fn hash_files(
    paths: &[PathBuf],
    threads: usize,
    cancel: &CancelToken,
) -> Vec<io::Result<Hashed>> {
    let results: Vec<Mutex<Option<io::Result<Hashed>>>> =
        paths.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let threads = threads.clamp(1, 8).min(paths.len().max(1));
    thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index) else {
                        break;
                    };
                    let result = hash_file(path, cancel);
                    if let Ok(mut slot) = results[index].lock() {
                        *slot = Some(result);
                    }
                }
            });
        }
    });
    results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .ok()
                .flatten()
                .unwrap_or_else(|| Err(io::Error::other("not hashed")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_bytes_give_equal_hashes() {
        let dir = std::env::temp_dir().join(format!("hk-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a");
        let b = dir.join("b");
        let c = dir.join("c");
        let bytes: Vec<u8> = (0..3_000_000_u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&a, &bytes).unwrap();
        std::fs::write(&b, &bytes).unwrap();
        let mut other = bytes.clone();
        other[1_500_000] ^= 1;
        std::fs::write(&c, &other).unwrap();
        let cancel = CancelToken::new();
        let hashed = hash_files(&[a, b, c], 2, &cancel);
        let hashes: Vec<Vec<u8>> = hashed.into_iter().map(|h| h.unwrap().hash).collect();
        assert_eq!(hashes[0], hashes[1]);
        assert_ne!(hashes[0], hashes[2]);
        assert_eq!(hashes[0].len(), 32);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
