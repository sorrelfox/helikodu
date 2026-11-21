// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The path plan: which stages a track's audio passes through on its way to the
//! device.
//!
//! The plan is computed once per track (and again when one of its inputs changes)
//! from the source format, the loudness information, the settings and the format the
//! device was opened with. It is plain data: the stages are built from it
//! ([`SourceChain`](super::SourceChain), [`OutputChain`](super::OutputChain)), and the
//! signal-path view shows it, for example
//! `44.1 kHz/16-bit stereo → ReplayGain -6.30 dB (track) → resample 44.1 → 48 kHz (high) → volume 85 % → TPDF dither 16-bit → s16le 48 kHz stereo`.
//!
//! When nothing needs to change the samples, the plan is **bit-perfect**: no gain,
//! no channel mixing, no resampling, no DSP, no crossfade, the volume at 100 % or
//! applied outside the engine, and a device format that carries the source samples
//! exactly. The ring buffer holds 32-bit floats, which carry integers of up to 24
//! bits exactly, except for a 32-bit integer device format: its ring holds
//! integers, and 32-bit sources that nothing changes go into it as integers, so
//! they are bit-perfect there.

use std::fmt;

use helikodu_model::audio::{AudioSpec, ChannelLayout, Endianness, SampleFormat, SampleRate};
use helikodu_model::player::Volume;
use helikodu_model::settings::{OutputFormat, OutputSettings, SampleRatePolicy, VolumeControl};

use super::{
    ClippingPrevention, FadeDurations, GainDecision, GainSource, LevellerPlan, LoudnessInfo,
    LoudnessSettings, MixKind, MixMatrix, MixOptions, PlaybackSpeed, REFERENCE_LUFS,
    ResampleMethod, ResamplerQuality, SpeedPlan, TimeScale, TpdfDither, resolve_track_gain,
};
use crate::dsp::{
    CompressorPlan, ConvolutionPlan, CrossfeedPlan, DspPlan, EqualizerPlan, LimiterPlan, MixerPlan,
    PitchPlan, StagePlan,
};
use crate::output::StreamConfig;

/// The format of a source, as the decoder delivers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceFormat {
    /// The decoded format.
    pub spec: AudioSpec,
    /// The significant bits per sample when they are fewer than the sample format
    /// holds: a 24-bit FLAC file decodes to 32-bit words.
    pub bits: Option<u32>,
}

impl SourceFormat {
    /// A source whose samples use every bit of their format.
    pub const fn new(spec: AudioSpec) -> Self {
        Self { spec, bits: None }
    }

    /// The same source with `bits` significant bits per sample.
    pub const fn with_bits(mut self, bits: u32) -> Self {
        self.bits = Some(bits);
        self
    }

    /// The significant bits per sample.
    pub fn significant_bits(&self) -> u32 {
        let full = self.spec.format.bits();
        self.bits.map_or(full, |bits| bits.min(full))
    }
}

/// Where the volume is applied for one stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VolumePlan {
    /// The sound server's stream volume; the samples are untouched.
    Stream,
    /// A hardware mixer element; the samples are untouched.
    Hardware,
    /// Always full volume.
    Fixed,
    /// The engine scales the samples, starting at this volume.
    Software(Volume),
}

impl VolumePlan {
    /// The plan for the volume setting `control`, on a backend that has a stream
    /// volume of its own or not, at `volume`.
    pub fn new(control: &VolumeControl, backend_has_volume: bool, volume: Volume) -> Self {
        match control {
            VolumeControl::System if backend_has_volume => Self::Stream,
            VolumeControl::System | VolumeControl::Software => Self::Software(volume),
            VolumeControl::Hardware(_) => Self::Hardware,
            VolumeControl::Fixed => Self::Fixed,
        }
    }

    /// Whether the engine changes the samples for the volume now.
    pub fn touches_samples(&self) -> bool {
        matches!(self, Self::Software(volume) if *volume != Volume::FULL)
    }
}

