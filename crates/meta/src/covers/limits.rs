// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The limits on pictures from untrusted files.

/// Limits on the pictures read from files and decoded.
///
/// The same limits apply wherever a picture is read or decoded: in the scan
/// workers and in the main process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoverLimits {
    /// The largest picture, in encoded bytes. Larger ones are skipped and
    /// reported, never copied or decoded.
    pub max_bytes: usize,
    /// The most pixels (width × height) a picture may declare. Larger ones are
    /// refused before any pixel is allocated.
    pub max_pixels: u64,
    /// The most embedded pictures kept from one file. Further ones are skipped.
    pub max_count: usize,
}

impl CoverLimits {
    /// 32 MiB.
    pub const DEFAULT_MAX_BYTES: usize = 32 * 1024 * 1024;
    /// 64 Mi pixels: 8192 × 8192.
    pub const DEFAULT_MAX_PIXELS: u64 = 64 * 1024 * 1024;
    /// 16 pictures.
    pub const DEFAULT_MAX_COUNT: usize = 16;

    /// The most bytes a decoder may allocate for a picture within these limits:
    /// the pixel limit at 8 bytes per pixel (16-bit RGBA, the widest layout the
    /// decoders produce).
    pub(crate) fn max_alloc(&self) -> u64 {
        self.max_pixels.saturating_mul(8)
    }
}

impl Default for CoverLimits {
    /// 32 MiB, 64 Mi pixels and 16 pictures per file.
    fn default() -> Self {
        Self {
            max_bytes: Self::DEFAULT_MAX_BYTES,
            max_pixels: Self::DEFAULT_MAX_PIXELS,
            max_count: Self::DEFAULT_MAX_COUNT,
        }
    }
}
