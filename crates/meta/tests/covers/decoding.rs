// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding and thumbnails: the three formats, hostile pictures and the limits.

use std::time::{Duration, Instant};

use helikodu_meta::covers::{
    CoverError, CoverLimits, PictureFormat, RgbaPicture, decode_picture, make_thumbnail,
    probe_picture,
};

use crate::media::{GIF, jpeg, png, png_bomb, png_claiming, webp};

fn limits() -> CoverLimits {
    CoverLimits::default()
}

#[test]
fn the_three_formats_decode() {
    for (bytes, format) in [
        (jpeg(120, 80, [200, 100, 50]), PictureFormat::Jpeg),
        (png(120, 80, [200, 100, 50]), PictureFormat::Png),
        (webp(120, 80, [200, 100, 50]), PictureFormat::WebP),
    ] {
        assert_eq!(PictureFormat::sniff(&bytes), Some(format));
        let info = probe_picture(&bytes, &limits()).expect("probe");
        assert_eq!((info.format, info.width, info.height), (format, 120, 80));
        let picture = decode_picture(&bytes, &limits()).expect("decode");
        assert_eq!((picture.width(), picture.height()), (120, 80));
        assert_eq!(picture.pixels().len(), 120 * 80 * 4);
        assert!(picture.is_opaque());
        // The top-left pixel is the base colour (JPEG within its loss).
        let p = &picture.pixels()[..4];
        assert!(p[0].abs_diff(200) < 8 && p[1].abs_diff(100) < 8 && p[2].abs_diff(50) < 8);
    }
}

#[test]
fn other_formats_are_refused() {
    assert!(matches!(
        decode_picture(GIF, &limits()),
        Err(CoverError::UnsupportedFormat(PictureFormat::Gif))
    ));
    assert!(matches!(
        decode_picture(b"\0\0\0\x1cftypavif\0\0\0\0mif1", &limits()),
        Err(CoverError::UnsupportedFormat(PictureFormat::Avif))
    ));
    assert!(matches!(
        decode_picture(b"plain text", &limits()),
        Err(CoverError::UnknownFormat)
    ));
    assert!(matches!(
        decode_picture(b"", &limits()),
        Err(CoverError::UnknownFormat)
    ));
}

#[test]
fn a_huge_claim_is_refused_before_decoding() {
    let claim = png_claiming(100_000, 100_000);
    assert!(claim.len() < 100);
    let started = Instant::now();
    let err = probe_picture(&claim, &limits()).expect_err("refused");
    assert!(
        matches!(
            err,
            CoverError::BadDimensions {
                width: 100_000,
                height: 100_000,
                ..
            }
        ),
        "{err:?}"
    );
    let err = decode_picture(&claim, &limits()).expect_err("refused");
    assert!(matches!(err, CoverError::BadDimensions { .. }), "{err:?}");
    // Allocating and filling 40 GB would take far longer.
    assert!(started.elapsed() < Duration::from_secs(2));

    // Zero sides.
    let err = decode_picture(&png_claiming(0, 10), &limits()).expect_err("refused");
    assert!(
        matches!(
            err,
            CoverError::BadDimensions { .. } | CoverError::Decode(_)
        ),
        "{err:?}"
    );
}

