// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The order of the DSP chain and the saved chains.

use serde::{Deserialize, Serialize};

use crate::settings::SettingsError;

/// A stage of the DSP chain whose place can change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChainStage {
    /// Pitch shift.
    Pitch,
    /// The equalizer, with its preamp.
    Equalizer,
    /// Convolution with an impulse response.
    Convolution,
    /// Crossfeed.
    Crossfeed,
    /// Mono, left/right swap and balance.
    ChannelMix,
    /// The dynamics compressor.
    Compressor,
    /// The true-peak limiter.
    Limiter,
}

impl ChainStage {
    /// Every stage, in the default order.
    pub const DEFAULT_ORDER: [Self; 7] = [
        Self::Pitch,
        Self::Equalizer,
        Self::Convolution,
        Self::Crossfeed,
        Self::ChannelMix,
        Self::Compressor,
        Self::Limiter,
    ];

    /// The name in `settings.toml`.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Pitch => "pitch",
            Self::Equalizer => "equalizer",
            Self::Convolution => "convolution",
            Self::Crossfeed => "crossfeed",
            Self::ChannelMix => "channel-mix",
            Self::Compressor => "compressor",
            Self::Limiter => "limiter",
        }
    }

    /// The stage named `key` in `settings.toml`.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::DEFAULT_ORDER.into_iter().find(|s| s.key() == key)
    }
}

/// A saved chain: a `[[dsp.chains]]` entry. Choosing it puts its stages in its
/// order, switches them on and switches every other stage off; the stages keep
/// their own values.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedChain {
    /// The name the user gave it.
    pub name: String,
    /// The stages that run, in order.
    pub stages: Vec<ChainStage>,
}

impl SavedChain {
    /// Checks the name and that no stage is listed twice.
    pub fn validate(&self, key: &str) -> Result<(), SettingsError> {
        if self.name.trim().is_empty() {
            return Err(SettingsError {
                key: format!("{key}.name"),
                problem: "is empty".into(),
            });
        }
        check_unique(&format!("{key}.stages"), &self.stages)
    }
}

/// Checks that no stage is listed twice.
pub(super) fn check_unique(key: &str, stages: &[ChainStage]) -> Result<(), SettingsError> {
    for (i, stage) in stages.iter().enumerate() {
        if stages[..i].contains(stage) {
            return Err(SettingsError {
                key: key.into(),
                problem: format!("lists \"{}\" twice", stage.key()),
            });
        }
    }
    Ok(())
}

/// `order` with the stages it leaves out added at the end, in the default order.
pub(super) fn complete(order: &[ChainStage]) -> Vec<ChainStage> {
    let mut full: Vec<ChainStage> = Vec::with_capacity(ChainStage::DEFAULT_ORDER.len());
    for stage in order.iter().chain(&ChainStage::DEFAULT_ORDER) {
        if !full.contains(stage) {
            full.push(*stage);
        }
    }
    full
}
