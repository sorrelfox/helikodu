// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The graphic equalizer's bands: constant-Q peaking filters at the ISO centre
//! frequencies, with gains corrected for the overlap of neighbouring bands.
//!
//! A peaking filter also lifts its neighbours' centres, so with the slider values as
//! the filter gains the response passes above the sliders where neighbours are
//! raised together. [`corrected_gains`] solves for the filter gains whose summed
//! response equals the slider value at every centre frequency, by Newton's method
//! on the exact (digital) responses. For slider values within ±12 dB the response
//! at each centre then meets its slider within [`TOLERANCE_DB`].

use super::biquad::{Coefficients, MAX_RELATIVE_FREQUENCY};

/// How closely the corrected response meets the slider values at the centre
/// frequencies, in dB.
pub const TOLERANCE_DB: f64 = 0.05;

/// The largest corrected filter gain, in dB. Only extreme zig-zag settings need
/// more; they then miss their sliders.
const MAX_GAIN_DB: f64 = 30.0;

/// The quality factor of the bands of a graphic equalizer with `bands` bands: one
/// octave wide for 10 bands, a third of an octave for 31.
pub fn band_q(bands: usize) -> f64 {
    let octaves: f64 = if bands >= 20 { 1.0 / 3.0 } else { 1.0 };
    let ratio = 2f64.powf(octaves);
    ratio.sqrt() / (ratio - 1.0)
}

/// The number of bands, from the lowest, whose centre frequencies are usable at
/// `rate`: those below [`MAX_RELATIVE_FREQUENCY`] of it.
pub fn usable_bands(centres: &[f64], rate: f64) -> usize {
    centres
        .iter()
        .take_while(|&&hz| hz < rate * MAX_RELATIVE_FREQUENCY)
        .count()
}

/// The filter gains, in dB, that make the summed response of the usable bands equal
/// `targets` at their centre frequencies. Bands above the usable ones are left out.
pub fn corrected_gains(centres: &[f64], targets: &[f64], rate: f64) -> Vec<f64> {
    let n = usable_bands(centres, rate).min(targets.len());
    let q = band_q(centres.len());
    let centres = &centres[..n];
    let targets: Vec<f64> = targets[..n]
        .iter()
        .map(|t| if t.is_finite() { *t } else { 0.0 })
        .collect();
    let mut gains = targets.clone();
    if targets.iter().all(|t| *t == 0.0) {
        return gains;
    }
    let band_db = |j: usize, gain: f64, at: f64| {
        Coefficients::peaking(rate, centres[j], q, gain).magnitude_db(at, rate)
    };
    let mut jacobian = vec![0.0; n * n];
    let mut error = vec![0.0; n];
    for _ in 0..40 {
        let mut worst: f64 = 0.0;
        for (i, e) in error.iter_mut().enumerate() {
            let response: f64 = (0..n).map(|j| band_db(j, gains[j], centres[i])).sum();
            *e = targets[i] - response;
            worst = worst.max(e.abs());
        }
        if worst < 1e-7 {
            break;
        }
        const H: f64 = 1e-4;
        for j in 0..n {
            for i in 0..n {
                jacobian[i * n + j] = (band_db(j, gains[j] + H, centres[i])
                    - band_db(j, gains[j] - H, centres[i]))
                    / (2.0 * H);
            }
        }
        if !solve(&mut jacobian, &mut error, n) {
            break;
        }
        for (gain, step) in gains.iter_mut().zip(&error) {
            *gain = (*gain + step).clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
        }
    }
    gains
}

/// The peaking sections of the usable bands with the corrected gains.
pub fn band_sections(centres: &[f64], targets: &[f64], rate: f64) -> Vec<Coefficients> {
    let q = band_q(centres.len());
    corrected_gains(centres, targets, rate)
        .iter()
        .zip(centres)
        .map(|(gain, hz)| Coefficients::peaking(rate, *hz, q, *gain))
        .collect()
}