#[test]
fn a_jpeg_claiming_65535_squared_is_refused() {
    let mut bytes = jpeg(16, 16, [1, 2, 3]);
    let sof = bytes
        .windows(2)
        .position(|w| w == [0xFF, 0xC0])
        .expect("a baseline frame header");
    // FF C0, length (2), precision (1), height (2), width (2).
    bytes[sof + 5..sof + 9].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    let err = decode_picture(&bytes, &limits()).expect_err("refused");
    assert!(
        matches!(
            err,
            CoverError::BadDimensions {
                width: 65_535,
                height: 65_535,
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn a_decompression_bomb_png_is_refused() {
    // 4096 × 4096 grey pixels in a few kilobytes: within the default limits,
    // but four times a 2048 × 2048 limit.
    let bomb = png_bomb(4096);
    assert!(bomb.len() < 100_000, "{} bytes", bomb.len());
    let tight = CoverLimits {
        max_pixels: 2048 * 2048,
        ..CoverLimits::default()
    };
    let err = decode_picture(&bomb, &tight).expect_err("refused");
    assert!(
        matches!(
            err,
            CoverError::BadDimensions {
                width: 4096,
                height: 4096,
                ..
            }
        ),
        "{err:?}"
    );
    // A thumbnail under the default limits works.
    let thumb = make_thumbnail(&bomb, 64, &limits()).expect("thumbnail");
    assert_eq!((thumb.width(), thumb.height()), (64, 64));
}

#[test]
fn the_byte_limit_comes_first() {
    let bytes = png(64, 64, [1, 2, 3]);
    let tight = CoverLimits {
        max_bytes: bytes.len() - 1,
        ..CoverLimits::default()
    };
    assert!(matches!(
        decode_picture(&bytes, &tight),
        Err(CoverError::TooLarge { .. })
    ));
}

#[test]
fn truncated_and_corrupt_pictures_do_not_panic() {
    let full = jpeg(256, 256, [90, 180, 30]);
    // Cut inside the header: an error.
    assert!(decode_picture(&full[..20], &limits()).is_err());
    // Cut inside the scan data: an error, or a picture of the declared size
    // (the JPEG decoder fills what is missing); never a panic.
    for cut in [full.len() / 2, full.len() - 3, 200] {
        match decode_picture(&full[..cut], &limits()) {
            Ok(p) => assert_eq!((p.width(), p.height()), (256, 256)),
            Err(e) => assert!(matches!(e, CoverError::Decode(_)), "{e:?}"),
        }
    }
    // Every prefix of a PNG and a WebP, and flipped bytes.
    for bytes in [png(32, 32, [1, 2, 3]), webp(32, 32, [1, 2, 3])] {
        for cut in (0..bytes.len()).step_by(7) {
            let _ = decode_picture(&bytes[..cut], &limits());
        }
        for i in (0..bytes.len()).step_by(5) {
            let mut bad = bytes.clone();
            bad[i] ^= 0x55;
            let _ = decode_picture(&bad, &limits());
        }
    }
    let png = png(32, 32, [1, 2, 3]);
    assert!(decode_picture(&png[..png.len() / 2], &limits()).is_err());
}

#[test]
fn thumbnails_keep_the_aspect_ratio() {
    let wide = jpeg(1000, 500, [10, 20, 30]);
    let thumb = make_thumbnail(&wide, 256, &limits()).expect("thumbnail");
    assert_eq!((thumb.width(), thumb.height()), (256, 128));

    let tall = png(300, 900, [10, 20, 30]);
    let thumb = make_thumbnail(&tall, 90, &limits()).expect("thumbnail");
    assert_eq!((thumb.width(), thumb.height()), (30, 90));

    // Never enlarged.
    let small = webp(40, 30, [10, 20, 30]);
    let thumb = make_thumbnail(&small, 256, &limits()).expect("thumbnail");
    assert_eq!((thumb.width(), thumb.height()), (40, 30));
}

#[test]
fn thumbnails_are_deterministic() {
    let bytes = png(640, 480, [250, 120, 10]);
    let a = make_thumbnail(&bytes, 128, &limits()).expect("thumbnail");
    let b = make_thumbnail(&bytes, 128, &limits()).expect("thumbnail");
    assert_eq!(a, b);
    assert_eq!(a.encode_png().expect("png"), b.encode_png().expect("png"));
    assert_eq!(
        a.encode_jpeg(85).expect("jpeg"),
        b.encode_jpeg(85).expect("jpeg")
    );
    // The colour survives scaling: the average stays near the source's.
    let avg = |p: &RgbaPicture, c: usize| {
        let n = p.pixels().len() / 4;
        p.pixels()
            .iter()
            .skip(c)
            .step_by(4)
            .map(|&v| u64::from(v))
            .sum::<u64>()
            / n as u64
    };
    let full = decode_picture(&bytes, &limits()).expect("decode");
    for c in 0..3 {
        assert!(avg(&a, c).abs_diff(avg(&full, c)) <= 2, "channel {c}");
    }
}

#[test]
fn encoded_thumbnails_decode_again() {
    let thumb =
        make_thumbnail(&jpeg(500, 500, [100, 150, 200]), 100, &limits()).expect("thumbnail");
    let png = thumb.encode_png().expect("png");
    let back = decode_picture(&png, &limits()).expect("decode the PNG");
    assert_eq!(back, thumb);
    let jpeg = thumb.encode_jpeg(90).expect("jpeg");
    assert_eq!(PictureFormat::sniff(&jpeg), Some(PictureFormat::Jpeg));
    let back = decode_picture(&jpeg, &limits()).expect("decode the JPEG");
    assert_eq!((back.width(), back.height()), (100, 100));
}
