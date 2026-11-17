// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The content hash that identifies equal pictures.

use std::fmt;

use sha2::{Digest, Sha256};

/// The SHA-256 hash of a picture's encoded bytes.
///
/// Equal pictures have equal hashes wherever they were found (embedded in any
/// number of files, or a folder file), so a picture is stored and thumbnailed
/// once. SHA-256 rather than a faster non-cryptographic hash: the hash is a
/// unique key and a cache file name for data from untrusted files, and with a
/// collision-resistant hash a crafted picture cannot take the place of another
/// album's cover. Hashing costs about a millisecond per megabyte, little next to
/// reading the picture and much less than decoding it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CoverHash([u8; 32]);

impl CoverHash {
    /// The hash of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// A hash from its 32 bytes, as stored; `None` for any other length.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }

    /// The 32 bytes of the hash.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The hash as 64 lower-case hexadecimal digits, as in a file name.
    pub fn to_hex(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for CoverHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for CoverHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CoverHash({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_digest() {
        let h = CoverHash::of(b"abc");
        assert_eq!(
            h.to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(CoverHash::from_bytes(h.as_bytes()), Some(h));
        assert_eq!(CoverHash::from_bytes(&[0; 31]), None);
    }
}
