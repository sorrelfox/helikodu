// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opens arbitrary bytes as SID tunes through libsidplayfp (and the C++ shim): every subsong, rendering, seeking. Any crash,
//! sanitizer report, panic or leak is a bug in the decoder or its library.

#![no_main]

#[path = "song_check.rs"]
mod song_check;

use helikodu_extra_decoders::sid::SidDecoder;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = helikodu_extra_decoders::sid::probe(data);
    let options = song_check::options();
    song_check::exercise(|subsong| SidDecoder::open(data, subsong, &options));
});