/// Settings of the processing path that are not part of the output settings.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProcessingSettings {
    /// Loudness normalization.
    pub loudness: LoudnessSettings,
    /// Resampler quality.
    pub resampler: ResamplerQuality,
    /// Downmix options.
    pub mix: MixOptions,
    /// Anti-click fade lengths.
    pub fades: FadeDurations,
    /// The playback speed.
    pub speed: PlaybackSpeed,
}

/// Everything the plan is computed from.
#[derive(Debug, Clone, PartialEq)]
pub struct PathRequest {
    /// The source format.
    pub source: SourceFormat,
    /// The source's loudness information.
    pub loudness: LoudnessInfo,
    /// Whether the track plays as part of its album, for the automatic loudness mode.
    pub album_context: bool,
    /// The track's own volume adjustment in dB, applied with the loudness gain.
    pub track_volume_db: f64,
    /// The format the device was opened with.
    pub device: AudioSpec,
    /// Where the volume is applied.
    pub volume: VolumePlan,
    /// Whether any DSP stage is active.
    pub dsp: bool,
    /// Whether the track is crossfaded with another.
    pub crossfade: bool,
}

/// A channel mixing step of the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelMixPlan {
    /// The source layout.
    pub from: ChannelLayout,
    /// The device layout.
    pub to: ChannelLayout,
    /// How the channels are mapped.
    pub kind: MixKind,
    /// The options the matrix is built with.
    pub options: MixOptions,
}

impl ChannelMixPlan {
    /// The matrix for this step.
    pub fn matrix(&self) -> MixMatrix {
        MixMatrix::new(self.from, self.to, self.options)
    }
}

impl fmt::Display for ChannelMixPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (from, to) = (self.from.channel_count(), self.to.channel_count());
        let verb = if to < from {
            "downmix"
        } else if to > from {
            "upmix"
        } else {
            "channel map"
        };
        write!(f, "{verb} {} → {}", self.from, self.to)
    }
}

/// A resampling step of the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResamplePlan {
    /// The source rate.
    pub from: SampleRate,
    /// The device rate.
    pub to: SampleRate,
    /// The resampler kind for this pair.
    pub method: ResampleMethod,
    /// The quality preset.
    pub quality: ResamplerQuality,
}

impl fmt::Display for ResamplePlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "resample {} → {} kHz ({})",
            KiloHertz(self.from).number(),
            KiloHertz(self.to).number(),
            self.quality
        )
    }
}

/// One stage of a plan, in signal order, for the signal-path view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlannedStage {
    /// Channel mapping, downmix or upmix.
    ChannelMix(ChannelMixPlan),
    /// The track's loudness gain.
    Gain(GainDecision),
    /// Sample-rate conversion.
    Resample(ResamplePlan),
    /// The crossfade with the neighbouring track.
    Crossfade,
    /// The DSP chain, when its stages are not known.
    Dsp,
    /// The equalizer of the DSP chain, with its preamp.
    Equalizer(EqualizerPlan),
    /// Crossfeed.
    Crossfeed(CrossfeedPlan),
    /// Mono, left/right swap and balance.
    Mixer(MixerPlan),
    /// The true-peak limiter.
    Limiter(LimiterPlan),
    /// The dynamics compressor.
    Compressor(CompressorPlan),
    /// Convolution with an impulse response.
    Convolution(ConvolutionPlan),
    /// Pitch shift.
    Pitch(PitchPlan),
    /// Playback speed.
    Speed(SpeedPlan),
    /// Software volume.
    Volume(Volume),
    /// TPDF dither to an integer width.
    Dither {
        /// The target width in bits.
        bits: u32,
    },
}

impl fmt::Display for PlannedStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChannelMix(mix) => mix.fmt(f),
            Self::Gain(gain) => gain.fmt(f),
            Self::Resample(resample) => resample.fmt(f),
            Self::Crossfade => f.write_str("crossfade"),
            Self::Dsp => f.write_str("DSP"),
            Self::Equalizer(stage) => stage.fmt(f),
            Self::Crossfeed(stage) => stage.fmt(f),
            Self::Mixer(stage) => stage.fmt(f),
            Self::Limiter(stage) => stage.fmt(f),
            Self::Compressor(stage) => stage.fmt(f),
            Self::Convolution(stage) => stage.fmt(f),
            Self::Pitch(stage) => stage.fmt(f),
            Self::Speed(stage) => stage.fmt(f),
            Self::Volume(volume) => write!(f, "volume {:.0} %", volume.position() * 100.0),
            Self::Dither { bits } => write!(f, "TPDF dither {bits}-bit"),
        }
    }
}

