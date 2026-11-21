// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSP settings: the `[dsp]` table.
//!
//! The chain runs its stages in the order of `order` (by default pitch shift,
//! equalizer, convolution, crossfeed, channel mix, compressor, limiter), with a
//! switch for each stage. Every stage is off by default, so playback stays
//! bit-perfect until one is switched on. A stage keeps its values while it is off.
//! Saved chains (`[[dsp.chains]]`) keep an order and a choice of stages by name.

mod chain;
mod compressor;
mod convolution;
mod pitch;
mod speed;

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use super::{SettingsError, check_range};

pub use self::chain::{ChainStage, SavedChain};
pub use self::compressor::{CompressorPreset, CompressorSettings, CompressorValues};
pub use self::convolution::{ConvolutionSettings, ImpulseFile};
pub use self::pitch::PitchSettings;
pub use self::speed::SpeedSettings;

/// The centre frequencies of the 10-band graphic equalizer, in Hz: octaves from
/// 31.5 Hz to 16 kHz (ISO 266).
pub const GRAPHIC_10_BANDS_HZ: [f64; 10] = [
    31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

/// The centre frequencies of the 31-band graphic equalizer, in Hz: third octaves
/// from 20 Hz to 20 kHz (ISO 266).
pub const GRAPHIC_31_BANDS_HZ: [f64; 31] = [
    20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0,
    500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0,
    8000.0, 10000.0, 12500.0, 16000.0, 20000.0,
];

/// The DSP chain: the `[dsp]` table of `settings.toml`.
///
/// ```toml
/// [dsp]
/// resampler = "high"           # fast | medium | high | maximum
/// order = ["pitch", "equalizer", "convolution", "crossfeed", "channel-mix",
///          "compressor", "limiter"]  # stages left out follow in this order
/// track_equalizer = true       # tracks with their own equalizer use it
///
/// [[dsp.chains]]               # saved chains
/// name = "Night"
/// stages = ["equalizer", "compressor", "limiter"]
///
/// [dsp.equalizer]
/// enabled = true
/// mode = "parametric"          # parametric | graphic-10 | graphic-31
/// auto_preamp = true           # the preamp cuts by the largest boost
/// preamp_db = 0.0              # used when auto_preamp is false (-24 to +6)
/// graphic_10 = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
/// graphic_31 = [0.0, …]        # 31 values, -12 to +12 dB
///
/// [[dsp.equalizer.parametric]] # up to 64 filters
/// kind = "peak"                # peak | low-shelf | high-shelf | low-pass | high-pass
/// frequency_hz = 105.0         # 10 to 22000
/// gain_db = -3.1               # -30 to +30; not used by the pass filters
/// q = 0.7                      # 0.05 to 20
/// enabled = true
///
/// [[dsp.equalizer.presets]]    # the user's graphic presets
/// name = "Late night"
/// bands = [3.0, 2.0, …]        # 10 or 31 values
///
/// [dsp.crossfeed]
/// enabled = false
/// preset = "default"           # default | chu-moy | jan-meier | custom
/// cutoff_hz = 700              # custom only: 300 to 2000
/// level_db = 4.5               # custom only: 1 to 15
///
/// [dsp.channel_mix]
/// enabled = false
/// channels = "stereo"          # stereo | mono | swap
/// balance_db = 0.0             # -12 (left) to +12 (right)
/// include_lfe = false          # surround files: mix the LFE channel in
///
/// [dsp.limiter]
/// enabled = false
/// lookahead_ms = 2.0           # 1 to 5, in steps of 0.5
/// ceiling_db = -1.0            # true peak, -6 to 0 dBTP
///
/// [dsp.compressor]
/// enabled = false
/// preset = "night"             # night | gentle | custom
/// threshold_db = -30.0         # custom only: -60 to 0
/// ratio = 4.0                  # custom only: 1 to 20
/// attack_ms = 5.0              # custom only: 0.1 to 200
/// release_ms = 250.0           # custom only: 10 to 3000
/// knee_db = 10.0               # custom only: 0 to 24
/// makeup_db = 12.0             # custom only: 0 to 24
/// lookahead_ms = 2.0           # 1 to 5, in steps of 0.5
///
/// [dsp.convolution]
/// enabled = false
/// folder = "/path/to/impulses" # WAV files, one per sample rate
/// normalize = true             # the response boosts no frequency
/// gain_db = 0.0                # -24 to +12
/// wet_percent = 100.0          # 0 to 100
///
/// [[dsp.convolution.files]]    # before the folder's files
/// rate_hz = 48000
/// path = "/path/to/room-48k.wav"
///
/// [dsp.pitch]
/// enabled = false
/// semitones = 0.0              # -12 to +12
///
/// [dsp.speed]                  # per track, ahead of the chain
/// enabled = false
/// rate = 1.0                   # 0.5 to 2
/// keep_pitch = true            # false: the pitch follows the speed (tape)
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DspSettings {
    /// The quality of sample-rate conversion, where the device rate differs from the
    /// track's. It applies from the next track (or seek) on.
    pub resampler: ResamplerQuality,
    /// The equalizer, with its preamp.
    pub equalizer: EqualizerSettings,
    /// Crossfeed for headphones.
    pub crossfeed: CrossfeedSettings,
    /// Mono, balance and left/right swap.
    pub channel_mix: ChannelMixSettings,
    /// The true-peak limiter at the end of the chain.
    pub limiter: LimiterSettings,
    /// The dynamics compressor, with its night mode.
    pub compressor: CompressorSettings,
    /// Convolution with an impulse response per sample rate.
    pub convolution: ConvolutionSettings,
    /// Pitch shift.
    pub pitch: PitchSettings,
    /// Playback speed. Not a stage of the chain: it applies to each track before
    /// the tracks are mixed, and is not part of `order` or the saved chains.
    pub speed: SpeedSettings,
    /// The order of the stages. Stages it leaves out follow in the default order.
    pub order: Vec<ChainStage>,
    /// The saved chains.
    pub chains: Vec<SavedChain>,
    /// Whether tracks that have an equalizer of their own (stored in the library)
    /// play with it instead of the equalizer above.
    pub track_equalizer: bool,
}

impl Default for DspSettings {
    fn default() -> Self {
        Self {
            resampler: ResamplerQuality::default(),
            equalizer: EqualizerSettings::default(),
            crossfeed: CrossfeedSettings::default(),
            channel_mix: ChannelMixSettings::default(),
            limiter: LimiterSettings::default(),
            compressor: CompressorSettings::default(),
            convolution: ConvolutionSettings::default(),
            pitch: PitchSettings::default(),
            speed: SpeedSettings::default(),
            order: ChainStage::DEFAULT_ORDER.to_vec(),
            chains: Vec::new(),
            track_equalizer: true,
        }
    }
}

impl DspSettings {
    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        self.equalizer.validate()?;
        self.crossfeed.validate()?;
        self.channel_mix.validate()?;
        self.limiter.validate()?;
        self.compressor.validate()?;
        self.convolution.validate()?;
        self.pitch.validate()?;
        self.speed.validate()?;
        chain::check_unique("dsp.order", &self.order)?;
        for (i, saved) in self.chains.iter().enumerate() {
            let key = format!("dsp.chains[{i}]");
            saved.validate(&key)?;
            if self.chains[..i].iter().any(|c| c.name == saved.name) {
                return Err(SettingsError {
                    key: format!("{key}.name"),
                    problem: format!("is \"{}\", which an earlier chain has too", saved.name),
                });
            }
        }
        Ok(())
    }

    /// Whether any stage is switched on.
    pub fn any_enabled(&self) -> bool {
        ChainStage::DEFAULT_ORDER
            .into_iter()
            .any(|stage| self.is_enabled(stage))
    }

    /// Every stage in the order the chain runs them.
    pub fn effective_order(&self) -> Vec<ChainStage> {
        chain::complete(&self.order)
    }

    /// Whether `stage` is switched on.
    pub fn is_enabled(&self, stage: ChainStage) -> bool {
        match stage {
            ChainStage::Pitch => self.pitch.enabled,
            ChainStage::Equalizer => self.equalizer.enabled,
            ChainStage::Convolution => self.convolution.enabled,
            ChainStage::Crossfeed => self.crossfeed.enabled,
            ChainStage::ChannelMix => self.channel_mix.enabled,
            ChainStage::Compressor => self.compressor.enabled,
            ChainStage::Limiter => self.limiter.enabled,
        }
    }

    /// Switches `stage` on or off.
    pub fn set_enabled(&mut self, stage: ChainStage, on: bool) {
        match stage {
            ChainStage::Pitch => self.pitch.enabled = on,
            ChainStage::Equalizer => self.equalizer.enabled = on,
            ChainStage::Convolution => self.convolution.enabled = on,
            ChainStage::Crossfeed => self.crossfeed.enabled = on,
            ChainStage::ChannelMix => self.channel_mix.enabled = on,
            ChainStage::Compressor => self.compressor.enabled = on,
            ChainStage::Limiter => self.limiter.enabled = on,
        }
    }

    /// The saved chain named `name`.
    pub fn chain(&self, name: &str) -> Option<&SavedChain> {
        self.chains.iter().find(|c| c.name == name)
    }

    /// Saves the current order and choice of stages as the chain `name`, replacing
    /// a saved chain of that name.
    pub fn save_chain(&mut self, name: &str) {
        let stages = self
            .effective_order()
            .into_iter()
            .filter(|stage| self.is_enabled(*stage))
            .collect();
        let saved = SavedChain {
            name: name.to_owned(),
            stages,
        };
        match self.chains.iter_mut().find(|c| c.name == name) {
            Some(existing) => *existing = saved,
            None => self.chains.push(saved),
        }
    }

    /// Uses the saved chain `name`: its stages in its order, switched on, and every
    /// other stage switched off. Returns false when there is no such chain.
    pub fn apply_chain(&mut self, name: &str) -> bool {
        let Some(saved) = self.chain(name).cloned() else {
            return false;
        };
        self.order = chain::complete(&saved.stages);
        for stage in ChainStage::DEFAULT_ORDER {
            self.set_enabled(stage, saved.stages.contains(&stage));
        }
        true
    }
}

