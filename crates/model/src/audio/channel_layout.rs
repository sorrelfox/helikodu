// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Channel layouts in the canonical WAVE order.

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::ChannelPosition;

/// Every bit of a WAVE channel mask that names a position.
const VALID_MASK: u32 = (1 << ChannelPosition::ALL.len()) - 1;

/// The channels of a stream and their order.
///
/// A layout is either a set of [`ChannelPosition`]s, always interleaved in canonical
/// order, or only a channel count, for streams that do not say which speaker each
/// channel is for.
///
/// Serialized as `{ "channels": <count>, "mask": <WAVE channel mask> }`, with a mask of 0
/// for a layout without positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChannelLayout {
    /// The WAVE channel mask, or 0 when the positions are not known.
    mask: u32,
    /// The number of channels; equals the number of mask bits when the mask is set.
    count: u16,
}

/// The common layouts with their names.
const NAMED: [(ChannelLayout, &str); 10] = [
    (ChannelLayout::MONO, "mono"),
    (ChannelLayout::STEREO, "stereo"),
    (ChannelLayout::SURROUND_2_1, "2.1"),
    (ChannelLayout::SURROUND_3_0, "3.0"),
    (ChannelLayout::QUAD, "quad"),
    (ChannelLayout::SURROUND_5_0, "5.0"),
    (ChannelLayout::SURROUND_5_1, "5.1"),
    (ChannelLayout::SURROUND_5_1_SIDE, "5.1(side)"),
    (ChannelLayout::SURROUND_6_1, "6.1"),
    (ChannelLayout::SURROUND_7_1, "7.1"),
];

impl ChannelLayout {
    /// One channel: front centre.
    pub const MONO: Self = Self::of(&[ChannelPosition::FrontCenter]);
    /// FL, FR.
    pub const STEREO: Self = Self::of(&[ChannelPosition::FrontLeft, ChannelPosition::FrontRight]);
    /// FL, FR, LFE.
    pub const SURROUND_2_1: Self = Self::of(&[
        ChannelPosition::FrontLeft,
        ChannelPosition::FrontRight,
        ChannelPosition::LowFrequency,
    ]);
    /// FL, FR, FC.
    pub const SURROUND_3_0: Self = Self::of(&[
        ChannelPosition::FrontLeft,
        ChannelPosition::FrontRight,
        ChannelPosition::FrontCenter,
    ]);
    /// FL, FR, BL, BR.
    pub const QUAD: Self = Self::of(&[
        ChannelPosition::FrontLeft,
        ChannelPosition::FrontRight,
        ChannelPosition::BackLeft,
        ChannelPosition::BackRight,
    ]);
    /// FL, FR, FC, BL, BR.
    pub const SURROUND_5_0: Self = Self::of(&[
        ChannelPosition::FrontLeft,
        ChannelPosition::FrontRight,
        ChannelPosition::FrontCenter,
        ChannelPosition::BackLeft,
        ChannelPosition::BackRight,
    ]);
    /// FL, FR, FC, LFE, BL, BR.
    pub const SURROUND_5_1: Self = Self::of(&[
        ChannelPosition::FrontLeft,
        ChannelPosition::FrontRight,
        ChannelPosition::FrontCenter,
        ChannelPosition::LowFrequency,
        ChannelPosition::BackLeft,
        ChannelPosition::BackRight,
    ]);
    /// FL, FR, FC, LFE, SL, SR.
    pub const SURROUND_5_1_SIDE: Self = Self::of(&[
        ChannelPosition::FrontLeft,
        ChannelPosition::FrontRight,
        ChannelPosition::FrontCenter,
        ChannelPosition::LowFrequency,
        ChannelPosition::SideLeft,
        ChannelPosition::SideRight,
    ]);
    /// FL, FR, FC, LFE, BC, SL, SR.
    pub const SURROUND_6_1: Self = Self::of(&[
        ChannelPosition::FrontLeft,
        ChannelPosition::FrontRight,
        ChannelPosition::FrontCenter,
        ChannelPosition::LowFrequency,
        ChannelPosition::BackCenter,
        ChannelPosition::SideLeft,
        ChannelPosition::SideRight,
    ]);
    /// FL, FR, FC, LFE, BL, BR, SL, SR.
    pub const SURROUND_7_1: Self = Self::of(&[
        ChannelPosition::FrontLeft,
        ChannelPosition::FrontRight,
        ChannelPosition::FrontCenter,
        ChannelPosition::LowFrequency,
        ChannelPosition::BackLeft,
        ChannelPosition::BackRight,
        ChannelPosition::SideLeft,
        ChannelPosition::SideRight,
    ]);

