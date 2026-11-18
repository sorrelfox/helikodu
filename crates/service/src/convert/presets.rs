// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Converter presets: a format with its quality, sample rate and channels.
//!
//! The built-in presets have stable identifiers (`opus-128`, `flac`, …); front ends
//! name and describe them in their own language. Presets the user makes have a
//! name of their own and are kept in the [`ExportStore`](super::ExportStore).

use helikodu_engine::convert::{EncodeFormat, EncodeQuality, TranscodeSettings};
use serde::{Deserialize, Serialize};

/// How a preset sets the encoder's quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PresetQuality {
    /// A bit rate in kbit/s.
    Bitrate {
        /// kbit/s.
        kbps: u32,
    },
    /// A LAME VBR level, 0 (best) to 9.
    Vbr {
        /// The level.
        level: u8,
    },
    /// Lossless, with a compression level or the encoder's default.
    Lossless {
        /// The compression level.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        level: Option<u8>,
    },
}

impl PresetQuality {
    /// The quality as the encoder takes it.
    pub fn encode_quality(self) -> EncodeQuality {
        match self {
            Self::Bitrate { kbps } => EncodeQuality::Bitrate(kbps),
            Self::Vbr { level } => EncodeQuality::Vbr(level),
            Self::Lossless { level } => EncodeQuality::Lossless(level),
        }
    }

    /// The bit rate in kbit/s the quality aims at, for estimates; `None` for
    /// lossless.
    pub fn approximate_kbps(self) -> Option<u32> {
        match self {
            Self::Bitrate { kbps } => Some(kbps),
            Self::Vbr { level } => Some(match level {
                0 => 245,
                1 => 225,
                2 => 190,
                3 => 175,
                4 => 165,
                5 => 130,
                6 => 115,
                7 => 100,
                _ => 85,
            }),
            Self::Lossless { .. } => None,
        }
    }
}

/// A converter preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    /// A stable identifier: the built-in ones' names, `own-N` for the user's.
    pub id: String,
    /// The user's name for the preset; empty for built-in ones, which front ends
    /// name themselves.
    #[serde(default)]
    pub name: String,
    /// The format, by [`EncodeFormat::id`].
    pub format: String,
    /// The quality.
    pub quality: PresetQuality,
    /// The sample rate in Hz; `None` keeps the source's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    /// The channels; `None` keeps the source's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
    /// Bits per sample of lossless formats; `None` keeps the source's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bits: Option<u8>,
}

impl Preset {
    fn builtin(id: &str, format: EncodeFormat, quality: PresetQuality) -> Self {
        Self {
            id: id.to_owned(),
            name: String::new(),
            format: format.id().to_owned(),
            quality,
            sample_rate: None,
            channels: None,
            bits: None,
        }
    }

    /// The built-in presets, in the order the converter lists them.
    pub fn builtins() -> Vec<Self> {
        let mut wav = Self::builtin(
            "wav-cd",
            EncodeFormat::Wav,
            PresetQuality::Lossless { level: None },
        );
        wav.sample_rate = Some(44_100);
        wav.bits = Some(16);
        wav.channels = Some(2);
        vec![
            Self::builtin(
                "opus-128",
                EncodeFormat::Opus,
                PresetQuality::Bitrate { kbps: 128 },
            ),
            Self::builtin(
                "opus-192",
                EncodeFormat::Opus,
                PresetQuality::Bitrate { kbps: 192 },
            ),
            Self::builtin("mp3-v0", EncodeFormat::Mp3, PresetQuality::Vbr { level: 0 }),
            Self::builtin(
                "aac-256",
                EncodeFormat::Aac,
                PresetQuality::Bitrate { kbps: 256 },
            ),
            Self::builtin(
                "flac",
                EncodeFormat::Flac,
                PresetQuality::Lossless { level: Some(5) },
            ),
            Self::builtin(
                "alac",
                EncodeFormat::Alac,
                PresetQuality::Lossless { level: None },
            ),
            wav,
        ]
    }

