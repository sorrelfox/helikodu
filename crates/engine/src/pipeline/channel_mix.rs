// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Channel mapping, downmix and upmix by a mixing matrix.
//!
//! Each output channel is a weighted sum of the input channels. A channel whose
//! position exists on both sides is copied. A position the output lacks is folded
//! into its nearest neighbours with the ITU-R BS.775 coefficients: the centre goes
//! to left and right at −3 dB, surrounds go to the fronts at −3 dB, back and side
//! channels stand in for each other, and the LFE channel is dropped unless asked
//! for. A downmix is then scaled so that no output can exceed full scale.
//!
//! Layouts without positions are treated as the usual layout for their channel
//! count; where that is unknown too, channels are matched by index.

use std::f64::consts::FRAC_1_SQRT_2;
use std::fmt;

use helikodu_model::audio::{ChannelLayout, ChannelPosition};

use ChannelPosition as P;

/// Options for building a [`MixMatrix`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MixOptions {
    /// Mix the LFE channel into the front channels (at −3 dB) when the output has no
    /// LFE channel, instead of dropping it.
    pub include_lfe: bool,
    /// Scale a downmix so that the sum of any output's coefficients is at most 1.0,
    /// which keeps full-scale input from clipping.
    pub normalize: bool,
}

impl Default for MixOptions {
    /// LFE dropped, downmix normalized.
    fn default() -> Self {
        Self {
            include_lfe: false,
            normalize: true,
        }
    }
}

/// How a [`MixMatrix`] maps channels, from cheapest to most general.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixKind {
    /// Every channel stays where it is.
    Identity,
    /// Each output channel copies one input channel, or is silent.
    Route,
    /// Output channels are weighted sums.
    Matrix,
}

/// A matrix that maps interleaved frames of one channel layout to another.
#[derive(Debug, Clone, PartialEq)]
pub struct MixMatrix {
    inputs: usize,
    outputs: usize,
    /// `outputs` rows of `inputs` coefficients.
    coefficients: Box<[f64]>,
    /// For [`MixKind::Route`]: the input of each output.
    routes: Box<[Option<usize>]>,
    kind: MixKind,
}

impl MixMatrix {
    /// The matrix from layout `from` to layout `to`.
    pub fn new(from: ChannelLayout, to: ChannelLayout, options: MixOptions) -> Self {
        let inputs = usize::from(from.channel_count());
        let outputs = usize::from(to.channel_count());
        let mut coefficients = vec![0.0; inputs * outputs].into_boxed_slice();
        let from_known = with_positions(from);
        let to_known = with_positions(to);
        match (from_known, to_known) {
            (Some(from), Some(to)) => {
                for (i, position) in from.positions().enumerate() {
                    for (target, weight) in fold(position, to, options, 0) {
                        if let Some(o) = to.index_of(target) {
                            coefficients[o * inputs + i] += weight;
                        }
                    }
                }
            }
            _ => {
                for c in 0..inputs.min(outputs) {
                    coefficients[c * inputs + c] = 1.0;
                }
            }
        }
        if options.normalize {
            let largest = coefficients
                .chunks(inputs.max(1))
                .map(|row| row.iter().map(|c| c.abs()).sum::<f64>())
                .fold(0.0, f64::max);
            if largest > 1.0 {
                for c in coefficients.iter_mut() {
                    *c /= largest;
                }
            }
        }
        Self::from_coefficients(inputs, outputs, coefficients)
    }

    /// A matrix with the given coefficients: `outputs` rows of `inputs` values.
    /// `None` if the number of coefficients does not match.
    pub fn from_rows(inputs: usize, outputs: usize, coefficients: &[f64]) -> Option<Self> {
        (coefficients.len() == inputs * outputs && inputs > 0 && outputs > 0)
            .then(|| Self::from_coefficients(inputs, outputs, coefficients.into()))
    }

