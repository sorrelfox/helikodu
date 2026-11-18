// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parses arbitrary bytes as a playlist in PLS, writes the result back in the same
//! format and parses that again. The first byte picks the legacy charset the user is
//! taken to have chosen. Any panic is a bug, and so is a local entry that is not
//! absolute or still holds `.` or `..`, more entries than the limit, or more
//! warnings than the cap.

#![no_main]

use helikodu_meta::playlists::PlaylistKind;
use libfuzzer_sys::fuzz_target;

mod playlist_check;

fuzz_target!(|data: &[u8]| {
    playlist_check::run(PlaylistKind::Pls, data);
});
