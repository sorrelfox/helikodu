// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opens arbitrary bytes as a DSF or DSDIFF file with the own DSD reader, then
//! reads the raw stream, converts some of it to PCM and packs some into DoP, with
//! seeks to the middle, past the end and back. Any panic, hang or huge allocation
//! is a bug, and so is a read that returns more than was asked for or than there is.

#![no_main]

use std::io::{Seek, SeekFrom, Write};

use helikodu_extra_decoders::dsd::{DopDecoder, DsdPcmDecoder, DsdReader};
use libfuzzer_sys::fuzz_target;

fn input_file(data: &[u8]) -> Option<std::fs::File> {
    let path = std::env::temp_dir().join(format!("helikodu-fuzz-dsd-{}", std::process::id()));
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

fn reader(data: &[u8]) -> Option<DsdReader> {
    DsdReader::from_file(input_file(data)?).ok()
}

fuzz_target!(|data: &[u8]| {
    let Some(mut raw) = reader(data) else { return };
    let channels = raw.info().channels();
    assert!(channels > 0);
    let total = raw.bytes_per_channel();
    let mut out = vec![Vec::new(); channels];
    for position in [0, total / 2, total, total.saturating_add(1), u64::MAX, 0] {
        raw.seek(position);
        for _ in 0..4 {
            match raw.read(4096, &mut out) {
                Ok(0) | Err(_) => break,
                Ok(n) => assert!(n <= 4096),
            }
        }
    }
    if let Some(pcm) = reader(data).and_then(|r| DsdPcmDecoder::new(r).ok()) {
        let mut pcm = pcm;
        let frames = pcm.frames();
        let mut samples = Vec::new();
        for frame in [0, frames / 2, frames, u64::MAX] {
            pcm.seek(frame);
            for _ in 0..4 {
                samples.clear();
                match pcm.read(1024, &mut samples) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => assert!(n <= 1024),
                }
            }
        }
    }
    if let Some(r) = reader(data) {
        let mut dop = DopDecoder::new(r);
        let frames = dop.frames();
        let mut words = Vec::new();
        for frame in [0, frames / 2, frames, u64::MAX] {
            dop.seek(frame);
            words.clear();
            if let Ok(n) = dop.read(1024, &mut words) {
                assert!(n <= 1024);
            }
        }
    }
});
