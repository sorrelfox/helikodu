// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads arbitrary bytes as `settings.toml`. Any panic or hang is a bug, and so
//! are settings that do not validate (unless they come from a newer version), or
//! that do not read back unchanged and without complaint once written.

#![no_main]

use helikodu_service::settings::{LoadNotice, read_settings};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (settings, notice) = read_settings(data);
    if matches!(notice, Some(LoadNotice::Newer { .. })) {
        return;
    }
    assert!(settings.validate().is_ok(), "{notice:?}");
    let text = toml::to_string(&settings).expect("settings write as TOML");
    let (again, notice) = read_settings(text.as_bytes());
    assert!(notice.is_none(), "{notice:?} in {text}");
    assert_eq!(again, settings, "{text}");
});
