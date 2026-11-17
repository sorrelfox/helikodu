// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opens arbitrary bytes as a media file through the FFmpeg bridge, with every
//! offline lock on, then reads the metadata, decodes a little, seeks and decodes
//! again. Any crash, sanitizer report or leak is a bug in the bridge or in FFmpeg.

#![no_main]

use std::io::{Seek, SeekFrom, Write};

use helikodu_ffmpeg_bridge::{LogLevel, MediaFile, PictureLimits, StreamChoice};
use helikodu_model::audio::MediaTime;
use libfuzzer_sys::fuzz_target;

/// Blocks decoded before and after the seek; enough to reach the decoders.
const BLOCKS: usize = 16;

fn input_file(data: &[u8]) -> Option<std::fs::File> {
    // The bridge reads only from a `File`; an unlinked temporary file keeps the
    // input off the disk's namespace.
    let path = std::env::temp_dir().join(format!("helikodu-fuzz-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .ok()?;
    let _ = std::fs::remove_file(&path);
    file.write_all(data).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    Some(file)
}

fuzz_target!(|data: &[u8]| {
    helikodu_ffmpeg_bridge::set_log_level(LogLevel::Quiet);
    let Some(file) = input_file(data) else { return };
    let Ok(media) = MediaFile::open_file(file) else { return };
    let _ = media.tags();
    let _ = media.chapters();
    let _ = media.duration();
    let _ = media.attached_pictures(&PictureLimits {
        max_bytes: 1 << 20,
        max_count: 4,
    });
    let Ok(mut decoder) = media.into_decoder(StreamChoice::Auto) else { return };
    for _ in 0..BLOCKS {
        match decoder.read() {
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    if decoder.seek(MediaTime::from_millis(500)).is_ok() {
        for _ in 0..BLOCKS {
            match decoder.read() {
                Ok(Some(_)) => {}
                _ => break,
            }
        }
    }
});
