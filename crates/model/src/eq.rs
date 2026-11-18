// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The equalizer's filter maths, shared by the engine that runs the filters and the
//! front ends that draw their response, so that the picture cannot disagree with
//! the sound. Pure arithmetic: no I/O, no state.
//!
//! - [`biquad`]: the RBJ cookbook coefficients of one section and its response.
//! - [`graphic`]: the bands of the graphic equalizers, with gains corrected for
//!   the overlap of neighbouring bands.
//! - [`sections`] builds the sections of an [`EqualizerSettings`] for a rate, and
//!   [`response_db`] gives the response of the whole equalizer.

pub mod biquad;
pub mod graphic;

pub use self::biquad::{Coefficients, MAX_RELATIVE_FREQUENCY};

use crate::settings::{EqMode, EqualizerSettings, GRAPHIC_10_BANDS_HZ, GRAPHIC_31_BANDS_HZ};

/// The biquad sections of the equalizer `settings` describe at `rate` Hz, whether
/// or not it is enabled: for a graphic equalizer a section for every band below the
/// Nyquist limit, with overlap-corrected gains; for the parametric one a section for
/// every enabled filter that fits the rate. The preamp is not included.
pub fn sections(settings: &EqualizerSettings, rate: f64) -> Vec<Coefficients> {
    match settings.mode {
        EqMode::Graphic10 => {
            graphic::band_sections(&GRAPHIC_10_BANDS_HZ, &settings.graphic_10, rate)
        }
        EqMode::Graphic31 => {
            graphic::band_sections(&GRAPHIC_31_BANDS_HZ, &settings.graphic_31, rate)
        }
        EqMode::Parametric => settings
            .parametric
            .iter()
            .filter(|filter| filter.enabled)
            .filter_map(|filter| Coefficients::for_filter(filter, rate))
            .collect(),
    }
}

/// The summed magnitude response of `sections` at `frequency` Hz, in dB, without
/// the preamp.
pub fn response_db(sections: &[Coefficients], frequency: f64, rate: f64) -> f64 {
    sections
        .iter()
        .map(|c| c.magnitude_db(frequency, rate))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FilterKind, ParametricFilter};

    #[test]
    fn graphic_sections_meet_the_sliders() {
        let mut settings = EqualizerSettings {
            mode: EqMode::Graphic10,
            ..EqualizerSettings::default()
        };
        settings.graphic_10 = [4.5, 3.5, 1.5, -1.0, -2.0, -1.0, 0.5, 2.5, 1.0, -2.0];
        let built = sections(&settings, 48_000.0);
        assert_eq!(built.len(), 10);
        for (hz, want) in GRAPHIC_10_BANDS_HZ.iter().zip(settings.graphic_10) {
            let got = response_db(&built, *hz, 48_000.0);
            assert!((got - want).abs() < graphic::TOLERANCE_DB, "{hz}: {got}");
        }
        settings.mode = EqMode::Graphic31;
        assert_eq!(sections(&settings, 48_000.0).len(), 31);
        assert!(response_db(&sections(&settings, 48_000.0), 1000.0, 48_000.0).abs() < 1e-9);
    }

    #[test]
    fn parametric_sections_follow_the_enabled_filters() {
        let settings = EqualizerSettings {
            parametric: vec![
                ParametricFilter {
                    kind: FilterKind::Peak,
                    frequency_hz: 1000.0,
                    gain_db: 6.0,
                    q: 1.0,
                    enabled: true,
                },
                ParametricFilter {
                    gain_db: -9.0,
                    enabled: false,
                    ..ParametricFilter::default()
                },
                ParametricFilter {
                    kind: FilterKind::HighShelf,
                    frequency_hz: 21_500.0,
                    gain_db: 2.0,
                    ..ParametricFilter::default()
                },
            ],
            ..EqualizerSettings::default()
        };
        let built = sections(&settings, 44_100.0);
        assert_eq!(built.len(), 1);
        assert!((response_db(&built, 1000.0, 44_100.0) - 6.0).abs() < 1e-9);
        assert_eq!(sections(&settings, 48_000.0).len(), 2);
        assert_eq!(response_db(&[], 440.0, 48_000.0), 0.0);
    }
}
