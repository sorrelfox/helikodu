// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The picture types of ID3v2 `APIC` frames, shared by FLAC, Vorbis comments and
//! (by item key) APEv2.

use std::fmt;

/// An ID3v2 picture type (0–20; other values are kept as they are).
///
/// FLAC `PICTURE` blocks and Vorbis `METADATA_BLOCK_PICTURE` use the same
/// numbers, and APEv2 names them in its item keys (`Cover Art (Front)`). MP4
/// `covr` atoms have no type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PictureType(pub u8);

/// The names of types 0–20, as FFmpeg writes them into a picture stream's
/// `comment`.
const FFMPEG_NAMES: [&str; 21] = [
    "Other",
    "32x32 pixels 'file icon'",
    "Other file icon",
    "Cover (front)",
    "Cover (back)",
    "Leaflet page",
    "Media (e.g. label side of CD)",
    "Lead artist/lead performer/soloist",
    "Artist/performer",
    "Conductor",
    "Band/Orchestra",
    "Composer",
    "Lyricist/text writer",
    "Recording Location",
    "During recording",
    "During performance",
    "Movie/video screen capture",
    "A bright coloured fish",
    "Illustration",
    "Band/artist logotype",
    "Publisher/Studio logotype",
];

/// The APEv2 item key suffixes of types 0–20, inside `Cover Art (…)`.
const APE_NAMES: [&str; 21] = [
    "Other",
    "Png Icon",
    "Icon",
    "Front",
    "Back",
    "Leaflet",
    "Media",
    "Lead Artist",
    "Artist",
    "Conductor",
    "Band",
    "Composer",
    "Lyricist",
    "Recording Location",
    "During Recording",
    "During Performance",
    "Video Capture",
    "Fish",
    "Illustration",
    "Band Logotype",
    "Publisher Logotype",
];

impl PictureType {
    /// Other.
    pub const OTHER: Self = Self(0);
    /// A 32×32 PNG file icon.
    pub const FILE_ICON: Self = Self(1);
    /// Another file icon.
    pub const OTHER_FILE_ICON: Self = Self(2);
    /// The front cover.
    pub const FRONT_COVER: Self = Self(3);
    /// The back cover.
    pub const BACK_COVER: Self = Self(4);
    /// A leaflet page.
    pub const LEAFLET: Self = Self(5);
    /// The medium, such as the label side of a CD.
    pub const MEDIA: Self = Self(6);
    /// An illustration.
    pub const ILLUSTRATION: Self = Self(18);

    /// The type an FFmpeg picture stream's `comment` names, such as
    /// `Cover (front)`.
    pub fn from_ffmpeg_name(name: &str) -> Option<Self> {
        let name = name.trim();
        FFMPEG_NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
            .and_then(|i| u8::try_from(i).ok())
            .map(Self)
    }

    /// The type an APEv2 item key names (`Cover Art (Front)`, in any case), or
    /// `None` when the key is not a cover art key. An unknown name in a cover
    /// art key is [`OTHER`](Self::OTHER).
    pub fn from_ape_key(key: &str) -> Option<Self> {
        let prefix = "cover art (";
        let head = key.get(..prefix.len())?;
        if !head.eq_ignore_ascii_case(prefix) {
            return None;
        }
        let name = key[prefix.len()..].strip_suffix(')').unwrap_or("");
        let index = APE_NAMES.iter().position(|n| n.eq_ignore_ascii_case(name));
        Some(
            index
                .and_then(|i| u8::try_from(i).ok())
                .map_or(Self::OTHER, Self),
        )
    }

    /// How well a picture of this type serves as the album cover: lower is
    /// better. A missing type (an MP4 `covr` atom, say) counts as
    /// [`OTHER`](Self::OTHER).
    ///
    /// 0: the front cover. 1: other or untyped. 2: every other type (leaflet,
    /// medium, artists, illustration, …). 3: the back cover. 4: file icons.
    pub fn cover_rank(kind: Option<Self>) -> u8 {
        match kind {
            Some(Self::FRONT_COVER) => 0,
            None | Some(Self::OTHER) => 1,
            Some(Self::BACK_COVER) => 3,
            Some(Self::FILE_ICON | Self::OTHER_FILE_ICON) => 4,
            Some(_) => 2,
        }
    }

    /// A short English name, such as `Cover (front)`.
    pub fn name(self) -> Option<&'static str> {
        FFMPEG_NAMES.get(usize::from(self.0)).copied()
    }
}

impl fmt::Debug for PictureType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "PictureType({} {name:?})", self.0),
            None => write!(f, "PictureType({})", self.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_names() {
        assert_eq!(
            PictureType::from_ffmpeg_name("Cover (front)"),
            Some(PictureType::FRONT_COVER)
        );
        assert_eq!(
            PictureType::from_ffmpeg_name("cover (back)"),
            Some(PictureType::BACK_COVER)
        );
        assert_eq!(
            PictureType::from_ffmpeg_name("Publisher/Studio logotype"),
            Some(PictureType(20))
        );
        assert_eq!(PictureType::from_ffmpeg_name("cover.jpg"), None);
    }

    #[test]
    fn ape_keys() {
        assert_eq!(
            PictureType::from_ape_key("Cover Art (Front)"),
            Some(PictureType::FRONT_COVER)
        );
        assert_eq!(
            PictureType::from_ape_key("COVER ART (BACK)"),
            Some(PictureType::BACK_COVER)
        );
        assert_eq!(
            PictureType::from_ape_key("Cover Art (Whatever)"),
            Some(PictureType::OTHER)
        );
        assert_eq!(PictureType::from_ape_key("Cover"), None);
        assert_eq!(PictureType::from_ape_key("Title"), None);
    }

    #[test]
    fn ranks() {
        let rank = PictureType::cover_rank;
        assert!(rank(Some(PictureType::FRONT_COVER)) < rank(None));
        assert_eq!(rank(None), rank(Some(PictureType::OTHER)));
        assert!(rank(None) < rank(Some(PictureType::MEDIA)));
        assert!(rank(Some(PictureType::MEDIA)) < rank(Some(PictureType::BACK_COVER)));
        assert!(rank(Some(PictureType::BACK_COVER)) < rank(Some(PictureType::FILE_ICON)));
        assert!(rank(Some(PictureType(200))) < rank(Some(PictureType::BACK_COVER)));
    }
}
