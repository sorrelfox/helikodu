// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The signal path: what happens to the samples of the item being heard on their way
//! from the file to the device, and the playback counters shown beside it.
//!
//! The engine builds a [`SignalPath`] from the plan it actually plays with and the
//! format the device was opened with, and sends it again whenever any of that
//! changes. Front ends show it as it comes and compute nothing themselves.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::ItemId;
use crate::audio::{AudioSpec, DeviceSelection, OutputBackendKind, OutputMode, SampleRate};

/// The path of the item being heard, from its decoded format to the device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalPath {
    /// The item.
    pub item: ItemId,
    /// The format the decoder delivers.
    pub source: SignalSource,
    /// Every stage of the path, in signal order, including those that do nothing
    /// (with a [`StageState`] that says why).
    pub stages: Vec<SignalStage>,
    /// The device and the format it was opened with.
    pub device: SignalDevice,
    /// What reaches the device, in one word.
    pub lamp: SignalLamp,
}

impl SignalPath {
    /// The stage of `kind`, if the path has one.
    pub fn stage(&self, kind: StageKind) -> Option<&SignalStage> {
        self.stages.iter().find(|stage| stage.kind == kind)
    }

    /// The stages after the decoder that change the samples.
    pub fn active_stages(&self) -> impl Iterator<Item = &SignalStage> {
        self.stages
            .iter()
            .filter(|stage| stage.kind != StageKind::Decoder && stage.state == StageState::Active)
    }

    /// Whether a stage of the DSP chain (equalizer, crossfeed, channel mixer,
    /// limiter) changes the samples.
    pub fn dsp_active(&self) -> bool {
        self.active_stages().any(|stage| stage.kind.is_dsp())
    }
}

impl fmt::Display for SignalPath {
    /// The path in one line, for diagnostics:
    /// `flac 96 kHz/24-bit stereo → resample 96 → 48 kHz (high) → volume 85 % →
    /// pipewire "Speakers" s32le 48 kHz stereo (shared) [resampled]`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.source)?;
        let mut active = self.active_stages().peekable();
        if active.peek().is_none() && self.lamp == SignalLamp::BitPerfect {
            f.write_str(" → bit-perfect")?;
        }
        for stage in active {
            write!(f, " → {}", stage.detail)?;
        }
        write!(f, " → {} [{}]", self.device, self.lamp)
    }
}

/// The decoded source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalSource {
    /// The format and layout of the decoded samples.
    pub spec: AudioSpec,
    /// The significant bits per sample: 24 for 24-bit audio decoded into 32-bit
    /// words. For float sources the bits of the float format.
    pub bits: u32,
    /// The codec, as the decoder names it (`flac`, `mp3`), if known.
    pub codec: Option<String>,
}

impl fmt::Display for SignalSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(codec) = &self.codec {
            write!(f, "{codec} ")?;
        }
        let bits = if self.spec.format.is_float() {
            "float".to_owned()
        } else {
            format!("{}-bit", self.bits)
        };
        write!(
            f,
            "{}/{bits} {}",
            Kilohertz(self.spec.rate),
            self.spec.layout
        )
    }
}

/// What a stage is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageKind {
    /// The decoder. Active for lossy codecs, which reconstruct an approximation.
    Decoder,
    /// Channel mapping, downmix or upmix to the device's layout.
    ChannelMix,
    /// The loudness gain (ReplayGain, R128).
    Loudness,
    /// Sample-rate conversion by the engine.
    Resampler,
    /// The crossfade with the neighbouring item.
    Crossfade,
    /// The equalizer of the DSP chain, with its preamp.
    Equalizer,
    /// Crossfeed.
    Crossfeed,
    /// Mono, left/right swap and balance.
    ChannelMixer,
    /// The true-peak limiter.
    Limiter,
    /// The dynamics compressor.
    Compressor,
    /// Convolution with an impulse response.
    Convolution,
    /// Pitch shift.
    Pitch,
    /// The volume.
    Volume,
    /// TPDF dither to the device's integer width.
    Dither,
    /// Conversion to the device's sample format, and what the sound server does
    /// after the engine (its own resampling or mixing).
    Conversion,
}

