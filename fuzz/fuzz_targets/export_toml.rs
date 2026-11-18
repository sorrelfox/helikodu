// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads arbitrary text as `export.toml`: the converter's presets and choices and
//! the sync destinations. Then uses every preset and destination the way the
//! converter and folder sync do: its settings, size estimates for extreme lengths,
//! the copy-as-is test, and the destination's naming pattern and character set.
//! Any panic is a bug, and so is a state that does not read back once written.

#![no_main]

use helikodu_service::convert::{CharacterSet, ExportState, FieldMap, NamingPattern};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(state) = toml::from_str::<ExportState>(text) else {
        return;
    };
    if let Ok(written) = toml::to_string(&state) {
        let back: ExportState = toml::from_str(&written).expect("the state reads back");
        assert_eq!(back, state, "{written}");
    }
    let _ = state.new_preset_id();
    let _ = state.new_destination_id();
    let _ = state.preset(&state.converter.preset);
    for preset in state.all_presets() {
        let _ = preset.settings();
        let _ = preset.is_builtin();
        for seconds in [0.0, 1.0, 1e12, f64::MAX, f64::NAN, -1.0] {
            for (rate, channels, bits) in
                [(0, 0, 0), (44_100, 2, 16), (u32::MAX, u16::MAX, u32::MAX)]
            {
                let _ = preset.expected_bytes(seconds, rate, channels, bits);
            }
        }
        for codec in ["flac", "opus", "mp3", "pcm_s16le", ""] {
            for kbps in [None, Some(0), Some(320), Some(u32::MAX)] {
                for rate in [None, Some(44_100), Some(u32::MAX)] {
                    let _ = preset.takes_as_is(codec, kbps, rate);
                }
            }
        }
    }
    let values = FieldMap::default();
    for destination in &state.destinations {
        let pattern = NamingPattern::parse(&destination.pattern).unwrap_or_default();
        let style = CharacterSet::from_id(&destination.names).unwrap_or_default();
        let _ = pattern.render_path(&values, style, "Unknown", ".opus");
        let _ = destination.is_connected();
    }
    let pattern = NamingPattern::parse(&state.converter.pattern).unwrap_or_default();
    let _ = pattern.render(&values, CharacterSet::default(), "Unknown", ".opus");
});