/// The stages one track passes through, and whether it plays bit-perfect.
#[derive(Debug, Clone, PartialEq)]
pub struct PathPlan {
    /// The source format.
    pub source: SourceFormat,
    /// The device format.
    pub device: AudioSpec,
    /// The loudness gain, unless it is off or exactly 0 dB (but always with the
    /// leveller).
    pub gain: Option<GainDecision>,
    /// The leveller, for a track without loudness values when the settings ask for
    /// it.
    pub leveller: Option<LevellerPlan>,
    /// Channel mixing, unless the layouts match.
    pub channel_mix: Option<ChannelMixPlan>,
    /// Resampling, unless the rates match.
    pub resample: Option<ResamplePlan>,
    /// Another speed than the normal one.
    pub speed: Option<SpeedPlan>,
    /// Whether the track is crossfaded.
    pub crossfade: bool,
    /// Whether the DSP chain is active.
    pub dsp: bool,
    /// The DSP chain's active stages, when known (see [`with_dsp`](Self::with_dsp)).
    pub dsp_stages: DspPlan,
    /// Where the volume is applied.
    pub volume: VolumePlan,
    /// The width the output is dithered to while the samples are processed, if the
    /// device takes integers of 24 bits or fewer.
    pub dither: Option<u32>,
    /// Whether the samples are processed even at full volume, so dither always runs.
    /// Otherwise it runs only while the volume or a fade changes the samples.
    pub always_dither: bool,
}

impl PathPlan {
    /// Computes the plan for `request`.
    pub fn new(request: &PathRequest, settings: &ProcessingSettings) -> Self {
        let source = request.source;
        let device = request.device;
        let gain = resolve_track_gain(
            &request.loudness,
            &settings.loudness,
            request.album_context,
            request.track_volume_db,
        )
        .filter(|gain| !gain.is_unity() || gain.source == GainSource::Leveller);
        let leveller = gain
            .filter(|gain| gain.source == GainSource::Leveller)
            .map(|_| LevellerPlan {
                target_lufs: REFERENCE_LUFS + settings.loudness.preamp_db,
                peak_safe: settings.loudness.clipping == ClippingPrevention::Peak,
            });
        let channel_mix = {
            let matrix = MixMatrix::new(source.spec.layout, device.layout, settings.mix);
            (!matrix.is_identity()).then(|| ChannelMixPlan {
                from: source.spec.layout,
                to: device.layout,
                kind: matrix.kind(),
                options: settings.mix,
            })
        };
        let resample = (source.spec.rate != device.rate).then(|| ResamplePlan {
            from: source.spec.rate,
            to: device.rate,
            method: ResampleMethod::for_rates(source.spec.rate, device.rate),
            quality: settings.resampler,
        });
        let speed = SpeedPlan::new(source.spec.rate, settings.speed, settings.resampler);
        let processed = gain.is_some()
            || channel_mix.is_some()
            || resample.is_some()
            || speed.is_some()
            || request.dsp
            || request.crossfade;
        let exact = format_is_exact(&source, device.format);
        let dither = TpdfDither::for_format(device.format).map(|d| d.bits());
        Self {
            source,
            device,
            gain,
            leveller,
            channel_mix,
            resample,
            speed,
            crossfade: request.crossfade,
            dsp: request.dsp,
            dsp_stages: DspPlan::default(),
            volume: request.volume,
            dither,
            always_dither: processed || !exact,
        }
    }

    /// The same plan with the DSP chain's active stages: a chain with any active
    /// stage makes the samples processed, and its stages appear in
    /// [`stages`](Self::stages) instead of a plain "DSP".
    pub fn with_dsp(mut self, dsp: DspPlan) -> Self {
        if !dsp.is_empty() {
            self.dsp = true;
            self.always_dither = true;
        }
        self.dsp_stages = dsp;
        self
    }