    fn from_coefficients(inputs: usize, outputs: usize, coefficients: Box<[f64]>) -> Self {
        let rows: Vec<&[f64]> = coefficients.chunks(inputs.max(1)).collect();
        // A row that is all zeros except one 1.0 copies a channel.
        let routes: Option<Vec<Option<usize>>> = rows
            .iter()
            .map(|row| {
                let mut source = None;
                for (i, &c) in row.iter().enumerate() {
                    if c == 1.0 && source.is_none() {
                        source = Some(i);
                    } else if c != 0.0 {
                        return None;
                    }
                }
                Some(source)
            })
            .collect();
        let (kind, routes) = match routes {
            Some(routes) => {
                let identity =
                    inputs == outputs && routes.iter().enumerate().all(|(o, r)| *r == Some(o));
                let kind = if identity {
                    MixKind::Identity
                } else {
                    MixKind::Route
                };
                (kind, routes.into_boxed_slice())
            }
            None => (MixKind::Matrix, Box::default()),
        };
        Self {
            inputs,
            outputs,
            coefficients,
            routes,
            kind,
        }
    }

    /// The number of input channels.
    pub fn inputs(&self) -> usize {
        self.inputs
    }

    /// The number of output channels.
    pub fn outputs(&self) -> usize {
        self.outputs
    }

    /// How the matrix maps channels.
    pub fn kind(&self) -> MixKind {
        self.kind
    }

    /// Whether the matrix leaves every channel where it is.
    pub fn is_identity(&self) -> bool {
        self.kind == MixKind::Identity
    }

    /// The weight of input channel `input` in output channel `output`.
    pub fn coefficient(&self, output: usize, input: usize) -> f64 {
        if output < self.outputs && input < self.inputs {
            self.coefficients[output * self.inputs + input]
        } else {
            0.0
        }
    }

    /// Mixes the whole frames of `input` into `output` and returns the number of
    /// frames mixed: as many as both slices hold.
    pub fn apply(&self, input: &[f32], output: &mut [f32]) -> usize {
        if self.inputs == 0 || self.outputs == 0 {
            return 0;
        }
        let frames = (input.len() / self.inputs).min(output.len() / self.outputs);
        let input = &input[..frames * self.inputs];
        let output = &mut output[..frames * self.outputs];
        match self.kind {
            MixKind::Identity => output.copy_from_slice(input),
            MixKind::Route => {
                for (src, dst) in input
                    .chunks_exact(self.inputs)
                    .zip(output.chunks_exact_mut(self.outputs))
                {
                    for (d, route) in dst.iter_mut().zip(self.routes.iter()) {
                        *d = route.map_or(0.0, |i| src[i]);
                    }
                }
            }
            MixKind::Matrix => {
                for (src, dst) in input
                    .chunks_exact(self.inputs)
                    .zip(output.chunks_exact_mut(self.outputs))
                {
                    for (d, row) in dst
                        .iter_mut()
                        .zip(self.coefficients.chunks_exact(self.inputs))
                    {
                        let sum: f64 = row.iter().zip(src).map(|(&c, &s)| c * f64::from(s)).sum();
                        *d = sum as f32;
                    }
                }
            }
        }
        frames
    }
}

impl fmt::Display for MixMatrix {
    /// One line per output channel, for diagnostics.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for row in self.coefficients.chunks(self.inputs.max(1)) {
            for (i, c) in row.iter().enumerate() {
                if i > 0 {
                    f.write_str(" ")?;
                }
                write!(f, "{c:.4}")?;
            }
            f.write_str("\n")?;
        }
        Ok(())
    }
}

/// The layout with positions that stands for `layout`, if there is one.
fn with_positions(layout: ChannelLayout) -> Option<ChannelLayout> {
    if layout.is_unspecified() {
        ChannelLayout::default_for_count(layout.channel_count()).filter(|l| !l.is_unspecified())
    } else {
        Some(layout)
    }
}

