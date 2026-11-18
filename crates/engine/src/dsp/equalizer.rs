// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The equalizer stage: the preamp, then a cascade of biquads from the graphic or
//! the parametric equalizer.

use helikodu_model::settings::{
    EqMode, EqualizerSettings, GRAPHIC_10_BANDS_HZ, GRAPHIC_31_BANDS_HZ,
};

use super::DspStage;
use super::biquad::{Cascade, Coefficients};
use super::graphic::band_sections;
use super::plan::EqualizerPlan;

/// The preamp and the equalizer's filters, for one rate and channel count.
#[derive(Debug, Clone)]
pub struct Equalizer {
    cascade: Cascade,
    preamp: f64,
    channels: usize,
    plan: EqualizerPlan,
}

impl Equalizer {
    /// Builds the equalizer `settings` describe (whether or not it is enabled) for
    /// `rate` Hz and `channels` channels. A graphic equalizer has a section for every
    /// band below the Nyquist limit, a parametric one for every enabled filter that
    /// fits the rate.
    pub fn new(settings: &EqualizerSettings, rate: u32, channels: usize) -> Self {
        let rate_hz = f64::from(rate);
        let sections: Vec<Coefficients> = match settings.mode {
            EqMode::Graphic10 => band_sections(&GRAPHIC_10_BANDS_HZ, &settings.graphic_10, rate_hz),
            EqMode::Graphic31 => band_sections(&GRAPHIC_31_BANDS_HZ, &settings.graphic_31, rate_hz),
            EqMode::Parametric => settings
                .parametric
                .iter()
                .filter(|filter| filter.enabled)
                .filter_map(|filter| Coefficients::for_filter(filter, rate_hz))
                .collect(),
        };
        let preamp_db = settings.effective_preamp_db();
        let preamp_db = if preamp_db.is_finite() {
            preamp_db
        } else {
            0.0
        };
        let filters = sections.iter().filter(|c| !c.is_identity()).count();
        Self {
            cascade: Cascade::new(sections, channels.max(1)),
            preamp: 10f64.powf(preamp_db / 20.0),
            channels: channels.max(1),
            plan: EqualizerPlan {
                mode: settings.mode,
                filters,
                preamp_db,
            },
        }
    }

    /// Whether the equalizer leaves every sample unchanged: no preamp and no
    /// section with an effect.
    pub fn is_identity(&self) -> bool {
        self.preamp == 1.0
            && self
                .cascade
                .sections()
                .iter()
                .all(Coefficients::is_identity)
    }

    /// The sections, in order.
    pub fn sections(&self) -> &[Coefficients] {
        self.cascade.sections()
    }

    /// The linear preamp gain.
    pub fn preamp(&self) -> f64 {
        self.preamp
    }

    /// What the signal path shows.
    pub fn plan(&self) -> EqualizerPlan {
        self.plan
    }

    /// The magnitude of the whole equalizer's response at `frequency` Hz, in dB,
    /// preamp included.
    pub fn magnitude_db(&self, frequency: f64, rate: u32) -> f64 {
        let rate = f64::from(rate);
        20.0 * self.preamp.log10()
            + self
                .sections()
                .iter()
                .map(|c| c.magnitude_db(frequency, rate))
                .sum::<f64>()
    }
}

impl DspStage for Equalizer {
    fn name(&self) -> &'static str {
        "equalizer"
    }

    fn process(&mut self, block: &mut [f32]) {
        for frame in block.chunks_exact_mut(self.channels) {
            for (channel, sample) in frame.iter_mut().enumerate() {
                let x = f64::from(*sample) * self.preamp;
                *sample = self.cascade.tick(channel, x) as f32;
            }
        }
        self.cascade.flush_tiny();
    }

    fn reset(&mut self) {
        self.cascade.reset();
    }
}

#[cfg(test)]
mod tests {
    use helikodu_model::settings::{FilterKind, ParametricFilter};

    use super::*;

    #[test]
    fn a_flat_equalizer_is_an_identity() {
        let mut settings = EqualizerSettings {
            mode: EqMode::Graphic31,
            ..EqualizerSettings::default()
        };
        assert!(Equalizer::new(&settings, 48_000, 2).is_identity());
        settings.graphic_31[4] = 1.5;
        let eq = Equalizer::new(&settings, 48_000, 2);
        assert!(!eq.is_identity());
        assert_eq!(eq.plan().preamp_db, -1.5);
        assert!((eq.magnitude_db(GRAPHIC_31_BANDS_HZ[4], 48_000)).abs() < 0.05);
    }

    #[test]
    fn disabled_and_unusable_filters_are_left_out() {
        let settings = EqualizerSettings {
            parametric: vec![
                ParametricFilter {
                    gain_db: -3.0,
                    ..ParametricFilter::default()
                },
                ParametricFilter {
                    gain_db: 4.0,
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
        let eq = Equalizer::new(&settings, 44_100, 2);
        assert_eq!(eq.sections().len(), 1);
        assert_eq!(eq.plan().filters, 1);
        // The automatic preamp counts the enabled filters' boosts, even one that
        // does not fit this rate.
        assert_eq!(eq.plan().preamp_db, -2.0);
    }
}