    /// How the source's frames and the device's frames convert, at the plan's
    /// speed.
    pub fn scale(&self) -> TimeScale {
        TimeScale::new(self.source.spec.rate, self.device.rate).with_speed(self.speed.as_ref())
    }

    /// Whether the device receives the decoded samples unchanged.
    pub fn is_bit_perfect(&self) -> bool {
        !self.always_dither && !self.volume.touches_samples()
    }

    /// The software volume the output side starts at, if the engine applies the
    /// volume.
    pub fn software_volume(&self) -> Option<Volume> {
        match self.volume {
            VolumePlan::Software(volume) => Some(volume),
            _ => None,
        }
    }

    /// The active stages in signal order. Empty for a bit-perfect plan.
    pub fn stages(&self) -> Vec<PlannedStage> {
        let mut stages = Vec::new();
        stages.extend(self.channel_mix.map(PlannedStage::ChannelMix));
        stages.extend(self.gain.map(PlannedStage::Gain));
        stages.extend(self.speed.map(PlannedStage::Speed));
        stages.extend(self.resample.map(PlannedStage::Resample));
        if self.crossfade {
            stages.push(PlannedStage::Crossfade);
        }
        if self.dsp && self.dsp_stages.is_empty() {
            stages.push(PlannedStage::Dsp);
        }
        stages.extend(self.dsp_stages.active().map(|stage| match stage {
            StagePlan::Equalizer(s) => PlannedStage::Equalizer(s),
            StagePlan::Crossfeed(s) => PlannedStage::Crossfeed(s),
            StagePlan::Mixer(s) => PlannedStage::Mixer(s),
            StagePlan::Limiter(s) => PlannedStage::Limiter(s),
            StagePlan::Compressor(s) => PlannedStage::Compressor(s),
            StagePlan::Convolution(s) => PlannedStage::Convolution(s),
            StagePlan::Pitch(s) => PlannedStage::Pitch(s),
        }));
        if let Some(volume) = self.software_volume().filter(|v| *v != Volume::FULL) {
            stages.push(PlannedStage::Volume(volume));
        }
        if let Some(bits) = self.dither.filter(|_| !self.is_bit_perfect()) {
            stages.push(PlannedStage::Dither { bits });
        }
        stages
    }
}

impl fmt::Display for PathPlan {
    /// The signal path in one line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let spec = self.source.spec;
        let bits = if spec.format.is_float() {
            "float".to_owned()
        } else {
            format!("{}-bit", self.source.significant_bits())
        };
        write!(f, "{}/{bits} {}", KiloHertz(spec.rate), spec.layout)?;
        if self.is_bit_perfect() {
            f.write_str(" → bit-perfect")?;
        } else {
            for stage in self.stages() {
                write!(f, " → {stage}")?;
            }
        }
        write!(
            f,
            " → {} {} {}",
            self.device.format,
            KiloHertz(self.device.rate),
            self.device.layout
        )
    }
}

/// Whether the device format carries the source's samples exactly through the ring
/// buffer: floats, exact for up to 24 bits, or integers for a 32-bit integer device.
fn format_is_exact(source: &SourceFormat, device: SampleFormat) -> bool {
    let format = source.spec.format;
    if format.is_integer() && device.is_integer() {
        let bits = source.significant_bits();
        let ring_bits = if matches!(device, SampleFormat::S32(_)) {
            32
        } else {
            24
        };
        bits <= ring_bits && bits <= device.bits()
    } else {
        matches!(format, SampleFormat::F32(_)) && matches!(device, SampleFormat::F32(_))
    }
}