/// How much effort sample-rate conversion spends on quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResamplerQuality {
    /// Short filters, for slow machines.
    Fast,
    /// A balance of cost and quality.
    Medium,
    /// Transparent for listening.
    #[default]
    High,
    /// The longest filters.
    Maximum,
}

/// Which equalizer is used. Each keeps its own values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EqMode {
    /// Up to 64 filters of any kind.
    #[default]
    Parametric,
    /// Ten octave bands.
    #[serde(rename = "graphic-10")]
    Graphic10,
    /// Thirty-one third-octave bands.
    #[serde(rename = "graphic-31")]
    Graphic31,
}

/// The equalizer: the `[dsp.equalizer]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqualizerSettings {
    /// Whether the stage runs.
    pub enabled: bool,
    /// Which equalizer is used.
    pub mode: EqMode,
    /// The preamp cuts by the largest boost of the equalizer, so that it cannot
    /// clip by construction.
    pub auto_preamp: bool,
    /// The preamp when it is not automatic, in dB.
    pub preamp_db: f64,
    /// The gains of the 10-band graphic equalizer, in dB, at
    /// [`GRAPHIC_10_BANDS_HZ`].
    pub graphic_10: [f64; 10],
    /// The gains of the 31-band graphic equalizer, in dB, at
    /// [`GRAPHIC_31_BANDS_HZ`].
    pub graphic_31: [f64; 31],
    /// The filters of the parametric equalizer, in order.
    pub parametric: Vec<ParametricFilter>,
    /// The user's graphic presets.
    pub presets: Vec<GraphicPreset>,
}

