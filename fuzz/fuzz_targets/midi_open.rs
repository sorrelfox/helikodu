// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opens arbitrary bytes as MIDI files through rustysynth, with a SoundFont written by the target: every subsong, rendering, seeking. Any crash,
//! sanitizer report, panic or leak is a bug in the decoder or its library.
//!
//! The decoder catches panics inside rustysynth and refuses the file, so this
//! target prints caught panics (bugs to report upstream) instead of aborting at
//! once; a panic that is not caught still ends the run.

#![no_main]

#[path = "song_check.rs"]
mod song_check;

use helikodu_extra_decoders::midi::MidiDecoder;
use libfuzzer_sys::fuzz_target;

/// A SoundFont for the target: one looped wave over the whole keyboard.
fn soundfont() -> std::path::PathBuf {
    static PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("helikodu-fuzz-{}.sf2", std::process::id()));
        std::fs::write(&path, longtail::soundfont()).expect("the SoundFont is written");
        path
    })
    .clone()
}

#[allow(dead_code)]
#[path = "../../crates/testkit/src/longtail.rs"]
mod longtail;

fuzz_target!(|data: &[u8]| {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| eprintln!("panic: {info}")));
    });
    let _ = helikodu_extra_decoders::midi::probe(data);
    let _ = helikodu_extra_decoders::midi::read_info(data);
    let options = helikodu_extra_decoders::song::SongOptions {
        soundfont: Some(soundfont()),
        ..song_check::options()
    };
    song_check::exercise(|subsong| MidiDecoder::open(data, subsong, &options));
});
