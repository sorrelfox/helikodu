// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Second-order IIR filters run in 64-bit float. The coefficients (Robert
//! Bristow-Johnson's "Audio EQ Cookbook") and their responses are in
//! [`helikodu_model::eq`], which the front ends use to draw the same response.
//!
//! The filters run in transposed direct form II, which keeps the rounding noise of
//! low-frequency filters small; the state of every channel and section is `f64`.

pub use helikodu_model::eq::{Coefficients, MAX_RELATIVE_FREQUENCY};

/// A cascade of biquad sections over the interleaved channels of a stream, with
/// 64-bit state for every channel and section.
#[derive(Debug, Clone)]
pub struct Cascade {
    sections: Box<[Coefficients]>,
    /// `[z1, z2]` per section and channel, section-major.
    state: Box<[[f64; 2]]>,
    channels: usize,
}

impl Cascade {
    /// A cascade of `sections` for `channels` channels, at rest.
    pub fn new(sections: Vec<Coefficients>, channels: usize) -> Self {
        let state = vec![[0.0; 2]; sections.len() * channels].into_boxed_slice();
        Self {
            sections: sections.into_boxed_slice(),
            state,
            channels,
        }
    }

    /// The sections.
    pub fn sections(&self) -> &[Coefficients] {
        &self.sections
    }

    /// Runs one sample of channel `channel` through every section.
    #[inline]
    pub fn tick(&mut self, channel: usize, mut x: f64) -> f64 {
        let channels = self.channels;
        for (i, c) in self.sections.iter().enumerate() {
            let z = &mut self.state[i * channels + channel];
            let y = c.b0 * x + z[0];
            z[0] = c.b1 * x - c.a1 * y + z[1];
            z[1] = c.b2 * x - c.a2 * y;
            x = y;
        }
        x
    }

    /// Sets state values too small to matter to zero, so that a decaying filter
    /// never reaches subnormal numbers.
    pub fn flush_tiny(&mut self) {
        for z in self.state.iter_mut().flatten() {
            if z.abs() < 1e-200 {
                *z = 0.0;
            }
        }
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.state.fill([0.0; 2]);
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;

    const RATE: f64 = 48_000.0;

    #[test]
    fn filtering_a_sine_gives_the_computed_gain() {
        let c = Coefficients::peaking(RATE, 1000.0, 1.4, -6.0);
        let mut cascade = Cascade::new(vec![c], 1);
        let mut peak: f64 = 0.0;
        for n in 0..48_000 {
            let x = (2.0 * PI * 1000.0 * f64::from(n) / RATE).sin();
            let y = cascade.tick(0, x);
            if n > 24_000 {
                peak = peak.max(y.abs());
            }
        }
        assert!((20.0 * peak.log10() + 6.0).abs() < 0.01, "{peak}");
    }
}