/// Where the input channel at `position` goes in layout `to`, with weights. Empty if
/// it is dropped.
fn fold(
    position: ChannelPosition,
    to: ChannelLayout,
    options: MixOptions,
    depth: u32,
) -> Vec<(ChannelPosition, f64)> {
    if to.contains(position) {
        return vec![(position, 1.0)];
    }
    // Two steps at most (back centre → back left → front left); more means the
    // output layout has nothing near this position.
    if depth > 3 {
        return Vec::new();
    }
    let has = |p: ChannelPosition| to.contains(p);
    let has_pair = |l: ChannelPosition, r: ChannelPosition| has(l) && has(r);
    let h = FRAC_1_SQRT_2;
    let next: Vec<(ChannelPosition, f64)> = match position {
        P::FrontLeft | P::FrontRight => vec![(P::FrontCenter, h)],
        P::FrontCenter => {
            if has_pair(P::FrontLeft, P::FrontRight) {
                vec![(P::FrontLeft, h), (P::FrontRight, h)]
            } else {
                Vec::new()
            }
        }
        P::LowFrequency => {
            if options.include_lfe {
                if has_pair(P::FrontLeft, P::FrontRight) {
                    vec![(P::FrontLeft, h), (P::FrontRight, h)]
                } else {
                    vec![(P::FrontCenter, 1.0)]
                }
            } else {
                Vec::new()
            }
        }
        P::BackLeft => side_or_front(has(P::SideLeft), P::SideLeft, P::FrontLeft),
        P::BackRight => side_or_front(has(P::SideRight), P::SideRight, P::FrontRight),
        P::SideLeft => side_or_front(has(P::BackLeft), P::BackLeft, P::FrontLeft),
        P::SideRight => side_or_front(has(P::BackRight), P::BackRight, P::FrontRight),
        P::BackCenter => {
            if has_pair(P::BackLeft, P::BackRight) {
                vec![(P::BackLeft, h), (P::BackRight, h)]
            } else if has_pair(P::SideLeft, P::SideRight) {
                vec![(P::SideLeft, h), (P::SideRight, h)]
            } else {
                vec![(P::FrontLeft, 0.5), (P::FrontRight, 0.5)]
            }
        }
        P::FrontLeftOfCenter => vec![(P::FrontLeft, 1.0)],
        P::FrontRightOfCenter => vec![(P::FrontRight, 1.0)],
        P::TopCenter | P::TopFrontCenter => vec![(P::FrontCenter, h)],
        P::TopFrontLeft => vec![(P::FrontLeft, h)],
        P::TopFrontRight => vec![(P::FrontRight, h)],
        P::TopBackLeft => vec![(P::BackLeft, h)],
        P::TopBackRight => vec![(P::BackRight, h)],
        P::TopBackCenter => vec![(P::BackCenter, h)],
    };
    next.into_iter()
        .flat_map(|(target, weight)| {
            fold(target, to, options, depth + 1)
                .into_iter()
                .map(move |(p, w)| (p, w * weight))
        })
        .collect()
}