impl EqualizerSettings {
    /// The range of a graphic band's gain, in dB.
    pub const BAND_RANGE: RangeInclusive<f64> = -12.0..=12.0;
    /// The range of the manual preamp, in dB.
    pub const PREAMP_RANGE: RangeInclusive<f64> = -24.0..=6.0;
    /// The most filters of the parametric equalizer.
    pub const MAX_FILTERS: usize = 64;

    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        check_number(
            "dsp.equalizer.preamp_db",
            self.preamp_db,
            Self::PREAMP_RANGE,
        )?;
        for (i, gain) in self.graphic_10.iter().enumerate() {
            check_number(
                &format!("dsp.equalizer.graphic_10[{i}]"),
                *gain,
                Self::BAND_RANGE,
            )?;
        }
        for (i, gain) in self.graphic_31.iter().enumerate() {
            check_number(
                &format!("dsp.equalizer.graphic_31[{i}]"),
                *gain,
                Self::BAND_RANGE,
            )?;
        }
        if self.parametric.len() > Self::MAX_FILTERS {
            return Err(SettingsError {
                key: "dsp.equalizer.parametric".into(),
                problem: format!(
                    "has {} filters, but may have at most {}",
                    self.parametric.len(),
                    Self::MAX_FILTERS
                ),
            });
        }
        for (i, filter) in self.parametric.iter().enumerate() {
            filter.validate(&format!("dsp.equalizer.parametric[{i}]"))?;
        }
        for (i, preset) in self.presets.iter().enumerate() {
            preset.validate(&format!("dsp.equalizer.presets[{i}]"))?;
        }
        Ok(())
    }

    /// The gains of the graphic equalizer `mode` uses, or `None` for the parametric
    /// one.
    pub fn graphic_gains(&self) -> Option<&[f64]> {
        match self.mode {
            EqMode::Parametric => None,
            EqMode::Graphic10 => Some(&self.graphic_10),
            EqMode::Graphic31 => Some(&self.graphic_31),
        }
    }

    /// The largest boost of the equalizer in use, in dB, or 0 when nothing boosts.
    /// For the graphic equalizers it is the largest band gain; for the parametric one
    /// the largest gain of an enabled peaking or shelving filter.
    pub fn largest_boost_db(&self) -> f64 {
        let boost = match self.graphic_gains() {
            Some(gains) => gains.iter().copied().fold(0.0, f64::max),
            None => self
                .parametric
                .iter()
                .filter(|f| f.enabled && f.kind.has_gain())
                .map(|f| f.gain_db)
                .fold(0.0, f64::max),
        };
        if boost.is_nan() { 0.0 } else { boost }
    }

    /// The preamp in dB: the negated largest boost when it is automatic, the manual
    /// value otherwise.
    pub fn effective_preamp_db(&self) -> f64 {
        if self.auto_preamp {
            -self.largest_boost_db()
        } else {
            self.preamp_db
        }
    }
}