    /// Builds a constant layout from distinct positions.
    const fn of(positions: &[ChannelPosition]) -> Self {
        let mut mask = 0;
        let mut i = 0;
        while i < positions.len() {
            mask |= positions[i].mask_bit();
            i += 1;
        }
        Self::from_valid_mask(mask)
    }

    const fn from_valid_mask(mask: u32) -> Self {
        // At most 18 bits are set, so the count fits.
        #[allow(clippy::cast_possible_truncation)]
        let count = mask.count_ones() as u16;
        Self { mask, count }
    }

    /// The layout with the positions of a WAVE channel mask, or `None` if the mask is
    /// zero or has bits that name no position.
    pub const fn from_mask(mask: u32) -> Option<Self> {
        if mask == 0 || mask & !VALID_MASK != 0 {
            None
        } else {
            Some(Self::from_valid_mask(mask))
        }
    }

    /// The layout with the given positions, in any order. `None` if there are none, or
    /// if a position appears twice.
    pub fn from_positions(positions: impl IntoIterator<Item = ChannelPosition>) -> Option<Self> {
        let mut mask = 0;
        for position in positions {
            if mask & position.mask_bit() != 0 {
                return None;
            }
            mask |= position.mask_bit();
        }
        Self::from_mask(mask)
    }

    /// A layout of `count` channels whose positions are not known, or `None` if `count`
    /// is zero.
    pub const fn unspecified(count: u16) -> Option<Self> {
        if count == 0 {
            None
        } else {
            Some(Self { mask: 0, count })
        }
    }

    /// The usual layout for a stream of `count` channels that does not name its
    /// positions: the defaults of FLAC and WAVE for one to eight channels (mono, stereo,
    /// 3.0, quad, 5.0, 5.1, 6.1, 7.1), and a layout without positions above that.
    /// `None` if `count` is zero.
    pub const fn default_for_count(count: u16) -> Option<Self> {
        match count {
            1 => Some(Self::MONO),
            2 => Some(Self::STEREO),
            3 => Some(Self::SURROUND_3_0),
            4 => Some(Self::QUAD),
            5 => Some(Self::SURROUND_5_0),
            6 => Some(Self::SURROUND_5_1),
            7 => Some(Self::SURROUND_6_1),
            8 => Some(Self::SURROUND_7_1),
            _ => Self::unspecified(count),
        }
    }

    /// The number of channels.
    pub const fn channel_count(self) -> u16 {
        self.count
    }

    /// The WAVE channel mask, or `None` when the positions are not known.
    pub const fn mask(self) -> Option<u32> {
        if self.mask == 0 {
            None
        } else {
            Some(self.mask)
        }
    }

    /// Whether the positions of the channels are unknown.
    pub const fn is_unspecified(self) -> bool {
        self.mask == 0
    }

    /// Whether the layout has a channel at `position`.
    pub const fn contains(self, position: ChannelPosition) -> bool {
        self.mask & position.mask_bit() != 0
    }

    /// The positions in interleaving order. Empty when the positions are not known.
    pub fn positions(self) -> impl Iterator<Item = ChannelPosition> {
        ChannelPosition::ALL
            .into_iter()
            .filter(move |position| self.contains(*position))
    }

    /// The index of the channel at `position` within an interleaved frame, or `None`
    /// if the layout has no such channel.
    pub const fn index_of(self, position: ChannelPosition) -> Option<usize> {
        if self.contains(position) {
            let below = self.mask & (position.mask_bit() - 1);
            Some(below.count_ones() as usize)
        } else {
            None
        }
    }

    /// The common name of the layout, such as `stereo` or `5.1`, if it has one.
    pub fn name(self) -> Option<&'static str> {
        NAMED
            .iter()
            .find(|(layout, _)| *layout == self)
            .map(|(_, name)| *name)
    }
}

impl fmt::Display for ChannelLayout {
    /// The common name, the positions joined with `+`, or the number of channels.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = self.name() {
            return f.write_str(name);
        }
        if self.is_unspecified() {
            return write!(f, "{} channels", self.count);
        }
        for (i, position) in self.positions().enumerate() {
            if i > 0 {
                f.write_str("+")?;
            }
            f.write_str(position.abbreviation())?;
        }
        Ok(())
    }
}

/// The serialized form of a [`ChannelLayout`].
#[derive(Serialize, Deserialize)]
struct LayoutFields {
    channels: u16,
    mask: u32,
}

