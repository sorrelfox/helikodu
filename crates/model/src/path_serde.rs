// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Serde for file paths that keeps their bytes: paths are never converted to strings
//! except for display, so names that are not valid UTF-8 survive.

use std::ffi::OsString;
use std::fmt;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserializer, Serializer};

/// A path as a byte array (in JSON, an array of numbers).
pub(crate) fn serialize_bytes<S: Serializer>(
    path: &Path,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_bytes(path.as_os_str().as_bytes())
}

/// A path as bytes, a sequence of numbers or text.
pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
    deserializer.deserialize_byte_buf(PathVisitor)
}

/// A path as text or a sequence of numbers, in formats without a byte type, such as
/// TOML.
pub(crate) fn deserialize_text<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<PathBuf, D::Error> {
    deserializer.deserialize_any(PathVisitor)
}

/// A path as text when it is valid UTF-8, and as an array of byte values otherwise:
/// the form for files people edit, such as `settings.toml`.
pub(crate) fn serialize_text<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    match path.to_str() {
        Some(text) => serializer.serialize_str(text),
        None => {
            let bytes = path.as_os_str().as_bytes();
            let mut seq = serializer.serialize_seq(Some(bytes.len()))?;
            for byte in bytes {
                seq.serialize_element(byte)?;
            }
            seq.end()
        }
    }
}

/// Paths as [`serialize_text`] writes them, for a list.
pub(crate) mod text_list {
    use std::path::PathBuf;

    use serde::de::{SeqAccess, Visitor};
    use serde::ser::SerializeSeq;
    use serde::{Deserializer, Serializer};

    /// One path in its text-or-bytes form.
    struct Item<'a>(&'a std::path::Path);

    impl serde::Serialize for Item<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            super::serialize_text(self.0, serializer)
        }
    }

    struct Owned(PathBuf);

    impl<'de> serde::Deserialize<'de> for Owned {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            super::deserialize_text(deserializer).map(Owned)
        }
    }

    #[allow(clippy::ptr_arg)] // serde passes the field
    pub(crate) fn serialize<S: Serializer>(
        paths: &Vec<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(paths.len()))?;
        for path in paths {
            seq.serialize_element(&Item(path))?;
        }
        seq.end()
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<PathBuf>, D::Error> {
        struct ListVisitor;

        impl<'de> Visitor<'de> for ListVisitor {
            type Value = Vec<PathBuf>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a list of file paths")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<PathBuf>, A::Error> {
                let mut paths = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(Owned(path)) = seq.next_element()? {
                    paths.push(path);
                }
                Ok(paths)
            }
        }

        deserializer.deserialize_seq(ListVisitor)
    }
}

/// An optional path as [`serialize_text`] writes it; skip the field when it is `None`
/// (`skip_serializing_if = "Option::is_none"`), since TOML has no null.
pub(crate) mod opt_text {
    use std::path::PathBuf;

    use serde::{Deserializer, Serializer};

    #[allow(clippy::ref_option)] // serde passes the field
    pub(crate) fn serialize<S: Serializer>(
        path: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match path {
            Some(path) => super::serialize_text(path, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        super::deserialize_text(deserializer).map(Some)
    }
}

/// Accepts a path as bytes, as a sequence of numbers, or as text.
struct PathVisitor;

impl<'de> Visitor<'de> for PathVisitor {
    type Value = PathBuf;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a file path as bytes or text")
    }

    fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<PathBuf, E> {
        Ok(from_bytes(bytes.to_vec()))
    }

    fn visit_byte_buf<E: de::Error>(self, bytes: Vec<u8>) -> Result<PathBuf, E> {
        Ok(from_bytes(bytes))
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<PathBuf, E> {
        Ok(PathBuf::from(text))
    }

    fn visit_string<E: de::Error>(self, text: String) -> Result<PathBuf, E> {
        Ok(PathBuf::from(text))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<PathBuf, A::Error> {
        let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(byte) = seq.next_element::<u8>()? {
            bytes.push(byte);
        }
        Ok(from_bytes(bytes))
    }
}

pub(crate) fn from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes))
}