/// The stream to ask the backend for, playing `source` with `settings`: the rate
/// from the sample-rate policy, the format from the format setting (the source's own
/// for "automatic"), and the device, mode and buffer.
///
/// `settings` are the effective settings, with any device profile already applied.
pub fn stream_config(settings: &OutputSettings, source: &SourceFormat) -> StreamConfig {
    let (rate, request_rate) = match settings.sample_rate {
        SampleRatePolicy::MatchTrack => (source.spec.rate, true),
        SampleRatePolicy::DeviceDefault => (source.spec.rate, false),
        SampleRatePolicy::Fixed(rate) => (rate, true),
    };
    let format = settings
        .format
        .sample_format(Endianness::NATIVE)
        .unwrap_or_else(|| native_format(source));
    let mut config = StreamConfig::new(AudioSpec::new(format, rate, source.spec.layout));
    config.device = settings.device.clone();
    config.mode = settings.mode;
    config.request_rate = request_rate;
    config.buffer = std::time::Duration::from_millis(u64::from(settings.buffer_ms));
    config
}

/// The device format that carries `source` best.
fn native_format(source: &SourceFormat) -> SampleFormat {
    let e = Endianness::NATIVE;
    let format = source.spec.format;
    if !format.is_integer() {
        return OutputFormat::F32
            .sample_format(e)
            .unwrap_or(SampleFormat::F32(e));
    }
    match source.significant_bits() {
        0..=8 if format == SampleFormat::U8 => SampleFormat::U8,
        0..=16 => SampleFormat::S16(e),
        17..=24 => SampleFormat::S24(e),
        _ => SampleFormat::S32(e),
    }
}

/// A rate shown in kilohertz: `44.1 kHz`, `48 kHz`, `22.05 kHz`.
struct KiloHertz(SampleRate);

impl KiloHertz {
    fn number(&self) -> String {
        let text = format!("{:.3}", f64::from(self.0.hz()) / 1000.0);
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

impl fmt::Display for KiloHertz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} kHz", self.number())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{LoudnessMode, PlannedStage};
    use Endianness::Little;

    fn spec(format: SampleFormat, hz: u32, layout: ChannelLayout) -> AudioSpec {
        AudioSpec::new(format, SampleRate::new(hz).unwrap(), layout)
    }

    fn request(source: AudioSpec, device: AudioSpec) -> PathRequest {
        PathRequest {
            source: SourceFormat::new(source),
            loudness: LoudnessInfo::default(),
            album_context: false,
            track_volume_db: 0.0,
            device,
            volume: VolumePlan::Fixed,
            dsp: false,
            crossfade: false,
        }
    }

    fn cd() -> AudioSpec {
        spec(SampleFormat::S16(Little), 44_100, ChannelLayout::STEREO)
    }

    #[test]
    fn untouched_source_is_bit_perfect() {
        let plan = PathPlan::new(&request(cd(), cd()), &ProcessingSettings::default());
        assert!(plan.is_bit_perfect());
        assert!(plan.stages().is_empty());
        assert_eq!(
            plan.to_string(),
            "44.1 kHz/16-bit stereo → bit-perfect → s16le 44.1 kHz stereo"
        );
        // 16 bits into a 24-bit device are carried exactly too.
        let wide = spec(
            SampleFormat::S24Packed(Little),
            44_100,
            ChannelLayout::STEREO,
        );
        assert!(
            PathPlan::new(&request(cd(), wide), &ProcessingSettings::default()).is_bit_perfect()
        );
    }