impl Default for EqualizerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: EqMode::default(),
            auto_preamp: true,
            preamp_db: 0.0,
            graphic_10: [0.0; 10],
            graphic_31: [0.0; 31],
            parametric: Vec::new(),
            presets: Vec::new(),
        }
    }
}

/// The kind of a parametric filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FilterKind {
    /// A peaking (bell) filter.
    #[default]
    Peak,
    /// A low shelf.
    LowShelf,
    /// A high shelf.
    HighShelf,
    /// A second-order low pass.
    LowPass,
    /// A second-order high pass.
    HighPass,
}

impl FilterKind {
    /// Whether the filter has a gain: peaks and shelves.
    pub const fn has_gain(self) -> bool {
        matches!(self, Self::Peak | Self::LowShelf | Self::HighShelf)
    }
}

/// One filter of the parametric equalizer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParametricFilter {
    /// The kind of filter.
    pub kind: FilterKind,
    /// The centre or corner frequency, in Hz.
    pub frequency_hz: f64,
    /// The gain, in dB; pass filters have none.
    pub gain_db: f64,
    /// The quality factor.
    pub q: f64,
    /// Whether the filter is used. A disabled filter keeps its values.
    pub enabled: bool,
}

impl ParametricFilter {
    /// The range of the frequency, in Hz.
    pub const FREQUENCY_RANGE: RangeInclusive<f64> = 10.0..=22_000.0;
    /// The range of the gain, in dB.
    pub const GAIN_RANGE: RangeInclusive<f64> = -30.0..=30.0;
    /// The range of the quality factor.
    pub const Q_RANGE: RangeInclusive<f64> = 0.05..=20.0;

    /// Checks the values; `key` names the filter in errors.
    pub fn validate(&self, key: &str) -> Result<(), SettingsError> {
        check_number(
            &format!("{key}.frequency_hz"),
            self.frequency_hz,
            Self::FREQUENCY_RANGE,
        )?;
        check_number(&format!("{key}.gain_db"), self.gain_db, Self::GAIN_RANGE)?;
        check_number(&format!("{key}.q"), self.q, Self::Q_RANGE)
    }
}

impl Default for ParametricFilter {
    /// A peak at 1 kHz with no gain.
    fn default() -> Self {
        Self {
            kind: FilterKind::Peak,
            frequency_hz: 1000.0,
            gain_db: 0.0,
            q: std::f64::consts::FRAC_1_SQRT_2,
            enabled: true,
        }
    }
}

/// A named set of graphic equalizer gains.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicPreset {
    /// The name shown in the preset list.
    pub name: String,
    /// The gains, in dB: 10 or 31 of them.
    pub bands: Vec<f64>,
}

impl GraphicPreset {
    /// Checks the values; `key` names the preset in errors.
    pub fn validate(&self, key: &str) -> Result<(), SettingsError> {
        if self.bands.len() != 10 && self.bands.len() != 31 {
            return Err(SettingsError {
                key: format!("{key}.bands"),
                problem: format!("has {} values, but must have 10 or 31", self.bands.len()),
            });
        }
        for (i, gain) in self.bands.iter().enumerate() {
            check_number(
                &format!("{key}.bands[{i}]"),
                *gain,
                EqualizerSettings::BAND_RANGE,
            )?;
        }
        Ok(())
    }
}

