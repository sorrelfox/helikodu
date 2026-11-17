// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Picture formats, recognised by their magic bytes.

/// How many leading bytes [`PictureFormat::sniff`] looks at.
pub const SNIFF_LEN: usize = 16;

/// The format of an encoded picture, as its leading bytes show it.
///
/// Only [`Jpeg`](Self::Jpeg), [`Png`](Self::Png) and [`WebP`](Self::WebP) are
/// decoded; the others are recognised so that they can be reported by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PictureFormat {
    /// JPEG (JFIF, Exif or bare).
    Jpeg,
    /// PNG.
    Png,
    /// WebP, lossy or lossless.
    WebP,
    /// GIF.
    Gif,
    /// Windows bitmap.
    Bmp,
    /// TIFF.
    Tiff,
    /// AVIF.
    Avif,
    /// HEIF/HEIC.
    Heif,
    /// JPEG XL, bare codestream or container.
    JpegXl,
}

impl PictureFormat {
    /// The format whose magic bytes `bytes` starts with, or `None` when it is not
    /// a picture format known here. Looks at most at [`SNIFF_LEN`] bytes; the
    /// declared media type and the file extension are never consulted.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        let at = |offset: usize, magic: &[u8]| {
            bytes
                .get(offset..offset + magic.len())
                .is_some_and(|b| b == magic)
        };
        if at(0, &[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if at(0, b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if at(0, b"RIFF") && at(8, b"WEBP") {
            Some(Self::WebP)
        } else if at(0, b"GIF87a") || at(0, b"GIF89a") {
            Some(Self::Gif)
        } else if at(0, b"II*\0") || at(0, b"MM\0*") {
            Some(Self::Tiff)
        } else if at(0, &[0xFF, 0x0A]) || at(0, b"\0\0\0\x0cJXL \r\n\x87\n") {
            Some(Self::JpegXl)
        } else if at(4, b"ftyp") {
            match bytes.get(8..12)? {
                b"avif" | b"avis" => Some(Self::Avif),
                b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"mif1" | b"msf1" => {
                    Some(Self::Heif)
                }
                _ => None,
            }
        } else if at(0, b"BM") && bytes.len() >= 14 {
            Some(Self::Bmp)
        } else {
            None
        }
    }

    /// Whether this crate decodes the format: JPEG, PNG and WebP.
    pub fn is_decodable(self) -> bool {
        matches!(self, Self::Jpeg | Self::Png | Self::WebP)
    }

    /// The media type, such as `image/jpeg`.
    pub fn mime_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::WebP => "image/webp",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
            Self::Tiff => "image/tiff",
            Self::Avif => "image/avif",
            Self::Heif => "image/heif",
            Self::JpegXl => "image/jxl",
        }
    }

    /// The usual file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Tiff => "tif",
            Self::Avif => "avif",
            Self::Heif => "heic",
            Self::JpegXl => "jxl",
        }
    }

    /// The `image` crate's format, for the decodable formats.
    pub(crate) fn decoder_format(self) -> Option<image::ImageFormat> {
        match self {
            Self::Jpeg => Some(image::ImageFormat::Jpeg),
            Self::Png => Some(image::ImageFormat::Png),
            Self::WebP => Some(image::ImageFormat::WebP),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_bytes() {
        let cases: &[(&[u8], Option<PictureFormat>)] = &[
            (b"\xFF\xD8\xFF\xE0\0\x10JFIF\0", Some(PictureFormat::Jpeg)),
            (b"\xFF\xD8\xFF\xE1\0\x10Exif\0", Some(PictureFormat::Jpeg)),
            (b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR", Some(PictureFormat::Png)),
            (b"RIFF\x10\0\0\0WEBPVP8 ", Some(PictureFormat::WebP)),
            (b"RIFF\x10\0\0\0WAVEfmt ", None),
            (b"GIF89a\x01\0\x01\0", Some(PictureFormat::Gif)),
            (b"BM\x36\0\0\0\0\0\0\0\x36\0\0\0", Some(PictureFormat::Bmp)),
            (b"BM", None),
            (b"II*\0\x08\0\0\0", Some(PictureFormat::Tiff)),
            (b"\0\0\0\x1cftypavif\0\0\0\0", Some(PictureFormat::Avif)),
            (b"\0\0\0\x18ftypheic\0\0\0\0", Some(PictureFormat::Heif)),
            (b"\0\0\0\x18ftypM4A \0\0\0\0", None),
            (b"\xFF\x0A\xFA", Some(PictureFormat::JpegXl)),
            (b"\0\0\0\x0cJXL \r\n\x87\n", Some(PictureFormat::JpegXl)),
            (b"<html><body>", None),
            (b"\xFF\xD8", None),
            (b"", None),
        ];
        for (bytes, expected) in cases {
            assert_eq!(PictureFormat::sniff(bytes), *expected, "{bytes:?}");
        }
    }

    #[test]
    fn only_three_formats_decode() {
        assert!(PictureFormat::Jpeg.is_decodable());
        assert!(PictureFormat::Png.is_decodable());
        assert!(PictureFormat::WebP.is_decodable());
        assert!(!PictureFormat::Gif.is_decodable());
        assert!(!PictureFormat::Avif.is_decodable());
        assert!(PictureFormat::Avif.decoder_format().is_none());
    }
}
