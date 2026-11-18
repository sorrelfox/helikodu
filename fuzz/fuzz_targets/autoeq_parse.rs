// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parses arbitrary bytes as an AutoEQ `ParametricEQ.txt` file. Any panic is a bug,
//! and so is a result the settings would refuse: more than 64 filters, or a filter
//! out of range. What is read must read back the same after writing it out.

#![no_main]

use helikodu_engine::dsp::autoeq;
use helikodu_model::settings::EqualizerSettings;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(import) = autoeq::parse_bytes(data) else {
        return;
    };
    assert!(!import.filters.is_empty());
    let settings = EqualizerSettings {
        parametric: import.filters.clone(),
        ..EqualizerSettings::default()
    };
    if let Err(error) = settings.validate() {
        panic!("an imported filter is not valid: {error}");
    }
    for warning in &import.warnings {
        let _ = warning.to_string();
    }
    let text = autoeq::format(import.preamp_db, &import.filters);
    let again = autoeq::parse(&text).expect("written filters read back");
    assert_eq!(again.filters.len(), import.filters.len());
    for (a, b) in again.filters.iter().zip(&import.filters) {
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.enabled, b.enabled);
        assert!((a.frequency_hz - b.frequency_hz).abs() <= 0.005 + 1e-9);
        assert!((a.q - b.q).abs() <= 0.0005 + 1e-9);
        if a.kind.has_gain() {
            assert!((a.gain_db - b.gain_db).abs() <= 0.005 + 1e-9);
        }
    }
});
