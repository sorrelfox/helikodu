// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The coefficients of second-order IIR filters from Robert Bristow-Johnson's
//! "Audio EQ Cookbook", in 64-bit float, and their frequency responses.

use std::f64::consts::PI;

use crate::settings::{FilterKind, ParametricFilter};

/// The highest centre or corner frequency a filter is built for, as a fraction of
/// the sample rate. Filters above it are left out: the bilinear transform crowds
/// them against the Nyquist frequency.
pub const MAX_RELATIVE_FREQUENCY: f64 = 0.475;

/// The coefficients of one biquad section, normalized so that `a0` is 1:
/// `H(z) = (b0 + b1·z⁻¹ + b2·z⁻²) / (1 + a1·z⁻¹ + a2·z⁻²)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefficients {
    /// Feed-forward coefficient of `x[n]`.
    pub b0: f64,
    /// Feed-forward coefficient of `x[n-1]`.
    pub b1: f64,
    /// Feed-forward coefficient of `x[n-2]`.
    pub b2: f64,
    /// Feedback coefficient of `y[n-1]`.
    pub a1: f64,
    /// Feedback coefficient of `y[n-2]`.
    pub a2: f64,
}

/// The intermediate values every cookbook filter starts from.
struct Design {
    cos: f64,
    alpha: f64,
}

impl Design {
    fn new(rate: f64, frequency: f64, q: f64) -> Self {
        let w0 = 2.0 * PI * frequency / rate;
        Self {
            cos: w0.cos(),
            alpha: w0.sin() / (2.0 * q),
        }
    }
}