    /// Whether the preset is one of the built-in ones.
    pub fn is_builtin(&self) -> bool {
        !self.id.starts_with("own-")
    }

    /// The format; FLAC when the stored identifier is unknown.
    pub fn encode_format(&self) -> EncodeFormat {
        EncodeFormat::from_id(&self.format).unwrap_or(EncodeFormat::Flac)
    }

    /// The settings a conversion with this preset uses.
    pub fn settings(&self) -> TranscodeSettings {
        TranscodeSettings {
            format: self.encode_format(),
            quality: self.quality.encode_quality(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            bits: self.bits,
        }
    }

    /// The expected size in bytes of `seconds` of audio with `channels` channels
    /// at `rate` and `bits`: from the bit rate for lossy formats, about 60 % of
    /// PCM for compressed lossless ones.
    pub fn expected_bytes(&self, seconds: f64, rate: u32, channels: u16, bits: u32) -> u64 {
        let channels = f64::from(
            self.channels
                .unwrap_or(channels)
                .min(self.encode_format().max_channels()),
        );
        let bytes = match self.quality.approximate_kbps() {
            Some(kbps) => f64::from(kbps) * 1000.0 / 8.0 * seconds,
            None => {
                let rate = f64::from(self.sample_rate.unwrap_or(rate));
                let bits = f64::from(self.bits.map_or(bits.clamp(16, 24), u32::from));
                let pcm = rate * channels * bits / 8.0 * seconds;
                if self.encode_format() == EncodeFormat::Wav {
                    pcm
                } else {
                    pcm * 0.6
                }
            }
        };
        bytes.max(0.0) as u64
    }

    /// Whether a source in `codec` with `kbps` (for lossy sources) can be copied
    /// as it is instead of converted: it is already in the preset's format, at no
    /// more than its bit rate, and needs no change of rate or channels.
    pub fn takes_as_is(&self, codec: &str, kbps: Option<u32>, rate: Option<u32>) -> bool {
        let format = self.encode_format();
        if !format.codec_names().contains(&codec) {
            return false;
        }
        if self.sample_rate.is_some_and(|r| Some(r) != rate) || self.channels.is_some() {
            return false;
        }
        match (self.quality.approximate_kbps(), kbps) {
            (None, _) => format != EncodeFormat::Wav || self.bits.is_none(),
            (Some(target), Some(source)) => source <= target + target / 10,
            (Some(_), None) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_round_trip_through_toml() {
        #[derive(Serialize, Deserialize)]
        struct File {
            preset: Vec<Preset>,
        }
        let file = File {
            preset: Preset::builtins(),
        };
        let text = toml::to_string(&file).unwrap();
        let back: File = toml::from_str(&text).unwrap();
        assert_eq!(back.preset, Preset::builtins());
        assert!(back.preset.iter().all(Preset::is_builtin));
    }

    #[test]
    fn copies_only_what_needs_no_conversion() {
        let presets = Preset::builtins();
        let opus = &presets[0];
        assert!(opus.takes_as_is("opus", Some(128), Some(48_000)));
        assert!(!opus.takes_as_is("opus", Some(256), Some(48_000)));
        assert!(!opus.takes_as_is("flac", None, Some(44_100)));
        let flac = &presets[4];
        assert!(flac.takes_as_is("flac", None, Some(96_000)));
        let wav = &presets[6];
        assert!(!wav.takes_as_is("pcm_s16le", None, Some(48_000)));
    }

    #[test]
    fn estimates() {
        let presets = Preset::builtins();
        // 128 kbit/s for a minute is 960 kB.
        assert_eq!(presets[0].expected_bytes(60.0, 44_100, 2, 16), 960_000);
        let wav = &presets[6];
        assert_eq!(wav.expected_bytes(1.0, 96_000, 2, 24), 176_400);
    }
}