/// A graphic equalizer preset that comes with the program.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuiltInPreset {
    /// A stable identifier, for example `bass-boost`.
    pub id: &'static str,
    /// The English name; front ends translate it.
    pub name: &'static str,
    /// The gains of the ten octave bands, in dB.
    pub octaves: [f64; 10],
}

impl BuiltInPreset {
    /// The presets, in the order the preset list shows them.
    pub const ALL: [Self; 13] = [
        Self::new("flat", "Flat", [0.0; 10]),
        Self::new(
            "bass-boost",
            "Bass boost",
            [6.0, 5.0, 4.0, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
        ),
        Self::new(
            "bass-cut",
            "Bass cut",
            [-6.0, -5.0, -4.0, -2.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
        ),
        Self::new(
            "treble-boost",
            "Treble boost",
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 2.0, 4.0, 5.0, 6.0],
        ),
        Self::new(
            "treble-cut",
            "Treble cut",
            [0.0, 0.0, 0.0, 0.0, 0.0, -0.5, -2.0, -4.0, -5.0, -6.0],
        ),
        Self::new(
            "loudness",
            "Loudness",
            [5.0, 3.5, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0, 3.0, 4.0],
        ),
        Self::new(
            "vocal",
            "Vocal",
            [-2.0, -2.0, -1.0, 1.0, 3.0, 3.5, 3.0, 1.5, 0.0, -1.0],
        ),
        Self::new(
            "rock",
            "Rock",
            [4.5, 3.5, 2.0, 0.0, -1.0, -0.5, 1.0, 2.5, 3.5, 4.0],
        ),
        Self::new(
            "pop",
            "Pop",
            [-1.0, 0.0, 1.5, 3.0, 3.5, 2.5, 1.0, 0.0, -0.5, -1.0],
        ),
        Self::new(
            "jazz",
            "Jazz",
            [3.0, 2.0, 1.0, 1.5, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0],
        ),
        Self::new(
            "classical",
            "Classical",
            [3.0, 2.5, 1.5, 1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0],
        ),
        Self::new(
            "electronic",
            "Electronic",
            [5.0, 4.0, 1.0, 0.0, -1.5, 1.0, 0.5, 1.0, 3.5, 4.5],
        ),
        Self::new(
            "acoustic",
            "Acoustic",
            [3.5, 3.0, 2.0, 1.0, 1.5, 1.5, 2.5, 3.0, 2.5, 1.5],
        ),
    ];

    const fn new(id: &'static str, name: &'static str, octaves: [f64; 10]) -> Self {
        Self { id, name, octaves }
    }

    /// The preset with the identifier `id`.
    pub fn find(id: &str) -> Option<&'static Self> {
        Self::ALL.iter().find(|preset| preset.id == id)
    }

    /// The gains for the 31-band equalizer: the octave gains interpolated linearly
    /// over the logarithm of the frequency, and held beyond the outer octaves.
    pub fn third_octaves(&self) -> [f64; 31] {
        let mut gains = [0.0; 31];
        for (gain, hz) in gains.iter_mut().zip(GRAPHIC_31_BANDS_HZ) {
            *gain = interpolate_octaves(&self.octaves, hz);
        }
        gains
    }
}

/// The gain at `hz` of the curve through the octave gains `octaves`, rounded to
/// 0.1 dB.
fn interpolate_octaves(octaves: &[f64; 10], hz: f64) -> f64 {
    let bands = &GRAPHIC_10_BANDS_HZ;
    let gain = if hz <= bands[0] {
        octaves[0]
    } else if hz >= bands[9] {
        octaves[9]
    } else {
        let high = bands.iter().position(|&band| band >= hz).unwrap_or(9);
        let low = high - 1;
        let t = (hz / bands[low]).ln() / (bands[high] / bands[low]).ln();
        octaves[low] * (1.0 - t) + octaves[high] * t
    };
    (gain * 10.0).round() / 10.0
}

/// A crossfeed strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CrossfeedPreset {
    /// 700 Hz, 4.5 dB: close to a speaker setup.
    #[default]
    Default,
    /// 700 Hz, 6.0 dB: Chu Moy's headphone amplifier.
    ChuMoy,
    /// 650 Hz, 9.5 dB: Jan Meier's, the mildest.
    JanMeier,
    /// The values in the settings.
    Custom,
}

