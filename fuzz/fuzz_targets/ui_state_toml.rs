// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads arbitrary text as `ui-state.toml`, the window state, as the window does
//! when it opens. Any panic is a bug, and so is a state that moving into range
//! changes twice, or that does not read back unchanged once written.

#![no_main]

use helikodu_model::settings::UiState;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(state) = toml::from_str::<UiState>(text) else {
        return;
    };
    let clamped = state.clamped();
    assert_eq!(clamped.clone().clamped(), clamped);
    let written = toml::to_string(&clamped).expect("the state writes as TOML");
    let back: UiState = toml::from_str(&written).expect("the state reads back");
    assert_eq!(back, clamped, "{written}");
});
