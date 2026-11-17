// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Speaker positions in the canonical WAVE order.

use std::fmt;

/// A speaker position, as defined for WAVE files (`WAVE_FORMAT_EXTENSIBLE`).
///
/// The variants are in canonical order: interleaved channels always appear in this
/// order, which is also FFmpeg's native order. Backends whose devices use another
/// order (ALSA's default 5.1 order, for example) remap at the edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChannelPosition {
    /// Front left (FL).
    FrontLeft,
    /// Front right (FR).
    FrontRight,
    /// Front centre (FC).
    FrontCenter,
    /// Low-frequency effects (LFE).
    LowFrequency,
    /// Back left (BL), also called rear or surround left.
    BackLeft,
    /// Back right (BR), also called rear or surround right.
    BackRight,
    /// Front left of centre (FLC).
    FrontLeftOfCenter,
    /// Front right of centre (FRC).
    FrontRightOfCenter,
    /// Back centre (BC).
    BackCenter,
    /// Side left (SL).
    SideLeft,
    /// Side right (SR).
    SideRight,
    /// Top centre (TC).
    TopCenter,
    /// Top front left (TFL).
    TopFrontLeft,
    /// Top front centre (TFC).
    TopFrontCenter,
    /// Top front right (TFR).
    TopFrontRight,
    /// Top back left (TBL).
    TopBackLeft,
    /// Top back centre (TBC).
    TopBackCenter,
    /// Top back right (TBR).
    TopBackRight,
}

impl ChannelPosition {
    /// Every position, in canonical order.
    pub const ALL: [Self; 18] = [
        Self::FrontLeft,
        Self::FrontRight,
        Self::FrontCenter,
        Self::LowFrequency,
        Self::BackLeft,
        Self::BackRight,
        Self::FrontLeftOfCenter,
        Self::FrontRightOfCenter,
        Self::BackCenter,
        Self::SideLeft,
        Self::SideRight,
        Self::TopCenter,
        Self::TopFrontLeft,
        Self::TopFrontCenter,
        Self::TopFrontRight,
        Self::TopBackLeft,
        Self::TopBackCenter,
        Self::TopBackRight,
    ];

    /// The position's bit in a WAVE channel mask (`dwChannelMask`).
    pub const fn mask_bit(self) -> u32 {
        1 << self as u32
    }

    /// The usual abbreviation, for example `FL` or `LFE`.
    pub const fn abbreviation(self) -> &'static str {
        match self {
            Self::FrontLeft => "FL",
            Self::FrontRight => "FR",
            Self::FrontCenter => "FC",
            Self::LowFrequency => "LFE",
            Self::BackLeft => "BL",
            Self::BackRight => "BR",
            Self::FrontLeftOfCenter => "FLC",
            Self::FrontRightOfCenter => "FRC",
            Self::BackCenter => "BC",
            Self::SideLeft => "SL",
            Self::SideRight => "SR",
            Self::TopCenter => "TC",
            Self::TopFrontLeft => "TFL",
            Self::TopFrontCenter => "TFC",
            Self::TopFrontRight => "TFR",
            Self::TopBackLeft => "TBL",
            Self::TopBackCenter => "TBC",
            Self::TopBackRight => "TBR",
        }
    }
}

impl fmt::Display for ChannelPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.abbreviation())
    }
}