    #[test]
    fn each_condition_breaks_bit_perfection() {
        let settings = ProcessingSettings::default();
        let mut r = request(cd(), cd());
        r.volume = VolumePlan::Software(Volume::new(0.85).unwrap());
        let plan = PathPlan::new(&r, &settings);
        assert!(!plan.is_bit_perfect());
        assert_eq!(
            plan.stages(),
            [
                PlannedStage::Volume(Volume::new(0.85).unwrap()),
                PlannedStage::Dither { bits: 16 }
            ]
        );
        // Software volume at 100 % stays exact.
        r.volume = VolumePlan::Software(Volume::FULL);
        assert!(PathPlan::new(&r, &settings).is_bit_perfect());

        let mut r = request(cd(), cd());
        r.dsp = true;
        assert!(!PathPlan::new(&r, &settings).is_bit_perfect());
        let mut r = request(cd(), cd());
        r.crossfade = true;
        assert!(!PathPlan::new(&r, &settings).is_bit_perfect());

        let narrow = spec(SampleFormat::S16(Little), 44_100, ChannelLayout::STEREO);
        let hi_res = SourceFormat::new(spec(
            SampleFormat::S32(Little),
            44_100,
            ChannelLayout::STEREO,
        ))
        .with_bits(24);
        let mut r = request(hi_res.spec, narrow);
        r.source = hi_res;
        let plan = PathPlan::new(&r, &settings);
        assert!(!plan.is_bit_perfect());
        assert_eq!(plan.dither, Some(16));
        // The same 24-bit source on a 32-bit device is exact.
        r.device = spec(SampleFormat::S32(Little), 44_100, ChannelLayout::STEREO);
        assert!(PathPlan::new(&r, &settings).is_bit_perfect());
        // So is a true 32-bit source: the ring of a 32-bit device holds integers.
        r.source = SourceFormat::new(hi_res.spec);
        assert!(PathPlan::new(&r, &settings).is_bit_perfect());
        // Not on a 24-bit device.
        r.device = spec(SampleFormat::S24(Little), 44_100, ChannelLayout::STEREO);
        assert!(!PathPlan::new(&r, &settings).is_bit_perfect());
    }

    #[test]
    fn the_leveller_takes_the_place_of_the_untagged_gain() {
        let mut settings = ProcessingSettings::default();
        settings.loudness.mode = super::super::LoudnessMode::Track;
        settings.loudness.untagged_preamp_db = 0.0;
        let r = request(cd(), cd());
        assert!(PathPlan::new(&r, &settings).is_bit_perfect());
        settings.loudness.leveller = true;
        settings.loudness.preamp_db = 2.0;
        let plan = PathPlan::new(&r, &settings);
        assert!(!plan.is_bit_perfect());
        assert_eq!(
            plan.leveller,
            Some(LevellerPlan {
                target_lufs: -16.0,
                peak_safe: true
            })
        );
        assert_eq!(plan.gain.map(|g| g.source), Some(GainSource::Leveller));
        assert!(plan.to_string().contains("leveller (no loudness values)"));
        // A track with values keeps its gain.
        let mut tagged = r;
        tagged.loudness.track_gain_db = Some(-3.0);
        assert_eq!(PathPlan::new(&tagged, &settings).leveller, None);
    }

    #[test]
    fn full_signal_path() {
        let source = spec(
            SampleFormat::S32(Little),
            96_000,
            ChannelLayout::SURROUND_5_1,
        );
        let device = spec(SampleFormat::S16(Little), 48_000, ChannelLayout::STEREO);
        let mut r = request(source, device);
        r.source = r.source.with_bits(24);
        r.loudness = LoudnessInfo {
            track_gain_db: Some(-6.3),
            ..LoudnessInfo::default()
        };
        r.volume = VolumePlan::Software(Volume::new(0.85).unwrap());
        let settings = ProcessingSettings {
            loudness: LoudnessSettings {
                mode: LoudnessMode::Track,
                ..LoudnessSettings::default()
            },
            ..ProcessingSettings::default()
        };
        let plan = PathPlan::new(&r, &settings);
        assert_eq!(
            plan.to_string(),
            "96 kHz/24-bit 5.1 → downmix 5.1 → stereo → ReplayGain -6.30 dB (track) → \
             resample 96 → 48 kHz (high) → volume 85 % → TPDF dither 16-bit → s16le 48 kHz stereo"
        );
        assert_eq!(plan.resample.unwrap().method, ResampleMethod::Synchronous);
        assert_eq!(plan.channel_mix.unwrap().kind, MixKind::Matrix);
    }

    #[test]
    fn unity_gain_and_matching_unknown_layouts_are_not_stages() {
        let mut r = request(cd(), cd());
        r.source.spec.layout = ChannelLayout::unspecified(2).unwrap();
        r.loudness.track_gain_db = Some(0.0);
        let settings = ProcessingSettings {
            loudness: LoudnessSettings {
                mode: LoudnessMode::Track,
                ..LoudnessSettings::default()
            },
            ..ProcessingSettings::default()
        };
        assert!(PathPlan::new(&r, &settings).is_bit_perfect());
    }