impl CrossfeedPreset {
    /// The cut-off frequency in Hz and the level in dB of a named preset; `None` for
    /// [`Custom`](Self::Custom).
    pub const fn values(self) -> Option<(u32, f64)> {
        match self {
            Self::Default => Some((700, 4.5)),
            Self::ChuMoy => Some((700, 6.0)),
            Self::JanMeier => Some((650, 9.5)),
            Self::Custom => None,
        }
    }
}

/// Crossfeed: the `[dsp.crossfeed]` table.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CrossfeedSettings {
    /// Whether the stage runs.
    pub enabled: bool,
    /// The strength.
    pub preset: CrossfeedPreset,
    /// The cut-off frequency of the custom strength, in Hz.
    pub cutoff_hz: u32,
    /// The level of the custom strength, in dB: how much quieter the mixed-in
    /// opposite channel is at low frequencies.
    pub level_db: f64,
}

impl CrossfeedSettings {
    /// The range of the cut-off frequency, in Hz.
    pub const CUTOFF_RANGE: RangeInclusive<u32> = 300..=2000;
    /// The range of the level, in dB.
    pub const LEVEL_RANGE: RangeInclusive<f64> = 1.0..=15.0;

    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        check_range(
            "dsp.crossfeed.cutoff_hz",
            self.cutoff_hz,
            Self::CUTOFF_RANGE,
        )?;
        check_number("dsp.crossfeed.level_db", self.level_db, Self::LEVEL_RANGE)
    }

    /// The cut-off frequency in Hz and the level in dB in use: the preset's, or the
    /// custom values.
    pub fn effective(&self) -> (u32, f64) {
        self.preset
            .values()
            .unwrap_or((self.cutoff_hz, self.level_db))
    }
}

impl Default for CrossfeedSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: CrossfeedPreset::Default,
            cutoff_hz: 700,
            level_db: 4.5,
        }
    }
}

/// How the two front channels are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelMode {
    /// Left and right as they are.
    #[default]
    Stereo,
    /// Both channels carry their average.
    Mono,
    /// Left and right swapped.
    Swap,
}

/// The channel mixer: the `[dsp.channel_mix]` table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelMixSettings {
    /// Whether the stage runs.
    pub enabled: bool,
    /// Stereo, mono or swapped.
    pub channels: ChannelMode,
    /// The balance in dB: negative moves the sound to the left by lowering the right
    /// channel, positive to the right by lowering the left one.
    pub balance_db: f64,
    /// Surround files: mix the LFE channel into the downmix instead of dropping it.
    /// This applies whether or not the stage runs.
    pub include_lfe: bool,
}

impl ChannelMixSettings {
    /// The range of the balance, in dB.
    pub const BALANCE_RANGE: RangeInclusive<f64> = -12.0..=12.0;

    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        check_number(
            "dsp.channel_mix.balance_db",
            self.balance_db,
            Self::BALANCE_RANGE,
        )
    }
}

/// The limiter: the `[dsp.limiter]` table.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LimiterSettings {
    /// Whether the stage runs.
    pub enabled: bool,
    /// How far ahead the limiter looks, in milliseconds (1 to 5, in steps of 0.5).
    pub lookahead_ms: f64,
    /// The highest true peak let through, in dBTP.
    pub ceiling_db: f64,
}

impl LimiterSettings {
    /// The range of the lookahead, in milliseconds.
    pub const LOOKAHEAD_RANGE: RangeInclusive<f64> = 1.0..=5.0;
    /// The range of the ceiling, in dBTP.
    pub const CEILING_RANGE: RangeInclusive<f64> = -6.0..=0.0;

    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        check_number(
            "dsp.limiter.lookahead_ms",
            self.lookahead_ms,
            Self::LOOKAHEAD_RANGE,
        )?;
        if (self.lookahead_ms * 2.0).fract() != 0.0 {
            return Err(SettingsError {
                key: "dsp.limiter.lookahead_ms".into(),
                problem: format!("is {}, but must be a multiple of 0.5", self.lookahead_ms),
            });
        }
        check_number(
            "dsp.limiter.ceiling_db",
            self.ceiling_db,
            Self::CEILING_RANGE,
        )
    }
}

impl Default for LimiterSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            lookahead_ms: 2.0,
            ceiling_db: -1.0,
        }
    }
}

