// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The quick hash that recognises a file after a move without reading all of it.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

use sha2::{Digest, Sha256};

/// How much of each end of the file goes into the hash.
pub const QUICK_HASH_SPAN: u64 = 64 << 10;

/// The length of a quick hash in bytes.
pub const QUICK_HASH_LEN: usize = 16;

/// The quick hash of `file`, whose size is `size`: the first 16 bytes of the SHA-256
/// of the size (64-bit little-endian), the first 64 KiB and the last 64 KiB. For a
/// file of 128 KiB or less, the two parts do not overlap: the second is what follows
/// the first.
///
/// Two files with the same quick hash are taken to be the same file (a move or a
/// copy); a change of tags changes it, because tags sit at either end of a file.
pub fn quick_hash(file: &mut File, size: u64) -> io::Result<Vec<u8>> {
    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());
    let head_len = size.min(QUICK_HASH_SPAN);
    let tail_len = (size - head_len).min(QUICK_HASH_SPAN);
    let mut buffer = vec![0_u8; usize::try_from(QUICK_HASH_SPAN).unwrap_or(65_536)];

    file.seek(SeekFrom::Start(0))?;
    read_part(file, &mut buffer, head_len, &mut hasher)?;
    if tail_len > 0 {
        file.seek(SeekFrom::Start(size - tail_len))?;
        read_part(file, &mut buffer, tail_len, &mut hasher)?;
    }
    let digest = hasher.finalize();
    Ok(digest[..QUICK_HASH_LEN].to_vec())
}

fn read_part(file: &mut File, buffer: &mut [u8], len: u64, hasher: &mut Sha256) -> io::Result<()> {
    let len = usize::try_from(len).map_err(io::Error::other)?;
    let part = &mut buffer[..len];
    file.read_exact(part)?;
    hasher.update(part);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn hash_of(bytes: &[u8]) -> Vec<u8> {
        let dir = std::env::temp_dir().join(format!("hk-quick-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}.bin", bytes.len()));
        File::create(&path).unwrap().write_all(bytes).unwrap();
        let mut file = File::open(&path).unwrap();
        let hash = quick_hash(&mut file, bytes.len() as u64).unwrap();
        std::fs::remove_file(&path).unwrap();
        hash
    }

    #[test]
    fn the_middle_of_a_large_file_does_not_count() {
        let mut a = vec![7_u8; 300_000];
        let b = a.clone();
        a[150_000] = 8;
        assert_eq!(hash_of(&a), hash_of(&b));
        a[10] = 8;
        assert_ne!(hash_of(&a), hash_of(&b));
        let mut c = b.clone();
        c[299_999] = 0;
        assert_ne!(hash_of(&c), hash_of(&b));
        assert_eq!(hash_of(&b).len(), QUICK_HASH_LEN);
    }

    #[test]
    fn small_files_are_hashed_whole() {
        assert_ne!(hash_of(b""), hash_of(b"\0"));
        let mut a = vec![1_u8; 100_000];
        let b = a.clone();
        a[70_000] = 2;
        assert_ne!(hash_of(&a), hash_of(&b));
    }
}
