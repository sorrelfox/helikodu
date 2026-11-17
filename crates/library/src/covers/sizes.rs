// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sizes thumbnails are made in.

use std::fmt;

/// The size of a thumbnail: the longest side in device pixels. Thumbnails keep
/// the picture's aspect ratio and are never larger than the picture.
///
/// There are four sizes, two for each scale of the screen:
///
/// | Size | Used for |
/// |---|---|
/// | [`SMALL`](Self::SMALL), 48 | covers of 22 to 44 logical pixels at scale 1: table and queue rows, the deck, the tag editor |
/// | [`SMALL_2X`](Self::SMALL_2X), 96 | the same at scale 2 |
/// | [`LARGE`](Self::LARGE), 192 | the panel's now-playing block (104), album tiles (148–160) and the album hero (184) at scale 1 |
/// | [`LARGE_2X`](Self::LARGE_2X), 384 | the same at scale 2 |
///
/// The Now Playing view (up to 400 logical pixels) decodes the picture itself.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThumbnailSize(u32);

impl ThumbnailSize {
    /// 48 pixels.
    pub const SMALL: Self = Self(48);
    /// 96 pixels.
    pub const SMALL_2X: Self = Self(96);
    /// 192 pixels.
    pub const LARGE: Self = Self(192);
    /// 384 pixels.
    pub const LARGE_2X: Self = Self(384);
    /// Every size, smallest first.
    pub const ALL: [Self; 4] = [Self::SMALL, Self::SMALL_2X, Self::LARGE, Self::LARGE_2X];

    /// The longest side in pixels.
    pub fn pixels(self) -> u32 {
        self.0
    }

    /// The size of `pixels`, if it is one of [`ALL`](Self::ALL).
    pub fn from_pixels(pixels: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.0 == pixels)
    }

    /// The smallest size at least `logical × scale` pixels, or the largest size.
    pub fn for_display(logical: u32, scale: u32) -> Self {
        let wanted = logical.saturating_mul(scale.max(1));
        Self::ALL
            .into_iter()
            .find(|s| s.0 >= wanted)
            .unwrap_or(Self::LARGE_2X)
    }
}

impl fmt::Debug for ThumbnailSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ThumbnailSize({})", self.0)
    }
}

impl fmt::Display for ThumbnailSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_sizes() {
        assert_eq!(ThumbnailSize::for_display(22, 1), ThumbnailSize::SMALL);
        assert_eq!(ThumbnailSize::for_display(44, 1), ThumbnailSize::SMALL);
        assert_eq!(ThumbnailSize::for_display(44, 2), ThumbnailSize::SMALL_2X);
        assert_eq!(ThumbnailSize::for_display(104, 1), ThumbnailSize::LARGE);
        assert_eq!(ThumbnailSize::for_display(160, 1), ThumbnailSize::LARGE);
        assert_eq!(ThumbnailSize::for_display(184, 2), ThumbnailSize::LARGE_2X);
        assert_eq!(ThumbnailSize::for_display(400, 2), ThumbnailSize::LARGE_2X);
        assert_eq!(ThumbnailSize::for_display(40, 0), ThumbnailSize::SMALL);
        assert_eq!(
            ThumbnailSize::from_pixels(96),
            Some(ThumbnailSize::SMALL_2X)
        );
        assert_eq!(ThumbnailSize::from_pixels(100), None);
    }
}
