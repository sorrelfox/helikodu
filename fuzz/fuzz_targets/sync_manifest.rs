// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads arbitrary bytes as the manifest of a sync destination,
//! `.helikodu-sync.json`. The file lies on the destination (a phone, a memory
//! stick), where anyone may have written it, and a sync removes the files it lists.
//! Any panic is a bug, and so is an entry whose path is not a plain relative path
//! inside the destination.

#![no_main]

use std::path::Component;

use helikodu_service::convert::Manifest;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let manifest = Manifest::from_json(data);
    for entry in &manifest.files {
        assert!(entry.path.components().next().is_some());
        assert!(
            entry
                .path
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
            "{:?}",
            entry.path
        );
    }
});