impl StageKind {
    /// Whether the stage belongs to the DSP chain.
    pub fn is_dsp(self) -> bool {
        matches!(
            self,
            Self::Equalizer
                | Self::Crossfeed
                | Self::ChannelMixer
                | Self::Limiter
                | Self::Compressor
                | Self::Convolution
                | Self::Pitch
        )
    }
}

/// Whether a stage changes the samples, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageState {
    /// It changes the samples.
    Active,
    /// It is switched on but changes nothing now (a flat equalizer, 0 dB of gain,
    /// full volume).
    Bypassed,
    /// It is switched off in the settings.
    Off,
    /// The path does not need it (the rates or layouts already match, the format
    /// carries the samples exactly).
    NotNeeded,
    /// It happens outside the engine without touching the samples it sends: the
    /// sound server's stream volume, a hardware mixer, a fixed volume.
    External,
}

/// One stage of the path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalStage {
    /// What it is.
    pub kind: StageKind,
    /// Whether it changes the samples.
    pub state: StageState,
    /// What it does, in the engine's diagnostic wording (`ReplayGain -6.30 dB
    /// (track)`, `resample 96 → 48 kHz (high)`, `s16le → s32le`). Values, not
    /// sentences; front ends translate the [`state`](Self::state), not this.
    pub detail: String,
    /// The delay the stage adds while it changes the samples, in milliseconds
    /// (lookahead, block or analysis latency); 0 for stages that add none.
    #[serde(default)]
    pub delay_ms: f64,
}

/// The device end of the path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalDevice {
    /// The backend.
    pub backend: OutputBackendKind,
    /// The device.
    pub device: DeviceSelection,
    /// The device's description from the backend's list, if it is there.
    pub description: Option<String>,
    /// Shared or exclusive.
    pub mode: OutputMode,
    /// The negotiated format the engine writes: rate, layout and sample format.
    pub format: AudioSpec,
    /// The rate the device itself runs at, when the backend reports it and it is not
    /// [`format`](Self::format)'s rate (a sound server's graph at another rate).
    pub device_rate: Option<SampleRate>,
    /// The most integer bits the backend carries exactly, when it is fewer than the
    /// format holds: PipeWire processes in 32-bit float, exact up to 24 bits.
    pub exact_bits: Option<u32>,
    /// The length of the ring buffer, in milliseconds.
    pub buffer_ms: u32,
}

impl fmt::Display for SignalDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.backend)?;
        match &self.description {
            Some(description) => write!(f, " \"{description}\"")?,
            None => write!(f, " {}", self.device)?,
        }
        write!(
            f,
            " {} {} {}",
            self.format.format,
            Kilohertz(self.format.rate),
            self.format.layout
        )?;
        if let Some(rate) = self.device_rate {
            write!(f, " (device at {})", Kilohertz(rate))?;
        }
        let mode = match self.mode {
            OutputMode::Shared => "shared",
            OutputMode::Exclusive => "exclusive",
        };
        write!(f, " ({mode})")
    }
}

/// What reaches the device, from best to worst. The lamp shows the worst thing that
/// happens on the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalLamp {
    /// The device receives the decoded samples unchanged (on PipeWire up to the
    /// [`exact_bits`](SignalDevice::exact_bits) it carries).
    BitPerfect,
    /// The samples are changed (gain, DSP, volume, dither, channel mixing, a sound
    /// server's mixing) at the source rate and with at least the source's bits.
    LosslessProcessed,
    /// The rate is converted, by the engine or by the sound server.
    Resampled,
    /// The device gets fewer bits than the source has.
    Lossy,
}

