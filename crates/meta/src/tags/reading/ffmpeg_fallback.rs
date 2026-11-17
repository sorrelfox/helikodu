// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading through FFmpeg, for the formats lofty lacks or fails on. The tags are
//! FFmpeg's metadata dictionary of the container and of the chosen audio stream.

use std::fs::File;

use helikodu_ffmpeg_bridge::{MediaFile, StreamChoice};

use super::TagError;
use super::text::TextFixer;
use crate::tags::builder::TagBuilder;
use crate::tags::mapping::ffmpeg;
use crate::tags::record::{AudioProperties, TrackTags};

pub(super) fn read(
    file: File,
    fixer: &mut TextFixer,
) -> Result<(TrackTags, AudioProperties), TagError> {
    let size = file.metadata().map(|m| m.len()).ok();
    let media = MediaFile::open_file(file)?;
    let stream = media.choose_stream(StreamChoice::Auto).ok();
    let tags = match stream {
        Some(index) => media.tags_with_stream(index),
        None => media.tags().clone(),
    };

    let mut builder = TagBuilder::new();
    builder.begin_tag();
    fixer.begin_tag(tags.iter().map(|(_, v)| v));
    for (key, value) in tags.iter() {
        let field = ffmpeg::field(key);
        for value in fixer.values(value) {
            match field {
                Some(field) => builder.apply(field, &value),
                None => builder.apply_unknown(key, &value),
            }
        }
    }

    let info = stream.and_then(|i| media.streams().get(i));
    let duration = info
        .and_then(|s| s.duration)
        .or_else(|| media.duration())
        .map(|d| d.as_duration())
        .filter(|d| !d.is_zero());
    let bitrate_kbps = match (size, duration) {
        (Some(size), Some(d)) if d.as_secs_f64() > 0.0 => {
            let kbps = size as f64 * 8.0 / d.as_secs_f64() / 1000.0;
            (kbps.is_finite() && kbps >= 1.0 && kbps < f64::from(u32::MAX))
                .then(|| kbps.round() as u32)
        }
        _ => None,
    };
    let container = media
        .format_name()
        .split(',')
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let audio = AudioProperties {
        duration,
        sample_rate: info.and_then(|s| s.sample_rate).filter(|r| *r > 0),
        channels: info.and_then(|s| s.channels).filter(|c| *c > 0),
        bit_depth: info.and_then(|s| s.bits_per_sample).filter(|b| *b > 0),
        bitrate_kbps,
        codec: info.map(|s| s.codec.to_ascii_lowercase()),
        container,
    };
    Ok((builder.finish(), audio))
}
