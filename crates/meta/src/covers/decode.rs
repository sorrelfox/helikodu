// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding pictures from untrusted files, and scaling them into thumbnails.
//!
//! The checks come in this order, each before the work it guards:
//! 1. the encoded size against [`CoverLimits::max_bytes`];
//! 2. the format, sniffed from the magic bytes: JPEG, PNG and WebP only;
//! 3. the header is parsed and the declared width × height checked against
//!    [`CoverLimits::max_pixels`], so a picture that claims 100000 × 100000
//!    pixels is refused before any pixel buffer exists;
//! 4. while decoding, the `image` crate's own [`Limits`] cap what the decoder
//!    allocates.
//!
//! The decoders are memory-safe Rust (zune-jpeg, png, image-webp). Malformed
//! and truncated input is an error, never a panic.

use std::io::Cursor;

use image::imageops::{self, FilterType};
use image::{
    DynamicImage, ExtendedColorType, ImageBuffer, ImageDecoder, ImageEncoder, ImageError,
    ImageReader, Limits, RgbImage, Rgba,
};

use super::error::CoverError;
use super::format::PictureFormat;
use super::limits::CoverLimits;

/// A picture's format and dimensions, read from its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureInfo {
    /// The sniffed format.
    pub format: PictureFormat,
    /// The width in pixels, after the stored orientation is applied.
    pub width: u32,
    /// The height in pixels, after the stored orientation is applied.
    pub height: u32,
}

/// A decoded picture: 8-bit RGBA, not premultiplied, rows top to bottom.
///
/// Thumbnails are this type too. RGBA rather than an encoded picture because
/// every consumer starts from pixels: a GTK memory texture takes them as they
/// are, and the thumbnail cache encodes them once in the format it stores
/// ([`encode_png`](Self::encode_png), or [`encode_jpeg`](Self::encode_jpeg) for
/// opaque pictures, which are most covers and much smaller as JPEG).
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaPicture {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl RgbaPicture {
    /// A picture from its pixels; `None` unless `pixels` holds exactly
    /// `width × height × 4` bytes and neither side is zero.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let len = u64::from(width) * u64::from(height) * 4;
        (width > 0 && height > 0 && pixels.len() as u64 == len).then_some(Self {
            width,
            height,
            pixels,
        })
    }

    /// The width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixels: `width × height` times R, G, B, A.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The pixels, consuming the picture.
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// Whether every pixel is fully opaque.
    pub fn is_opaque(&self) -> bool {
        self.pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255)
    }

    /// The picture scaled down to fit a `max_side` × `max_side` square, keeping
    /// its aspect ratio (each side at least one pixel). A picture that already
    /// fits is returned as it is; pictures are never enlarged.
    ///
    /// The filter is Lanczos 3: the sharpest of the `image` crate's filters,
    /// with little visible ringing when reducing. A picture more than four
    /// times the target size is first reduced to twice the target by averaging
    /// pixel areas, which costs little and gives the same result as Lanczos on
    /// the full picture to the eye. The result depends only on the input.
    pub fn scale_to_fit(&self, max_side: u32) -> RgbaPicture {
        let max_side = max_side.max(1);
        if self.width <= max_side && self.height <= max_side {
            return self.clone();
        }
        let (width, height) = fit(self.width, self.height, max_side);
        let Some(source) =
            ImageBuffer::<Rgba<u8>, &[u8]>::from_raw(self.width, self.height, &self.pixels)
        else {
            // Not reached: the size was checked when the picture was made.
            return self.clone();
        };
        let scaled = if self.width / 4 >= width && self.height / 4 >= height {
            let reduced = imageops::thumbnail(&source, width * 2, height * 2);
            imageops::resize(&reduced, width, height, FilterType::Lanczos3)
        } else {
            imageops::resize(&source, width, height, FilterType::Lanczos3)
        };
        Self {
            width,
            height,
            pixels: scaled.into_raw(),
        }
    }

    /// The picture as PNG: lossless, keeps transparency.
    pub fn encode_png(&self) -> Result<Vec<u8>, CoverError> {
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(
                &self.pixels,
                self.width,
                self.height,
                ExtendedColorType::Rgba8,
            )
            .map_err(|e| CoverError::Encode(e.to_string()))?;
        Ok(out)
    }

    /// The picture as baseline JPEG at `quality` (1–100). JPEG has no
    /// transparency: transparent parts are blended onto black, so use PNG for
    /// pictures that are not [opaque](Self::is_opaque).
    pub fn encode_jpeg(&self, quality: u8) -> Result<Vec<u8>, CoverError> {
        let mut rgb = Vec::with_capacity(self.pixels.len() / 4 * 3);
        for p in self.pixels.as_chunks::<4>().0 {
            let alpha = u16::from(p[3]);
            for &c in &p[..3] {
                // Rounded c × alpha / 255.
                let v = (u16::from(c) * alpha + 127) / 255;
                rgb.push(u8::try_from(v).unwrap_or(u8::MAX));
            }
        }
        let rgb = RgbImage::from_raw(self.width, self.height, rgb)
            .ok_or_else(|| CoverError::Encode("bad buffer size".to_owned()))?;
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100))
            .encode_image(&rgb)
            .map_err(|e| CoverError::Encode(e.to_string()))?;
        Ok(out)
    }
}