/// Checks that `value` is a number in `range`.
fn check_number(key: &str, value: f64, range: RangeInclusive<f64>) -> Result<(), SettingsError> {
    if value.is_nan() {
        return Err(SettingsError {
            key: key.into(),
            problem: "is not a number".into(),
        });
    }
    check_range(key, value, range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn everything_is_off_by_default() {
        let dsp = DspSettings::default();
        assert!(!dsp.any_enabled());
        assert_eq!(dsp.limiter.lookahead_ms, 2.0);
        assert_eq!(dsp.crossfeed.effective(), (700, 4.5));
        assert_eq!(dsp.validate(), Ok(()));
    }

    #[test]
    fn a_full_table_round_trips() {
        let mut settings = Settings::default();
        let eq = &mut settings.dsp.equalizer;
        eq.enabled = true;
        eq.mode = EqMode::Graphic31;
        eq.graphic_10[3] = -2.5;
        eq.graphic_31 = BuiltInPreset::ALL[1].third_octaves();
        eq.parametric = vec![
            ParametricFilter {
                kind: FilterKind::LowShelf,
                frequency_hz: 105.0,
                gain_db: 5.5,
                q: 0.7,
                enabled: true,
            },
            ParametricFilter {
                kind: FilterKind::HighPass,
                enabled: false,
                ..ParametricFilter::default()
            },
        ];
        eq.presets = vec![GraphicPreset {
            name: "Late night".into(),
            bands: vec![1.0; 10],
        }];
        settings.dsp.crossfeed.preset = CrossfeedPreset::JanMeier;
        settings.dsp.channel_mix.channels = ChannelMode::Swap;
        settings.dsp.channel_mix.balance_db = -2.0;
        settings.dsp.limiter.enabled = true;
        settings.dsp.limiter.lookahead_ms = 3.5;
        let text = toml::to_string(&settings).unwrap();
        assert!(text.contains("[dsp.equalizer]"), "{text}");
        assert!(text.contains("mode = \"graphic-31\""), "{text}");
        assert!(text.contains("kind = \"low-shelf\""), "{text}");
        assert!(text.contains("preset = \"jan-meier\""), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
        assert_eq!(settings.validate(), Ok(()));
    }

    #[test]
    fn the_documented_example_parses() {
        let text = "[dsp]\nresampler = \"fast\"\n[dsp.equalizer]\nenabled = true\nmode = \"graphic-10\"\n\
                    graphic_10 = [1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -2.0]\n\
                    [[dsp.equalizer.parametric]]\nkind = \"peak\"\nfrequency_hz = 105.0\n\
                    gain_db = -3.1\nq = 0.7\n\
                    [dsp.crossfeed]\nenabled = true\npreset = \"chu-moy\"\n\
                    [dsp.limiter]\nenabled = true\n";
        let settings: Settings = toml::from_str(text).unwrap();
        let dsp = &settings.dsp;
        assert_eq!(dsp.resampler, ResamplerQuality::Fast);
        assert_eq!(dsp.equalizer.graphic_gains().unwrap()[2], 3.0);
        assert_eq!(dsp.equalizer.parametric[0].gain_db, -3.1);
        assert!(dsp.equalizer.parametric[0].enabled);
        assert_eq!(dsp.crossfeed.effective(), (700, 6.0));
        assert_eq!(dsp.limiter.ceiling_db, -1.0);
        assert!(dsp.any_enabled());
        assert_eq!(settings.validate(), Ok(()));
    }

    #[test]
    fn values_are_checked() {
        let mut dsp = DspSettings::default();
        dsp.equalizer.graphic_31[30] = 12.5;
        assert_eq!(
            dsp.validate().unwrap_err().key,
            "dsp.equalizer.graphic_31[30]"
        );
        let mut dsp = DspSettings::default();
        dsp.equalizer.parametric = vec![ParametricFilter::default(); 65];
        assert!(dsp.validate().is_err());
        let mut dsp = DspSettings::default();
        dsp.equalizer.parametric = vec![ParametricFilter {
            q: f64::NAN,
            ..ParametricFilter::default()
        }];
        assert_eq!(
            dsp.validate().unwrap_err().key,
            "dsp.equalizer.parametric[0].q"
        );
        let mut dsp = DspSettings::default();
        dsp.limiter.lookahead_ms = 2.25;
        assert!(dsp.validate().is_err());
        let mut dsp = DspSettings::default();
        dsp.crossfeed.cutoff_hz = 250;
        assert!(dsp.validate().is_err());
        let mut dsp = DspSettings::default();
        dsp.equalizer.presets = vec![GraphicPreset {
            name: "Short".into(),
            bands: vec![0.0; 9],
        }];
        assert!(dsp.validate().is_err());
    }

    #[test]
    fn saved_chains_set_the_order_and_the_switches() {
        let mut dsp = DspSettings::default();
        assert_eq!(dsp.effective_order(), ChainStage::DEFAULT_ORDER.to_vec());
        dsp.order = vec![ChainStage::Limiter, ChainStage::Equalizer];
        assert_eq!(
            dsp.effective_order()[..3],
            [
                ChainStage::Limiter,
                ChainStage::Equalizer,
                ChainStage::Pitch
            ]
        );
        assert_eq!(dsp.effective_order().len(), 7);
        dsp.compressor.enabled = true;
        dsp.limiter.enabled = true;
        dsp.save_chain("Night");
        assert_eq!(
            dsp.chain("Night").unwrap().stages,
            [ChainStage::Limiter, ChainStage::Compressor]
        );
        let mut other = DspSettings::default();
        other.equalizer.enabled = true;
        other.chains = dsp.chains.clone();
        assert!(other.apply_chain("Night"));
        assert!(!other.equalizer.enabled);
        assert!(other.compressor.enabled && other.limiter.enabled);
        assert_eq!(other.effective_order()[0], ChainStage::Limiter);
        assert!(!other.apply_chain("Day"));
        assert_eq!(other.validate(), Ok(()));
        other.order.push(ChainStage::Limiter);
        assert_eq!(other.validate().unwrap_err().key, "dsp.order");
        other.order.pop();
        other.chains.push(other.chains[0].clone());
        assert_eq!(other.validate().unwrap_err().key, "dsp.chains[1].name");
    }

    #[test]
    fn the_new_tables_parse() {
        let text = "[dsp]\norder = [\"limiter\", \"compressor\"]\n\
                    [[dsp.chains]]\nname = \"Room\"\nstages = [\"convolution\"]\n\
                    [dsp.compressor]\nenabled = true\npreset = \"gentle\"\n\
                    [dsp.convolution]\nenabled = true\nfolder = \"/ir\"\nwet_percent = 50.0\n\
                    [[dsp.convolution.files]]\nrate_hz = 48000\npath = \"/ir/48.wav\"\n\
                    [dsp.pitch]\nenabled = true\nsemitones = -2.5\n\
                    [dsp.speed]\nenabled = true\nrate = 1.5\nkeep_pitch = false\n";
        let settings: Settings = toml::from_str(text).unwrap();
        let dsp = &settings.dsp;
        assert_eq!(settings.validate(), Ok(()));
        assert_eq!(dsp.compressor.preset, CompressorPreset::Gentle);
        assert_eq!(
            dsp.convolution.file_for(48_000).unwrap().to_str(),
            Some("/ir/48.wav")
        );
        assert!(dsp.convolution.normalize);
        assert_eq!(dsp.pitch.semitones, -2.5);
        assert_eq!(dsp.speed, SpeedSettings::at(1.5, false));
        assert_eq!(dsp.chains[0].stages, [ChainStage::Convolution]);
        assert!(dsp.track_equalizer);
        let back: Settings = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(back, settings);
    }

    #[test]
    fn the_automatic_preamp_cuts_by_the_largest_boost() {
        let mut eq = EqualizerSettings {
            mode: EqMode::Graphic10,
            ..EqualizerSettings::default()
        };
        eq.graphic_10[2] = 4.5;
        eq.graphic_10[5] = -6.0;
        assert_eq!(eq.effective_preamp_db(), -4.5);
        eq.mode = EqMode::Parametric;
        eq.parametric = vec![
            ParametricFilter {
                gain_db: 3.0,
                ..ParametricFilter::default()
            },
            ParametricFilter {
                gain_db: 9.0,
                enabled: false,
                ..ParametricFilter::default()
            },
            ParametricFilter {
                kind: FilterKind::LowPass,
                gain_db: 12.0,
                ..ParametricFilter::default()
            },
        ];
        assert_eq!(eq.effective_preamp_db(), -3.0);
        eq.auto_preamp = false;
        eq.preamp_db = -1.5;
        assert_eq!(eq.effective_preamp_db(), -1.5);
    }

    #[test]
    fn built_in_presets_fit_both_equalizers() {
        assert_eq!(BuiltInPreset::find("flat").unwrap().octaves, [0.0; 10]);
        for preset in &BuiltInPreset::ALL {
            let thirds = preset.third_octaves();
            // The octave centres of the 31-band equalizer carry the octave gains.
            for (octave, gain) in preset.octaves.iter().enumerate() {
                assert_eq!(thirds[2 + 3 * octave], *gain, "{}", preset.id);
            }
            let as_user = GraphicPreset {
                name: preset.name.into(),
                bands: thirds.to_vec(),
            };
            assert_eq!(as_user.validate("p"), Ok(()));
        }
    }
}
