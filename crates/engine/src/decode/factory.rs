// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The decoder factory trait and probing.

use std::fmt;
use std::path::Path;

use super::decoder::Decoder;
use super::error::DecodeError;

/// How well a factory recognises a file. Higher is better; the registry tries
/// factories from the highest score down and leaves out those that score
/// [`NONE`](Self::NONE).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProbeScore(u8);

impl ProbeScore {
    /// The factory cannot open the file.
    pub const NONE: Self = Self(0);
    /// The factory may be able to open any file: a broad decoder that probes the
    /// content itself (FFmpeg).
    pub const FALLBACK: Self = Self(10);
    /// The file name suggests the factory's format. Never enough alone to win over
    /// a content match.
    pub const EXTENSION: Self = Self(50);
    /// The content carries the factory's signature (magic bytes).
    pub const SIGNATURE: Self = Self(100);

    /// A score of `value`.
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// The score as a number.
    pub const fn value(self) -> u8 {
        self.0
    }
}

/// What a factory sees of a file when probing: its path and its first bytes.
#[derive(Debug, Clone, Copy)]
pub struct Probe<'a> {
    path: &'a Path,
    header: &'a [u8],
}

impl<'a> Probe<'a> {
    /// How many bytes from the start of the file the registry reads for probing.
    pub const HEADER_LEN: usize = 4096;

    /// A probe of `path`, whose first bytes are `header` (fewer than
    /// [`HEADER_LEN`](Self::HEADER_LEN) when the file is shorter).
    pub fn new(path: &'a Path, header: &'a [u8]) -> Self {
        Self { path, header }
    }

    /// The file's path.
    pub fn path(&self) -> &'a Path {
        self.path
    }

    /// The first bytes of the file.
    pub fn header(&self) -> &'a [u8] {
        self.header
    }

    /// The file name's extension in ASCII lower case, if it has one. A hint only:
    /// files are recognised by their content.
    pub fn extension(&self) -> Option<String> {
        self.path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
    }
}

/// A decoder backend: recognises files and opens decoders for them.
pub trait DecoderFactory: Send + Sync + fmt::Debug {
    /// The backend's short name, such as `ffmpeg` or `dsd`.
    fn name(&self) -> &'static str;

    /// How well the backend recognises the file. Cheap: it looks only at the probe.
    fn probe(&self, probe: &Probe<'_>) -> ProbeScore;

    /// Opens a decoder for the whole file at `path`, or for its subsong `subsong`
    /// (counted from 0). A backend without subsongs fails with
    /// [`DecodeError::SubsongUnsupported`] when one is asked for. Spans are applied
    /// by the registry, not here.
    fn open(&self, path: &Path, subsong: Option<u32>) -> Result<Box<dyn Decoder>, DecodeError>;
}
