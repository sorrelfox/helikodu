// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The volume control's position and the gain it stands for.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

/// The position of a volume control, from 0.0 (silent) to 1.0 (full).
///
/// The position maps to an amplitude on the cubic curve `a = p³` that desktop mixers
/// use, so the player's slider, the desktop's mixer and MPRIS all agree. Muting is
/// separate from the volume.
///
/// Serialized as the plain position.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Volume(f32);

impl Volume {
    /// Silence.
    pub const SILENT: Self = Self(0.0);
    /// Full volume: samples are not scaled.
    pub const FULL: Self = Self(1.0);

    /// The volume at `position`, or `None` if it is not a number from 0.0 to 1.0.
    pub fn new(position: f32) -> Option<Self> {
        (0.0..=1.0).contains(&position).then_some(Self(position))
    }

    /// The volume at `position`, moved into the range from 0.0 to 1.0. Not-a-number
    /// becomes silence.
    pub fn clamped(position: f32) -> Self {
        if position.is_nan() {
            Self::SILENT
        } else {
            Self(position.clamp(0.0, 1.0))
        }
    }

    /// The volume whose amplitude is `amplitude`, moved into the range from 0.0 to 1.0.
    pub fn from_amplitude(amplitude: f32) -> Self {
        Self::clamped(amplitude.cbrt())
    }

    /// The position of the control.
    pub const fn position(self) -> f32 {
        self.0
    }

    /// The factor samples are multiplied by: the position cubed.
    pub fn amplitude(self) -> f32 {
        self.0 * self.0 * self.0
    }
}

impl<'de> Deserialize<'de> for Volume {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let position = f32::deserialize(deserializer)?;
        Self::new(position)
            .ok_or_else(|| D::Error::custom(format_args!("invalid volume: {position}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cubic_curve() {
        assert_eq!(Volume::FULL.amplitude(), 1.0);
        assert_eq!(Volume::SILENT.amplitude(), 0.0);
        assert_eq!(Volume::new(0.5).unwrap().amplitude(), 0.125);
        let back = Volume::from_amplitude(0.125).position();
        assert!((back - 0.5).abs() < 1e-6);
    }

    #[test]
    fn range_is_enforced() {
        assert_eq!(Volume::new(1.5), None);
        assert_eq!(Volume::new(-0.1), None);
        assert_eq!(Volume::new(f32::NAN), None);
        assert_eq!(Volume::clamped(1.5), Volume::FULL);
        assert_eq!(Volume::clamped(f32::NAN), Volume::SILENT);
        assert!(serde_json::from_str::<Volume>("2.0").is_err());
        assert_eq!(
            serde_json::from_str::<Volume>("0.25").unwrap().position(),
            0.25
        );
    }
}
