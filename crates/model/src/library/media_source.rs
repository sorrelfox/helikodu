// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where the audio of a track comes from.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::audio::MediaTime;

/// A file, or a part of one, that the engine can play.
///
/// The path holds the bytes the kernel returned, whether or not they are valid UTF-8,
/// and is never converted to a string except for display. It is serialized as a byte
/// array (in JSON, an array of numbers); text is also accepted when deserializing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MediaSource {
    /// The file.
    #[serde(
        serialize_with = "crate::path_serde::serialize_bytes",
        deserialize_with = "crate::path_serde::deserialize"
    )]
    pub path: PathBuf,
    /// The part of the file to play, or `None` for all of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<SourceSection>,
}

impl MediaSource {
    /// The whole of the file at `path`.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            section: None,
        }
    }

    /// The same file, restricted to `section`.
    #[must_use]
    pub fn with_section(self, section: SourceSection) -> Self {
        Self {
            section: Some(section),
            ..self
        }
    }

    /// The path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A part of a file that is played as a track of its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSection {
    /// One of several logical tracks inside the file, as the decoder numbers them
    /// (game music, multi-track modules, multi-programme containers). Counted from 0.
    Subsong(u32),
    /// A span of the file, for example a track of a CUE sheet.
    Span {
        /// Where the span starts.
        start: MediaTime,
        /// Where the span ends, or `None` for the end of the file.
        end: Option<MediaTime>,
    },
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;

    use super::*;
    use crate::path_serde::from_bytes as path_from_bytes;

    /// A name in Latin-1, which is not valid UTF-8: "Café.flac".
    fn latin1_path() -> PathBuf {
        path_from_bytes(b"/music/Caf\xe9.flac".to_vec())
    }

    #[test]
    fn non_utf8_path_survives_json() {
        let source = MediaSource::file(latin1_path());
        assert!(source.path.to_str().is_none());
        let json = serde_json::to_string(&source).unwrap();
        let back: MediaSource = serde_json::from_str(&json).unwrap();
        assert_eq!(back, source);
        assert_eq!(back.path.as_os_str().as_bytes(), b"/music/Caf\xe9.flac");
    }

    #[test]
    fn text_paths_are_accepted() {
        let back: MediaSource = serde_json::from_str(r#"{"path":"/music/a.flac"}"#).unwrap();
        assert_eq!(back, MediaSource::file("/music/a.flac"));
    }

    #[test]
    fn sections_round_trip() {
        let source = MediaSource::file(latin1_path()).with_section(SourceSection::Span {
            start: MediaTime::from_cue_frames(1234),
            end: None,
        });
        let json = serde_json::to_string(&source).unwrap();
        assert_eq!(serde_json::from_str::<MediaSource>(&json).unwrap(), source);

        let subsong = MediaSource::file("/music/game.nsf").with_section(SourceSection::Subsong(3));
        let json = serde_json::to_string(&subsong).unwrap();
        assert!(json.ends_with(r#""section":{"subsong":3}}"#), "{json}");
        assert_eq!(serde_json::from_str::<MediaSource>(&json).unwrap(), subsong);
    }
}
