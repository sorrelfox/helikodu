// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the DSP chain does, as plain data for the signal path.

use std::fmt;

use helikodu_model::settings::{ChannelMode, EqMode};

/// The equalizer of a plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqualizerPlan {
    /// Graphic or parametric.
    pub mode: EqMode,
    /// The filters that change the sound.
    pub filters: usize,
    /// The preamp, in dB.
    pub preamp_db: f64,
}

impl fmt::Display for EqualizerPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.mode {
            EqMode::Graphic10 => f.write_str("EQ 10-band")?,
            EqMode::Graphic31 => f.write_str("EQ 31-band")?,
            EqMode::Parametric => {
                let s = if self.filters == 1 { "" } else { "s" };
                write!(f, "EQ {} filter{s}", self.filters)?;
            }
        }
        if self.preamp_db != 0.0 {
            write!(f, " · preamp {:.1} dB", self.preamp_db)?;
        }
        Ok(())
    }
}

/// The crossfeed of a plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrossfeedPlan {
    /// The cut-off frequency, in Hz.
    pub cutoff_hz: u32,
    /// The low-frequency separation, in dB.
    pub level_db: f64,
}

impl fmt::Display for CrossfeedPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "crossfeed {} Hz · {:.1} dB",
            self.cutoff_hz, self.level_db
        )
    }
}

/// The channel mixer of a plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixerPlan {
    /// Stereo, mono or swapped.
    pub channels: ChannelMode,
    /// The balance, in dB; negative is to the left.
    pub balance_db: f64,
}

impl fmt::Display for MixerPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mode = match self.channels {
            ChannelMode::Stereo => None,
            ChannelMode::Mono => Some("mono"),
            ChannelMode::Swap => Some("swap left and right"),
        };
        if let Some(mode) = mode {
            f.write_str(mode)?;
        }
        if self.balance_db != 0.0 {
            if mode.is_some() {
                f.write_str(" · ")?;
            }
            let side = if self.balance_db < 0.0 {
                "left"
            } else {
                "right"
            };
            write!(f, "balance {:.1} dB {side}", self.balance_db.abs())?;
        }
        Ok(())
    }
}

/// The limiter of a plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimiterPlan {
    /// The lookahead, in milliseconds.
    pub lookahead_ms: f64,
    /// The ceiling, in dBTP.
    pub ceiling_db: f64,
}

impl fmt::Display for LimiterPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "limiter {:.1} dBTP · lookahead {} ms",
            self.ceiling_db, self.lookahead_ms
        )
    }
}

/// The active stages of the DSP chain, in their fixed order. Empty when the chain
/// leaves the samples unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DspPlan {
    /// The equalizer, with its preamp.
    pub equalizer: Option<EqualizerPlan>,
    /// Crossfeed.
    pub crossfeed: Option<CrossfeedPlan>,
    /// Mono, swap and balance.
    pub mixer: Option<MixerPlan>,
    /// The limiter.
    pub limiter: Option<LimiterPlan>,
}

impl DspPlan {
    /// Whether no stage is active.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl fmt::Display for DspPlan {
    /// The active stages joined by arrows, as in the signal path.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        let mut item = |f: &mut fmt::Formatter<'_>, stage: &dyn fmt::Display| {
            let arrow = if first { "" } else { " → " };
            first = false;
            write!(f, "{arrow}{stage}")
        };
        if let Some(stage) = &self.equalizer {
            item(f, stage)?;
        }
        if let Some(stage) = &self.crossfeed {
            item(f, stage)?;
        }
        if let Some(stage) = &self.mixer {
            item(f, stage)?;
        }
        if let Some(stage) = &self.limiter {
            item(f, stage)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signal_path_text() {
        let plan = DspPlan {
            equalizer: Some(EqualizerPlan {
                mode: EqMode::Parametric,
                filters: 5,
                preamp_db: -4.5,
            }),
            crossfeed: Some(CrossfeedPlan {
                cutoff_hz: 700,
                level_db: 4.5,
            }),
            mixer: Some(MixerPlan {
                channels: ChannelMode::Mono,
                balance_db: -2.0,
            }),
            limiter: Some(LimiterPlan {
                lookahead_ms: 2.0,
                ceiling_db: -1.0,
            }),
        };
        assert_eq!(
            plan.to_string(),
            "EQ 5 filters · preamp -4.5 dB → crossfeed 700 Hz · 4.5 dB → \
             mono · balance 2.0 dB left → limiter -1.0 dBTP · lookahead 2 ms"
        );
        assert!(DspPlan::default().is_empty());
        assert_eq!(DspPlan::default().to_string(), "");
    }
}