/// A surround channel goes to its partner at full level when the output has it, or
/// to the front at −3 dB.
fn side_or_front(
    has_partner: bool,
    partner: ChannelPosition,
    front: ChannelPosition,
) -> Vec<(ChannelPosition, f64)> {
    if has_partner {
        vec![(partner, 1.0)]
    } else {
        vec![(front, FRAC_1_SQRT_2)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: f64 = FRAC_1_SQRT_2;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn raw() -> MixOptions {
        MixOptions {
            normalize: false,
            ..MixOptions::default()
        }
    }

    #[test]
    fn same_layout_is_identity() {
        let m = MixMatrix::new(
            ChannelLayout::SURROUND_5_1,
            ChannelLayout::SURROUND_5_1,
            MixOptions::default(),
        );
        assert!(m.is_identity());
        let unspecified = ChannelLayout::unspecified(2).unwrap();
        let m = MixMatrix::new(unspecified, ChannelLayout::STEREO, MixOptions::default());
        assert!(m.is_identity());
    }

    #[test]
    fn surround_5_1_to_stereo_uses_itu_coefficients() {
        // FL FR FC LFE BL BR
        let m = MixMatrix::new(ChannelLayout::SURROUND_5_1, ChannelLayout::STEREO, raw());
        let left: Vec<f64> = (0..6).map(|i| m.coefficient(0, i)).collect();
        let right: Vec<f64> = (0..6).map(|i| m.coefficient(1, i)).collect();
        for (got, want) in left.iter().zip([1.0, 0.0, H, 0.0, H, 0.0]) {
            assert!(close(*got, want), "{left:?}");
        }
        for (got, want) in right.iter().zip([0.0, 1.0, H, 0.0, 0.0, H]) {
            assert!(close(*got, want), "{right:?}");
        }
        assert_eq!(m.kind(), MixKind::Matrix);

        // Normalized: each row sums to 1.0, so full-scale input cannot clip.
        let m = MixMatrix::new(
            ChannelLayout::SURROUND_5_1,
            ChannelLayout::STEREO,
            MixOptions::default(),
        );
        let scale = 1.0 / (1.0 + 2.0 * H);
        assert!(close(m.coefficient(0, 0), scale));
        assert!(close(m.coefficient(0, 2), H * scale));
        let mut out = [0.0f32; 2];
        m.apply(&[1.0, -1.0, 1.0, 1.0, 1.0, -1.0], &mut out);
        assert!((out[0] - 1.0).abs() < 1e-6 && out[0] <= 1.0);
        assert!((f64::from(out[1]) + scale).abs() < 1e-6);
        m.apply(&[0.0, -1.0, 1.0, 1.0, 1.0, 1.0], &mut out);
        assert!((f64::from(out[1]) - (2.0 * H - 1.0) * scale).abs() < 1e-6);
    }

    #[test]
    fn lfe_is_dropped_unless_included() {
        let m = MixMatrix::new(ChannelLayout::SURROUND_5_1, ChannelLayout::STEREO, raw());
        assert_eq!(m.coefficient(0, 3), 0.0);
        let options = MixOptions {
            include_lfe: true,
            normalize: false,
        };
        let m = MixMatrix::new(ChannelLayout::SURROUND_5_1, ChannelLayout::STEREO, options);
        assert!(close(m.coefficient(0, 3), H));
    }

    #[test]
    fn side_and_back_channels_stand_in_for_each_other() {
        // 7.1 (FL FR FC LFE BL BR SL SR) to 5.1 with back channels: sides fold to back.
        let m = MixMatrix::new(
            ChannelLayout::SURROUND_7_1,
            ChannelLayout::SURROUND_5_1,
            raw(),
        );
        assert!(close(m.coefficient(4, 4), 1.0));
        assert!(close(m.coefficient(4, 6), 1.0));
        assert!(close(m.coefficient(5, 7), 1.0));
        // 5.1 to 5.1(side): the back channels play on the sides, in the same slots.
        let m = MixMatrix::new(
            ChannelLayout::SURROUND_5_1,
            ChannelLayout::SURROUND_5_1_SIDE,
            MixOptions::default(),
        );
        assert_eq!(m.kind(), MixKind::Identity);
        let mut out = [0.0f32; 6];
        m.apply(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &mut out);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn mono_and_stereo() {
        let up = MixMatrix::new(
            ChannelLayout::MONO,
            ChannelLayout::STEREO,
            MixOptions::default(),
        );
        assert!(close(up.coefficient(0, 0), H) && close(up.coefficient(1, 0), H));
        let down = MixMatrix::new(ChannelLayout::STEREO, ChannelLayout::MONO, raw());
        assert!(close(down.coefficient(0, 0), H) && close(down.coefficient(0, 1), H));
        // Surround to mono reaches the centre through the fronts.
        let m = MixMatrix::new(ChannelLayout::SURROUND_5_1, ChannelLayout::MONO, raw());
        assert!(close(m.coefficient(0, 2), 1.0));
        assert!(close(m.coefficient(0, 4), 0.5));
    }

    #[test]
    fn upmix_places_channels_and_leaves_the_rest_silent() {
        let m = MixMatrix::new(
            ChannelLayout::STEREO,
            ChannelLayout::SURROUND_5_1,
            MixOptions::default(),
        );
        assert_eq!(m.kind(), MixKind::Route);
        let mut out = [9.0f32; 12];
        assert_eq!(m.apply(&[0.5, -0.5, 0.25, -0.25], &mut out), 2);
        assert_eq!(out[..6], [0.5, -0.5, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(out[6..], [0.25, -0.25, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn unknown_layouts_match_by_index() {
        let from = ChannelLayout::unspecified(10).unwrap();
        let m = MixMatrix::new(from, ChannelLayout::STEREO, MixOptions::default());
        assert_eq!(m.kind(), MixKind::Route);
        assert_eq!(m.coefficient(1, 1), 1.0);
        assert_eq!(m.coefficient(1, 5), 0.0);
    }

    #[test]
    fn custom_rows() {
        let swap = MixMatrix::from_rows(2, 2, &[0.0, 1.0, 1.0, 0.0]).unwrap();
        assert_eq!(swap.kind(), MixKind::Route);
        let mut out = [0.0f32; 2];
        swap.apply(&[0.1, 0.2], &mut out);
        assert_eq!(out, [0.2, 0.1]);
        assert!(MixMatrix::from_rows(2, 2, &[1.0]).is_none());
    }
}