impl Coefficients {
    /// The filter that passes everything unchanged.
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalized(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b0: b[0] / a[0],
            b1: b[1] / a[0],
            b2: b[2] / a[0],
            a1: a[1] / a[0],
            a2: a[2] / a[0],
        }
    }

    /// A peaking filter at `frequency` Hz with `gain_db` at its centre and quality `q`.
    pub fn peaking(rate: f64, frequency: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let Design { cos, alpha } = Design::new(rate, frequency, q);
        Self::normalized(
            [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
        )
    }

    /// A low shelf with its midpoint at `frequency` Hz and `gain_db` below it.
    pub fn low_shelf(rate: f64, frequency: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let Design { cos, alpha } = Design::new(rate, frequency, q);
        let k = 2.0 * a.sqrt() * alpha;
        Self::normalized(
            [
                a * ((a + 1.0) - (a - 1.0) * cos + k),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - k),
            ],
            [
                (a + 1.0) + (a - 1.0) * cos + k,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - k,
            ],
        )
    }

    /// A high shelf with its midpoint at `frequency` Hz and `gain_db` above it.
    pub fn high_shelf(rate: f64, frequency: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let Design { cos, alpha } = Design::new(rate, frequency, q);
        let k = 2.0 * a.sqrt() * alpha;
        Self::normalized(
            [
                a * ((a + 1.0) + (a - 1.0) * cos + k),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - k),
            ],
            [
                (a + 1.0) - (a - 1.0) * cos + k,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - k,
            ],
        )
    }

    /// A second-order low pass with its corner at `frequency` Hz.
    pub fn low_pass(rate: f64, frequency: f64, q: f64) -> Self {
        let Design { cos, alpha } = Design::new(rate, frequency, q);
        Self::normalized(
            [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    /// A second-order high pass with its corner at `frequency` Hz.
    pub fn high_pass(rate: f64, frequency: f64, q: f64) -> Self {
        let Design { cos, alpha } = Design::new(rate, frequency, q);
        Self::normalized(
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    /// The coefficients of a parametric filter at `rate`, or `None` when its
    /// frequency is too close to the Nyquist frequency (see
    /// [`MAX_RELATIVE_FREQUENCY`]) or its values are not finite.
    pub fn for_filter(filter: &ParametricFilter, rate: f64) -> Option<Self> {
        let ParametricFilter {
            kind,
            frequency_hz: f,
            gain_db: g,
            q,
            ..
        } = *filter;
        let usable = f > 0.0 && f < rate * MAX_RELATIVE_FREQUENCY && q > 0.0;
        if !usable || !g.is_finite() || !q.is_finite() {
            return None;
        }
        Some(match kind {
            FilterKind::Peak => Self::peaking(rate, f, q, g),
            FilterKind::LowShelf => Self::low_shelf(rate, f, q, g),
            FilterKind::HighShelf => Self::high_shelf(rate, f, q, g),
            FilterKind::LowPass => Self::low_pass(rate, f, q),
            FilterKind::HighPass => Self::high_pass(rate, f, q),
        })
    }

    /// Whether the section passes every signal unchanged: a peak or shelf without
    /// gain.
    pub fn is_identity(&self) -> bool {
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-12;
        close(self.b0, 1.0) && close(self.b1, self.a1) && close(self.b2, self.a2)
    }

    /// The complex response at `frequency` Hz, as (real, imaginary).
    pub fn response(&self, frequency: f64, rate: f64) -> (f64, f64) {
        let w = 2.0 * PI * frequency / rate;
        // z⁻¹ = e^{-jw}, z⁻² = e^{-2jw}.
        let (c1, s1) = (w.cos(), -w.sin());
        let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
        let num = (
            self.b0 + self.b1 * c1 + self.b2 * c2,
            self.b1 * s1 + self.b2 * s2,
        );
        let den = (
            1.0 + self.a1 * c1 + self.a2 * c2,
            self.a1 * s1 + self.a2 * s2,
        );
        let norm = den.0 * den.0 + den.1 * den.1;
        (
            (num.0 * den.0 + num.1 * den.1) / norm,
            (num.1 * den.0 - num.0 * den.1) / norm,
        )
    }

    /// The magnitude of the response at `frequency` Hz, in dB.
    pub fn magnitude_db(&self, frequency: f64, rate: f64) -> f64 {
        let (re, im) = self.response(frequency, rate);
        10.0 * (re * re + im * im).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    /// The magnitude of the cookbook's analog prototype `H(s)` at `s = jΩ`, where
    /// `Ω` is the digital frequency warped by the bilinear transform and normalized
    /// to the filter's frequency. Independent of the digital coefficients.
    fn analog_db(kind: FilterKind, f: f64, f0: f64, q: f64, gain_db: f64) -> f64 {
        let omega = (PI * f / RATE).tan() / (PI * f0 / RATE).tan();
        let a = 10f64.powf(gain_db / 40.0);
        // A polynomial `p2·s² + p1·s + p0` at `s = jΩ`: (p0 − p2·Ω², p1·Ω).
        let poly = |p2: f64, p1: f64, p0: f64| {
            let re = p0 - p2 * omega * omega;
            let im = p1 * omega;
            re * re + im * im
        };
        let power = match kind {
            FilterKind::LowPass => poly(0.0, 0.0, 1.0) / poly(1.0, 1.0 / q, 1.0),
            FilterKind::HighPass => poly(1.0, 0.0, 0.0) / poly(1.0, 1.0 / q, 1.0),
            FilterKind::Peak => poly(1.0, a / q, 1.0) / poly(1.0, 1.0 / (a * q), 1.0),
            FilterKind::LowShelf => a * a * poly(1.0, a.sqrt() / q, a) / poly(a, a.sqrt() / q, 1.0),
            FilterKind::HighShelf => {
                a * a * poly(a, a.sqrt() / q, 1.0) / poly(1.0, a.sqrt() / q, a)
            }
        };
        10.0 * power.log10()
    }

    #[test]
    fn magnitudes_match_the_cookbook_prototypes() {
        let kinds = [
            FilterKind::Peak,
            FilterKind::LowShelf,
            FilterKind::HighShelf,
            FilterKind::LowPass,
            FilterKind::HighPass,
        ];
        for kind in kinds {
            for (f0, q, gain) in [(100.0, 0.7, 6.0), (1000.0, 2.5, -9.0), (8000.0, 0.5, 12.0)] {
                let filter = ParametricFilter {
                    kind,
                    frequency_hz: f0,
                    gain_db: gain,
                    q,
                    enabled: true,
                };
                let c = Coefficients::for_filter(&filter, RATE).unwrap();
                let mut f = 20.0;
                while f < 20_000.0 {
                    let digital = c.magnitude_db(f, RATE);
                    let analog = analog_db(kind, f, f0, q, gain);
                    assert!(
                        (digital - analog).abs() < 1e-6,
                        "{kind:?} f0 {f0} at {f} Hz: {digital} vs {analog}"
                    );
                    f *= 1.07;
                }
            }
        }
    }

    #[test]
    fn landmarks_of_the_responses() {
        let peak = Coefficients::peaking(RATE, 1000.0, 1.0, 6.0);
        assert!((peak.magnitude_db(1000.0, RATE) - 6.0).abs() < 1e-9);
        assert!(peak.magnitude_db(20.0, RATE).abs() < 0.01);
        let shelf = Coefficients::low_shelf(RATE, 200.0, 0.707, -8.0);
        assert!((shelf.magnitude_db(200.0, RATE) + 4.0).abs() < 1e-9);
        assert!((shelf.magnitude_db(10.0, RATE) + 8.0).abs() < 0.05);
        let shelf = Coefficients::high_shelf(RATE, 4000.0, 0.707, 5.0);
        assert!((shelf.magnitude_db(4000.0, RATE) - 2.5).abs() < 1e-9);
        let lp = Coefficients::low_pass(RATE, 1000.0, std::f64::consts::FRAC_1_SQRT_2);
        assert!((lp.magnitude_db(1000.0, RATE) + 3.0103).abs() < 1e-3);
        assert!(lp.magnitude_db(10.0, RATE).abs() < 1e-3);
        assert!(Coefficients::peaking(RATE, 500.0, 1.0, 0.0).is_identity());
        assert!(!peak.is_identity());
    }

    #[test]
    fn filters_near_nyquist_are_left_out() {
        let filter = ParametricFilter {
            frequency_hz: 21_000.0,
            ..ParametricFilter::default()
        };
        assert!(Coefficients::for_filter(&filter, 44_100.0).is_none());
        assert!(Coefficients::for_filter(&filter, 48_000.0).is_some());
    }
}