impl Serialize for ChannelLayout {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        LayoutFields {
            channels: self.count,
            mask: self.mask,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ChannelLayout {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = LayoutFields::deserialize(deserializer)?;
        let layout = if fields.mask == 0 {
            Self::unspecified(fields.channels)
        } else {
            Self::from_mask(fields.mask).filter(|layout| layout.count == fields.channels)
        };
        layout.ok_or_else(|| {
            D::Error::custom(format_args!(
                "invalid channel layout: {} channels, mask {:#x}",
                fields.channels, fields.mask
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ChannelPosition::*;

    #[test]
    fn positions_follow_wave_mask_bits() {
        assert_eq!(FrontLeft.mask_bit(), 0x1);
        assert_eq!(LowFrequency.mask_bit(), 0x8);
        assert_eq!(BackLeft.mask_bit(), 0x10);
        assert_eq!(SideLeft.mask_bit(), 0x200);
        assert_eq!(TopBackRight.mask_bit(), 0x20000);
    }

    #[test]
    fn common_layouts_have_the_wave_masks() {
        assert_eq!(ChannelLayout::MONO.mask(), Some(0x4));
        assert_eq!(ChannelLayout::STEREO.mask(), Some(0x3));
        assert_eq!(ChannelLayout::QUAD.mask(), Some(0x33));
        assert_eq!(ChannelLayout::SURROUND_5_1.mask(), Some(0x3f));
        assert_eq!(ChannelLayout::SURROUND_5_1_SIDE.mask(), Some(0x60f));
        assert_eq!(ChannelLayout::SURROUND_7_1.mask(), Some(0x63f));
    }

    #[test]
    fn positions_come_out_in_canonical_order() {
        let layout = ChannelLayout::from_positions([
            SideRight,
            LowFrequency,
            FrontRight,
            SideLeft,
            FrontLeft,
            FrontCenter,
        ])
        .unwrap();
        assert_eq!(layout, ChannelLayout::SURROUND_5_1_SIDE);
        let order: Vec<_> = layout.positions().collect();
        assert_eq!(
            order,
            [
                FrontLeft,
                FrontRight,
                FrontCenter,
                LowFrequency,
                SideLeft,
                SideRight
            ]
        );
        assert_eq!(layout.index_of(LowFrequency), Some(3));
        assert_eq!(layout.index_of(SideLeft), Some(4));
        assert_eq!(layout.index_of(BackLeft), None);
    }

    #[test]
    fn counts() {
        for count in 1..=8 {
            let layout = ChannelLayout::default_for_count(count).unwrap();
            assert_eq!(layout.channel_count(), count);
            assert!(!layout.is_unspecified());
            assert_eq!(layout.positions().count(), usize::from(count));
        }
        let wide = ChannelLayout::default_for_count(12).unwrap();
        assert!(wide.is_unspecified());
        assert_eq!(wide.channel_count(), 12);
        assert_eq!(wide.positions().count(), 0);
        assert_eq!(ChannelLayout::default_for_count(0), None);
    }

    #[test]
    fn invalid_layouts_are_rejected() {
        assert_eq!(ChannelLayout::from_mask(0), None);
        assert_eq!(ChannelLayout::from_mask(1 << 18), None);
        assert_eq!(ChannelLayout::from_positions([FrontLeft, FrontLeft]), None);
        assert_eq!(ChannelLayout::from_positions([]), None);
        assert_eq!(ChannelLayout::unspecified(0), None);
    }

    #[test]
    fn display() {
        assert_eq!(ChannelLayout::SURROUND_5_1.to_string(), "5.1");
        let odd = ChannelLayout::from_positions([FrontLeft, FrontRight, BackCenter]).unwrap();
        assert_eq!(odd.to_string(), "FL+FR+BC");
        assert_eq!(
            ChannelLayout::unspecified(3).unwrap().to_string(),
            "3 channels"
        );
    }

    #[test]
    fn serde_round_trip() {
        for layout in [
            ChannelLayout::SURROUND_7_1,
            ChannelLayout::unspecified(10).unwrap(),
        ] {
            let json = serde_json::to_string(&layout).unwrap();
            assert_eq!(
                serde_json::from_str::<ChannelLayout>(&json).unwrap(),
                layout
            );
        }
        assert_eq!(
            serde_json::to_string(&ChannelLayout::STEREO).unwrap(),
            r#"{"channels":2,"mask":3}"#
        );
        assert!(serde_json::from_str::<ChannelLayout>(r#"{"channels":3,"mask":3}"#).is_err());
        assert!(serde_json::from_str::<ChannelLayout>(r#"{"channels":0,"mask":0}"#).is_err());
    }
}
