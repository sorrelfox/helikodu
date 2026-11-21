// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the DSP chain does, as plain data for the signal path.

use std::fmt;

use helikodu_model::settings::{ChainStage, ChannelMode, EqMode};

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

/// The compressor of a plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompressorPlan {
    /// Whether it is the night mode preset.
    pub night: bool,
    /// The threshold, in dBFS.
    pub threshold_db: f64,
    /// The ratio.
    pub ratio: f64,
    /// The makeup gain, in dB.
    pub makeup_db: f64,
    /// The attack, in milliseconds.
    pub attack_ms: f64,
    /// The release, in milliseconds.
    pub release_ms: f64,
    /// The knee's width, in dB.
    pub knee_db: f64,
    /// The lookahead, in milliseconds.
    pub lookahead_ms: f64,
}

impl fmt::Display for CompressorPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.night {
            f.write_str("night mode · ")?;
        }
        write!(
            f,
            "compressor {:.1} dB · {}:1",
            self.threshold_db,
            (self.ratio * 10.0).round() / 10.0
        )?;
        if self.makeup_db != 0.0 {
            write!(f, " · makeup {:+.1} dB", self.makeup_db)?;
        }
        Ok(())
    }
}

/// The convolution of a plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConvolutionPlan {
    /// The length of the impulse response, in frames at the device rate.
    pub taps: usize,
    /// The channels of the impulse response.
    pub ir_channels: u8,
    /// The rate of the impulse response's file, in Hz.
    pub file_rate: u32,
    /// Whether the impulse response was resampled to the device rate.
    pub resampled: bool,
    /// The delay the stage adds, in frames.
    pub latency_frames: usize,
    /// The delay the stage adds, in milliseconds.
    pub latency_ms: f64,
    /// The share of the processed signal, in percent.
    pub wet_percent: f64,
    /// The gain of the processed signal, in dB.
    pub gain_db: f64,
    /// Whether the impulse response is normalized to boost no frequency.
    pub normalized: bool,
}

impl fmt::Display for ConvolutionPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let channels = if self.ir_channels == 1 {
            "mono"
        } else {
            "stereo"
        };
        write!(f, "convolution {channels} · {} taps", self.taps)?;
        if self.resampled {
            write!(f, " · resampled from {} Hz", self.file_rate)?;
        }
        if self.wet_percent < 100.0 {
            write!(f, " · {:.0} % wet", self.wet_percent)?;
        }
        write!(f, " · latency {:.1} ms", self.latency_ms)
    }
}

/// The pitch shift of a plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchPlan {
    /// The shift, in semitones.
    pub semitones: f64,
    /// The delay the stage adds, in milliseconds.
    pub latency_ms: f64,
}

impl fmt::Display for PitchPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pitch {:+.2} semitones", self.semitones)
    }
}

/// The active stages of the DSP chain, in the chain's order. Empty when the chain
/// leaves the samples unchanged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DspPlan {
    /// The equalizer, with its preamp.
    pub equalizer: Option<EqualizerPlan>,
    /// Crossfeed.
    pub crossfeed: Option<CrossfeedPlan>,
    /// Mono, swap and balance.
    pub mixer: Option<MixerPlan>,
    /// The limiter.
    pub limiter: Option<LimiterPlan>,
    /// The compressor.
    pub compressor: Option<CompressorPlan>,
    /// Convolution.
    pub convolution: Option<ConvolutionPlan>,
    /// Pitch shift.
    pub pitch: Option<PitchPlan>,
    /// The order of all stages, active or not.
    pub order: [ChainStage; 7],
}

impl Default for DspPlan {
    fn default() -> Self {
        Self {
            equalizer: None,
            crossfeed: None,
            mixer: None,
            limiter: None,
            compressor: None,
            convolution: None,
            pitch: None,
            order: ChainStage::DEFAULT_ORDER,
        }
    }
}

/// One active stage of a plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StagePlan {
    /// The equalizer.
    Equalizer(EqualizerPlan),
    /// Crossfeed.
    Crossfeed(CrossfeedPlan),
    /// Mono, swap and balance.
    Mixer(MixerPlan),
    /// The limiter.
    Limiter(LimiterPlan),
    /// The compressor.
    Compressor(CompressorPlan),
    /// Convolution.
    Convolution(ConvolutionPlan),
    /// Pitch shift.
    Pitch(PitchPlan),
}

impl fmt::Display for StagePlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Equalizer(s) => s.fmt(f),
            Self::Crossfeed(s) => s.fmt(f),
            Self::Mixer(s) => s.fmt(f),
            Self::Limiter(s) => s.fmt(f),
            Self::Compressor(s) => s.fmt(f),
            Self::Convolution(s) => s.fmt(f),
            Self::Pitch(s) => s.fmt(f),
        }
    }
}

impl DspPlan {
    /// Whether no stage is active.
    pub fn is_empty(&self) -> bool {
        self.active().next().is_none()
    }

    /// The plan of `stage`, if it is active.
    pub fn stage(&self, stage: ChainStage) -> Option<StagePlan> {
        match stage {
            ChainStage::Pitch => self.pitch.map(StagePlan::Pitch),
            ChainStage::Equalizer => self.equalizer.map(StagePlan::Equalizer),
            ChainStage::Convolution => self.convolution.map(StagePlan::Convolution),
            ChainStage::Crossfeed => self.crossfeed.map(StagePlan::Crossfeed),
            ChainStage::ChannelMix => self.mixer.map(StagePlan::Mixer),
            ChainStage::Compressor => self.compressor.map(StagePlan::Compressor),
            ChainStage::Limiter => self.limiter.map(StagePlan::Limiter),
        }
    }

    /// The active stages, in order.
    pub fn active(&self) -> impl Iterator<Item = StagePlan> + '_ {
        self.order.iter().filter_map(|stage| self.stage(*stage))
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
        for stage in self.active() {
            item(f, &stage)?;
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
            ..DspPlan::default()
        };
        assert_eq!(
            plan.to_string(),
            "EQ 5 filters · preamp -4.5 dB → crossfeed 700 Hz · 4.5 dB → \
             mono · balance 2.0 dB left → limiter -1.0 dBTP · lookahead 2 ms"
        );
        assert!(DspPlan::default().is_empty());
        assert_eq!(DspPlan::default().to_string(), "");
        // Another order changes the text, not the stages.
        let mut moved = plan;
        moved.order.reverse();
        assert!(moved.to_string().starts_with("limiter -1.0 dBTP"));
        let plan = DspPlan {
            compressor: Some(CompressorPlan {
                night: true,
                threshold_db: -30.0,
                ratio: 4.0,
                makeup_db: 12.0,
                attack_ms: 5.0,
                release_ms: 250.0,
                knee_db: 10.0,
                lookahead_ms: 2.0,
            }),
            convolution: Some(ConvolutionPlan {
                taps: 4800,
                ir_channels: 2,
                file_rate: 44_100,
                resampled: true,
                latency_frames: 512,
                latency_ms: 10.666,
                wet_percent: 100.0,
                gain_db: 0.0,
                normalized: true,
            }),
            pitch: Some(PitchPlan {
                semitones: -2.0,
                latency_ms: 33.0,
            }),
            ..DspPlan::default()
        };
        assert_eq!(
            plan.to_string(),
            "pitch -2.00 semitones → convolution stereo · 4800 taps · resampled from 44100 Hz \
             · latency 10.7 ms → night mode · compressor -30.0 dB · 4:1 · makeup +12.0 dB"
        );
    }
}