/// Solves `a·x = b` in place by Gaussian elimination with partial pivoting; `x`
/// replaces `b`. Returns false for a singular matrix.
fn solve(a: &mut [f64], b: &mut [f64], n: usize) -> bool {
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&r, &s| a[r * n + col].abs().total_cmp(&a[s * n + col].abs()))
            .unwrap_or(col);
        if a[pivot * n + col].abs() < 1e-12 {
            return false;
        }
        if pivot != col {
            for k in 0..n {
                a.swap(pivot * n + k, col * n + k);
            }
            b.swap(pivot, col);
        }
        for row in col + 1..n {
            let factor = a[row * n + col] / a[col * n + col];
            if factor != 0.0 {
                for k in col..n {
                    a[row * n + k] -= factor * a[col * n + k];
                }
                b[row] -= factor * b[col];
            }
        }
    }
    for row in (0..n).rev() {
        let sum: f64 = (row + 1..n).map(|k| a[row * n + k] * b[k]).sum();
        b[row] = (b[row] - sum) / a[row * n + row];
    }
    true
}

#[cfg(test)]
mod tests {
    use crate::settings::{BuiltInPreset, GRAPHIC_10_BANDS_HZ, GRAPHIC_31_BANDS_HZ};

    use super::*;

    fn response(sections: &[Coefficients], hz: f64, rate: f64) -> f64 {
        sections.iter().map(|c| c.magnitude_db(hz, rate)).sum()
    }

    /// The largest miss of the response at the usable centres.
    fn worst_miss(centres: &[f64], targets: &[f64], rate: f64) -> f64 {
        let sections = band_sections(centres, targets, rate);
        sections
            .iter()
            .zip(centres)
            .zip(targets)
            .map(|((_, hz), t)| (response(&sections, *hz, rate) - t).abs())
            .fold(0.0, f64::max)
    }

    /// A simple deterministic generator of slider settings in 0.5 dB steps.
    fn sliders(seed: &mut u64, n: usize) -> Vec<f64> {
        (0..n)
            .map(|_| {
                *seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                f64::from(((*seed >> 33) % 49) as u32) * 0.5 - 12.0
            })
            .collect()
    }

    #[test]
    fn the_bandwidths() {
        assert!((band_q(10) - std::f64::consts::SQRT_2).abs() < 1e-12);
        assert!((band_q(31) - 4.318).abs() < 1e-3);
    }

    #[test]
    fn the_response_meets_the_sliders() {
        let mut seed = 7;
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            for centres in [&GRAPHIC_10_BANDS_HZ[..], &GRAPHIC_31_BANDS_HZ[..]] {
                for _ in 0..20 {
                    let targets = sliders(&mut seed, centres.len());
                    let miss = worst_miss(centres, &targets, rate);
                    assert!(miss < TOLERANCE_DB, "{rate} Hz {targets:?}: {miss} dB");
                }
                for preset in &BuiltInPreset::ALL {
                    let targets = if centres.len() == 10 {
                        preset.octaves.to_vec()
                    } else {
                        preset.third_octaves().to_vec()
                    };
                    assert!(worst_miss(centres, &targets, rate) < TOLERANCE_DB);
                }
                // Neighbours all raised to the top: uncorrected, the response would
                // pass far above them.
                let all_up = vec![12.0; centres.len()];
                assert!(worst_miss(centres, &all_up, rate) < TOLERANCE_DB);
                let zigzag: Vec<f64> = (0..centres.len())
                    .map(|i| if i % 2 == 0 { 12.0 } else { -12.0 })
                    .collect();
                assert!(worst_miss(centres, &zigzag, rate) < TOLERANCE_DB);
            }
        }
    }

    #[test]
    fn without_correction_the_curve_would_miss() {
        let rate = 48_000.0;
        let q = band_q(10);
        let targets = [6.0; 10];
        let raw: Vec<Coefficients> = GRAPHIC_10_BANDS_HZ
            .iter()
            .zip(targets)
            .map(|(hz, g)| Coefficients::peaking(rate, *hz, q, g))
            .collect();
        assert!(response(&raw, 1000.0, rate) > 8.0);
        assert!(worst_miss(&GRAPHIC_10_BANDS_HZ, &targets, rate) < TOLERANCE_DB);
    }

    #[test]
    fn bands_beyond_nyquist_are_left_out() {
        assert_eq!(usable_bands(&GRAPHIC_31_BANDS_HZ, 44_100.0), 31);
        assert_eq!(usable_bands(&GRAPHIC_31_BANDS_HZ, 32_000.0), 29);
        assert_eq!(usable_bands(&GRAPHIC_10_BANDS_HZ, 22_050.0), 9);
        let flat = corrected_gains(&GRAPHIC_10_BANDS_HZ, &[0.0; 10], 48_000.0);
        assert_eq!(flat, vec![0.0; 10]);
    }
}
