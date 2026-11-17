// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Paths as stored in the database: the bytes the kernel returned, never text.
//!
//! The `path`, `name` and `source_path` columns are BLOBs. A path goes in through
//! [`path_to_blob`] and comes out through [`path_from_blob`] without any conversion to
//! `String`, so names in Latin-1, cp1251, Shift-JIS or any other encoding survive. The
//! `*_text` columns beside them hold [`display_text`], which is for display and search
//! only and is never used to open a file.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// The bytes of `path`, for a BLOB column.
pub fn path_to_blob(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}

/// The path stored as `bytes` in a BLOB column.
pub fn path_from_blob(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes))
}

/// The decoded form of a path or file name, for the `*_text` columns.
///
/// Valid UTF-8 is taken as it is. Anything else is decoded with replacement characters
/// for now; detecting legacy encodings comes with the tag reader's charset detection.
pub fn display_text(name: &OsStr) -> Cow<'_, str> {
    name.to_string_lossy()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_round_trip() {
        let latin1 = OsStr::from_bytes(b"/music/Bj\xf6rk/J\xf3ga.flac");
        let blob = path_to_blob(Path::new(latin1)).to_vec();
        assert_eq!(blob, b"/music/Bj\xf6rk/J\xf3ga.flac");
        assert_eq!(path_from_blob(blob).as_os_str(), latin1);
    }

    #[test]
    fn display_text_replaces_invalid_bytes() {
        assert_eq!(display_text(OsStr::new("Jóga.flac")), "Jóga.flac");
        assert_eq!(
            display_text(OsStr::from_bytes(b"J\xf3ga.flac")),
            "J\u{FFFD}ga.flac"
        );
    }
}
