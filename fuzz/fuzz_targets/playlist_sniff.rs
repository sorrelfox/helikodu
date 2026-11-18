// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parses arbitrary bytes as a playlist whose format is not known, as the
//! automatic import of playlist files found in the library does: the format is
//! sniffed from the bytes. The first byte picks the legacy charset. The checks are
//! those of the other playlist targets.

#![no_main]

use helikodu_meta::playlists::PlaylistKind;
use libfuzzer_sys::fuzz_target;

mod playlist_check;

fuzz_target!(|data: &[u8]| {
    let _ = PlaylistKind::sniff(data);
    playlist_check::run_sniffed(data);
});