impl fmt::Display for SignalLamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::BitPerfect => "bit-perfect",
            Self::LosslessProcessed => "processed",
            Self::Resampled => "resampled",
            Self::Lossy => "lossy",
        })
    }
}

/// What the limiter has done, for its activity readout.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LimiterActivity {
    /// The gain reduction applied to the last frame, in dB (0 or positive).
    pub reduction_db: f64,
    /// The largest reduction in the track, in dB.
    pub largest_db: f64,
    /// How often limiting has started in the track.
    pub times: u64,
}

/// What the compressor has done, for its activity readout.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct CompressorActivity {
    /// The gain reduction applied to the last frame, in dB (0 or positive).
    pub reduction_db: f64,
    /// The largest reduction in the track, in dB.
    pub largest_db: f64,
}

/// Playback counters of the open stream, sent a few times per second while playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlaybackStats {
    /// The length of the ring buffer, in milliseconds.
    pub buffer_ms: u32,
    /// How much of it is filled now, in milliseconds.
    pub buffered_ms: u32,
    /// Device periods that found the buffer short since the stream was opened.
    pub underruns: u64,
    /// Frames the device has taken since the stream was opened, at its rate.
    pub frames_delivered: u64,
    /// Frames of silence played in place of missing samples since the stream was
    /// opened.
    pub silent_frames: u64,
    /// Underruns of every stream since the engine started.
    pub total_underruns: u64,
}

/// A rate shown in kilohertz: `44.1 kHz`, `48 kHz`.
struct Kilohertz(SampleRate);

impl fmt::Display for Kilohertz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = format!("{:.3}", f64::from(self.0.hz()) / 1000.0);
        write!(
            f,
            "{} kHz",
            text.trim_end_matches('0').trim_end_matches('.')
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{ChannelLayout, DeviceId, Endianness, SampleFormat};

    fn path() -> SignalPath {
        let spec = AudioSpec::new(
            SampleFormat::S32(Endianness::Little),
            SampleRate::new(96_000).unwrap(),
            ChannelLayout::STEREO,
        );
        SignalPath {
            item: ItemId::new(3),
            source: SignalSource {
                spec,
                bits: 24,
                codec: Some("flac".into()),
            },
            stages: vec![
                SignalStage {
                    kind: StageKind::Resampler,
                    state: StageState::Active,
                    detail: "resample 96 → 48 kHz (high)".into(),
                    delay_ms: 0.0,
                },
                SignalStage {
                    kind: StageKind::Equalizer,
                    state: StageState::Off,
                    detail: String::new(),
                    delay_ms: 0.0,
                },
            ],
            device: SignalDevice {
                backend: OutputBackendKind::Alsa,
                device: DeviceSelection::Device(DeviceId::new("hw:CARD=D10s,DEV=0").unwrap()),
                description: None,
                mode: OutputMode::Exclusive,
                format: AudioSpec {
                    rate: SampleRate::HZ_48000,
                    ..spec
                },
                device_rate: None,
                exact_bits: None,
                buffer_ms: 200,
            },
            lamp: SignalLamp::Resampled,
        }
    }

    #[test]
    fn one_line_and_queries() {
        let path = path();
        assert_eq!(
            path.to_string(),
            "flac 96 kHz/24-bit stereo → resample 96 → 48 kHz (high) → \
             alsa hw:CARD=D10s,DEV=0 s32le 48 kHz stereo (exclusive) [resampled]"
        );
        assert!(!path.dsp_active());
        assert_eq!(path.active_stages().count(), 1);
        assert_eq!(
            path.stage(StageKind::Equalizer).map(|s| s.state),
            Some(StageState::Off)
        );
        assert!(SignalLamp::BitPerfect < SignalLamp::Lossy);
    }

    #[test]
    fn json_round_trip() {
        let path = path();
        let json = serde_json::to_string(&path).unwrap();
        assert_eq!(serde_json::from_str::<SignalPath>(&json).unwrap(), path);
    }
}
