// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What every long-tail target does with a decoder it opened: render a little of
//! each subsong, seek, render again, and check the interface's promises.

use helikodu_extra_decoders::song::{SongDecoder, SongError, SongOptions};

/// Frames rendered per call, and calls per subsong; enough to reach the players.
const FRAMES: usize = 512;
const CALLS: usize = 8;

/// Short lengths, so that a seek to the end stays cheap.
pub fn options() -> SongOptions {
    SongOptions {
        default_length: std::time::Duration::from_secs(2),
        fade: std::time::Duration::from_millis(200),
        ..SongOptions::default()
    }
}

/// Opens every subsong with `open` (at most 4) and exercises it.
pub fn exercise<D: SongDecoder>(open: impl Fn(Option<u32>) -> Result<D, SongError>) {
    let Ok(first) = open(None) else { return };
    let count = first.info().subsong_count().min(4);
    drop(first);
    for subsong in 0..count {
        let Ok(mut decoder) = open(Some(subsong)) else {
            continue;
        };
        let channels = usize::from(decoder.channels());
        let mut block = Vec::new();
        for _ in 0..CALLS {
            match decoder.read(FRAMES, &mut block) {
                Ok(0) | Err(_) => break,
                Ok(frames) => assert_eq!(block.len(), frames * channels),
            }
        }
        // Seeking renders from the start, so its cost grows with the position; a
        // file may claim hours. The seek goes at most a minute in.
        if let Some(frames) = decoder.frames()
            && decoder.seek((frames / 2).min(60 * 48_000)).is_ok()
        {
            let _ = decoder.read(FRAMES, &mut block);
        }
        if decoder.seek(0).is_ok() {
            let _ = decoder.read(FRAMES, &mut block);
        }
    }
}