    #[test]
    fn the_track_volume_is_a_gain_stage() {
        let settings = ProcessingSettings::default();
        let mut r = request(cd(), cd());
        r.track_volume_db = -3.0;
        let plan = PathPlan::new(&r, &settings);
        assert!(!plan.is_bit_perfect());
        let gain = plan.gain.unwrap();
        assert_eq!(gain.db, -3.0);
        assert!((gain.linear() - 0.707_945_784_384_137_9).abs() < 1e-12);
        assert_eq!(
            plan.to_string(),
            "44.1 kHz/16-bit stereo → track volume -3.00 dB → TPDF dither 16-bit → \
             s16le 44.1 kHz stereo"
        );
        // Zero keeps a bit-perfect path bit-perfect.
        r.track_volume_db = 0.0;
        assert!(PathPlan::new(&r, &settings).is_bit_perfect());
    }

    #[test]
    fn volume_plan_follows_the_setting_and_backend() {
        let v = Volume::new(0.5).unwrap();
        assert_eq!(
            VolumePlan::new(&VolumeControl::System, true, v),
            VolumePlan::Stream
        );
        assert_eq!(
            VolumePlan::new(&VolumeControl::System, false, v),
            VolumePlan::Software(v)
        );
        assert_eq!(
            VolumePlan::new(&VolumeControl::Hardware("PCM".into()), false, v),
            VolumePlan::Hardware
        );
        assert_eq!(
            VolumePlan::new(&VolumeControl::Fixed, true, v),
            VolumePlan::Fixed
        );
    }

    #[test]
    fn stream_config_follows_the_policies() {
        let source = SourceFormat::new(spec(
            SampleFormat::S32(Little),
            96_000,
            ChannelLayout::STEREO,
        ))
        .with_bits(24);
        let mut settings = OutputSettings::default();
        let config = stream_config(&settings, &source);
        assert_eq!(config.spec.rate.hz(), 96_000);
        assert!(config.request_rate);
        assert_eq!(config.spec.format, SampleFormat::S24(Endianness::NATIVE));

        settings.sample_rate = SampleRatePolicy::Fixed(SampleRate::HZ_48000);
        settings.format = OutputFormat::S16;
        let config = stream_config(&settings, &source);
        assert_eq!(config.spec.rate, SampleRate::HZ_48000);
        assert_eq!(config.spec.format, SampleFormat::S16(Endianness::NATIVE));

        settings.sample_rate = SampleRatePolicy::DeviceDefault;
        assert!(!stream_config(&settings, &source).request_rate);
    }

    #[test]
    fn kilohertz() {
        assert_eq!(KiloHertz(SampleRate::HZ_44100).to_string(), "44.1 kHz");
        assert_eq!(
            KiloHertz(SampleRate::new(22_050).unwrap()).to_string(),
            "22.05 kHz"
        );
        assert_eq!(KiloHertz(SampleRate::HZ_48000).to_string(), "48 kHz");
    }

    #[test]
    fn dsp_stages_join_the_signal_path() {
        use crate::dsp::DspChain;
        use helikodu_model::settings::DspSettings;

        let mut dsp = DspSettings::default();
        let plan = PathPlan::new(&request(cd(), cd()), &ProcessingSettings::default());
        // Stages that are switched on but change nothing keep the path bit-perfect.
        dsp.equalizer.enabled = true;
        let chain = DspChain::new(&dsp, 44_100, 2);
        assert!(plan.clone().with_dsp(chain.plan()).is_bit_perfect());
        dsp.equalizer.mode = helikodu_model::settings::EqMode::Graphic10;
        dsp.equalizer.graphic_10[0] = 3.0;
        dsp.limiter.enabled = true;
        let chain = DspChain::new(&dsp, 44_100, 2);
        let plan = plan.with_dsp(chain.plan());
        assert!(!plan.is_bit_perfect());
        assert_eq!(
            plan.to_string(),
            "44.1 kHz/16-bit stereo → EQ 10-band · preamp -3.0 dB → \
             limiter -1.0 dBTP · lookahead 2 ms → TPDF dither 16-bit → s16le 44.1 kHz stereo"
        );
    }
}