impl std::fmt::Debug for RgbaPicture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RgbaPicture")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// The size of a `width` × `height` picture scaled to fit `max_side`.
fn fit(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let scale = |long: u32, short: u32| {
        let v = (u64::from(short) * u64::from(max_side) + u64::from(long) / 2) / u64::from(long);
        u32::try_from(v).unwrap_or(max_side).clamp(1, max_side)
    };
    if width >= height {
        (max_side, scale(width, height))
    } else {
        (scale(height, width), max_side)
    }
}

/// The format and dimensions of the encoded picture `bytes`, read from its
/// header within `limits`. No pixel is decoded.
pub fn probe_picture(bytes: &[u8], limits: &CoverLimits) -> Result<PictureInfo, CoverError> {
    let (format, mut decoder) = open(bytes, limits)?;
    let (width, height) = decoder.dimensions();
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let (width, height) = if swaps_sides(orientation) {
        (height, width)
    } else {
        (width, height)
    };
    Ok(PictureInfo {
        format,
        width,
        height,
    })
}

/// Decodes the encoded picture `bytes` (JPEG, PNG or WebP) within `limits`,
/// applying its stored orientation.
pub fn decode_picture(bytes: &[u8], limits: &CoverLimits) -> Result<RgbaPicture, CoverError> {
    let (_, mut decoder) = open(bytes, limits)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut picture = DynamicImage::from_decoder(decoder).map_err(image_error)?;
    picture.apply_orientation(orientation);
    let rgba = picture.into_rgba8();
    let (width, height) = rgba.dimensions();
    RgbaPicture::new(width, height, rgba.into_raw())
        .ok_or_else(|| CoverError::Decode("the decoder returned no pixels".to_owned()))
}

/// Decodes `bytes` within `limits` and scales the picture to fit a `max_side` ×
/// `max_side` square (see [`RgbaPicture::scale_to_fit`]).
pub fn make_thumbnail(
    bytes: &[u8],
    max_side: u32,
    limits: &CoverLimits,
) -> Result<RgbaPicture, CoverError> {
    Ok(decode_picture(bytes, limits)?.scale_to_fit(max_side))
}

/// Checks the size and format, parses the header and checks the declared
/// dimensions.
fn open<'a>(
    bytes: &'a [u8],
    limits: &CoverLimits,
) -> Result<(PictureFormat, impl ImageDecoder + 'a), CoverError> {
    if bytes.len() > limits.max_bytes {
        return Err(CoverError::TooLarge {
            bytes: bytes.len() as u64,
            limit: limits.max_bytes as u64,
        });
    }
    let format = PictureFormat::sniff(bytes).ok_or(CoverError::UnknownFormat)?;
    let decoder_format = format
        .decoder_format()
        .ok_or(CoverError::UnsupportedFormat(format))?;

    // No side can be longer than the pixel limit allows with the other side at
    // one pixel; the exact product is checked below, with the dimensions in
    // the error.
    let max_side = u32::try_from(limits.max_pixels).unwrap_or(u32::MAX);
    let mut image_limits = Limits::default();
    image_limits.max_image_width = Some(max_side);
    image_limits.max_image_height = Some(max_side);
    image_limits.max_alloc = Some(limits.max_alloc());

    let mut reader = ImageReader::with_format(Cursor::new(bytes), decoder_format);
    reader.limits(image_limits);
    let decoder = reader.into_decoder().map_err(image_error)?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > limits.max_pixels {
        return Err(CoverError::BadDimensions {
            width,
            height,
            limit: limits.max_pixels,
        });
    }
    Ok((format, decoder))
}

fn swaps_sides(orientation: image::metadata::Orientation) -> bool {
    use image::metadata::Orientation::*;
    matches!(
        orientation,
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH
    )
}

fn image_error(e: ImageError) -> CoverError {
    match e {
        ImageError::Limits(e) => CoverError::LimitExceeded(e.to_string()),
        ImageError::IoError(e) => CoverError::Decode(e.to_string()),
        e => CoverError::Decode(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_the_aspect_ratio() {
        assert_eq!(fit(1000, 500, 100), (100, 50));
        assert_eq!(fit(500, 1000, 100), (50, 100));
        assert_eq!(fit(1000, 1000, 100), (100, 100));
        assert_eq!(fit(10_000, 1, 100), (100, 1));
        assert_eq!(fit(1, 10_000, 100), (1, 100));
        assert_eq!(fit(300, 200, 256), (256, 171));
    }

    #[test]
    fn new_checks_the_buffer() {
        assert!(RgbaPicture::new(2, 2, vec![0; 16]).is_some());
        assert!(RgbaPicture::new(2, 2, vec![0; 15]).is_none());
        assert!(RgbaPicture::new(0, 2, Vec::new()).is_none());
    }

    #[test]
    fn small_pictures_are_not_enlarged() {
        let p = RgbaPicture::new(3, 2, vec![255; 24]).expect("valid");
        assert_eq!(p.scale_to_fit(64), p);
        assert!(p.is_opaque());
    }

    #[test]
    fn jpeg_blends_transparency_onto_black() {
        let p = RgbaPicture::new(8, 8, [255, 255, 255, 0].repeat(64)).expect("valid");
        assert!(!p.is_opaque());
        let jpeg = p.encode_jpeg(90).expect("encodes");
        let back = decode_picture(&jpeg, &CoverLimits::default()).expect("decodes");
        assert!(back.pixels().as_chunks::<4>().0.iter().all(|px| px[0] < 8));
    }
}
