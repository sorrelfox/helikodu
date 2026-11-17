// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fuzzes the cover picture decoder: format sniffing, header probing, decoding
//! within the limits and scaling into a thumbnail.

#![no_main]

use helikodu_meta::covers::{CoverLimits, PictureFormat, decode_picture, probe_picture};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The first byte picks the thumbnail size; the rest is the picture.
    let Some((&size, picture)) = data.split_first() else {
        return;
    };
    // Smaller limits than the defaults keep each run fast; the checks are the same.
    let limits = CoverLimits {
        max_bytes: 4 * 1024 * 1024,
        max_pixels: 4 * 1024 * 1024,
        max_count: 16,
    };
    let format = PictureFormat::sniff(picture);
    let probed = probe_picture(picture, &limits);
    if let Ok(info) = &probed {
        assert_eq!(Some(info.format), format);
        assert!(info.format.is_decodable());
        assert!(u64::from(info.width) * u64::from(info.height) <= limits.max_pixels);
    }
    let Ok(decoded) = decode_picture(picture, &limits) else {
        return;
    };
    // Whatever the header said, a decoded picture is within the limits and the
    // probe agreed on its size.
    assert!(u64::from(decoded.width()) * u64::from(decoded.height()) <= limits.max_pixels);
    assert_eq!(
        decoded.pixels().len(),
        decoded.width() as usize * decoded.height() as usize * 4
    );
    if let Ok(info) = probed {
        assert_eq!(
            (info.width, info.height),
            (decoded.width(), decoded.height())
        );
    }
    let side = u32::from(size).max(1);
    let thumb = decoded.scale_to_fit(side);
    assert!(thumb.width() <= side && thumb.height() <= side);
    assert!(thumb.width() >= 1 && thumb.height() >= 1);
});
