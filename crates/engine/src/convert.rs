// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Converting a source into another format, for the converter and folder sync.
//!
//! [`transcode`] decodes a source through the [`DecoderRegistry`] (so every format
//! the player plays can be converted, sections of CUE sheets and DSD included),
//! maps the channels with a [`MixMatrix`] when the target has fewer, resamples with
//! the playback resampler when the target rate differs, and encodes through the
//! FFmpeg bridge into a file the caller opened. Lossless targets fed by integer
//! sources at their own rate and channels get the exact sample values.
//!
//! Encoding is as offline as decoding: the bridge writes through custom I/O to the
//! open file, with every nested open refused.

use std::fs::File;

use helikodu_ffmpeg_bridge::{AudioEncoder, EncoderConfig, nearest_rate};
pub use helikodu_ffmpeg_bridge::{
    EncodeFormat, EncodeQuality, EncoderAvailability, encoder_availability, supported_sample_rates,
};
use helikodu_model::audio::{ChannelLayout, SampleRate};
use helikodu_model::library::MediaSource;

use crate::decode::{BlockSamples, DecodeError, DecoderRegistry, PcmBlock};
use crate::pipeline::{MixMatrix, MixOptions, PipelineError, ResampleStage, ResamplerQuality};

/// The format a source is converted to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TranscodeSettings {
    /// The target format.
    pub format: EncodeFormat,
    /// How hard the encoder works.
    pub quality: EncodeQuality,
    /// The sample rate; `None` keeps the source's. Either way the nearest rate the
    /// encoder takes is used (48 kHz for Opus).
    pub sample_rate: Option<u32>,
    /// The number of channels; `None` keeps the source's, up to the most the
    /// format takes. Fewer channels are a downmix.
    pub channels: Option<u16>,
    /// For lossless targets, the bits per sample (16 or 24); `None` keeps the
    /// source's (16 for sources of up to 16 bits, otherwise 24).
    pub bits: Option<u8>,
}

/// What a conversion wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscodeReport {
    /// The size of the file in bytes.
    pub bytes: u64,
    /// The frames encoded.
    pub frames: u64,
    /// The sample rate written.
    pub sample_rate: u32,
    /// The channels written.
    pub channels: u16,
    /// The encoder used, by FFmpeg's name.
    pub encoder: String,
}

/// Why a conversion failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TranscodeError {
    /// The source could not be opened or decoded.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// The encoder failed or does not exist in this FFmpeg build.
    #[error(transparent)]
    Encode(#[from] helikodu_ffmpeg_bridge::Error),
    /// The resampler or the channel mapping refused the audio.
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
    /// The source changed its sample rate or channels part of the way.
    #[error("the audio format changes inside the file")]
    FormatChanged,
    /// The source holds no audio.
    #[error("the source holds no audio")]
    Empty,
    /// The conversion was stopped by its caller.
    #[error("the conversion was cancelled")]
    Cancelled,
}

/// The bits per sample a lossless target gets for a source with `bits`.
fn lossless_bits(wanted: Option<u8>, bits: Option<u32>) -> u8 {
    match wanted {
        Some(b) if b > 16 => 24,
        Some(_) => 16,
        None if bits.is_some_and(|b| b <= 16) => 16,
        None => 24,
    }
}

/// Converts `source` into `output` with `settings`.
///
/// `progress` is called after each block with the frames of the source decoded
/// so far and its length in frames, when known. `cancelled` is asked between
/// blocks. On an error the output holds an incomplete file, which the caller
/// removes.
pub fn transcode(
    registry: &DecoderRegistry,
    source: &MediaSource,
    output: File,
    settings: &TranscodeSettings,
    progress: &mut dyn FnMut(u64, Option<u64>),
    cancelled: &dyn Fn() -> bool,
) -> Result<TranscodeReport, TranscodeError> {
    let mut decoder = registry.open(source)?;
    let spec = decoder.spec();
    let total = decoder.duration().map(|d| d.frames_at(spec.rate));
    let source_bits = decoder.info().bits_per_sample;
    let format = settings.format;

    let in_channels = spec.layout.channel_count();
    let out_channels = settings
        .channels
        .unwrap_or(in_channels)
        .clamp(1, format.max_channels());
    let rates = supported_sample_rates(format);
    let out_rate = nearest_rate(
        rates.as_deref(),
        settings.sample_rate.unwrap_or(spec.rate.hz()),
    );
    let bits = if format.is_lossless() {
        lossless_bits(settings.bits, source_bits)
    } else {
        16
    };
    let config = EncoderConfig {
        format,
        sample_rate: out_rate,
        channels: out_channels,
        bits,
        quality: settings.quality,
    };
    let mut encoder = AudioEncoder::create(output, &config)?;
    let encoder_name = encoder.encoder_name().to_owned();

    let out_layout = ChannelLayout::default_for_count(out_channels)
        .or_else(|| ChannelLayout::unspecified(out_channels))
        .ok_or(PipelineError::InvalidShape("no channels"))?;
    let mix = (out_channels != in_channels)
        .then(|| MixMatrix::new(spec.layout, out_layout, MixOptions::default()));
    let target_rate =
        SampleRate::new(out_rate).ok_or(PipelineError::InvalidShape("no sample rate"))?;
    let mut resampler = if target_rate == spec.rate {
        None
    } else {
        Some(ResampleStage::new(
            spec.rate,
            target_rate,
            usize::from(out_channels),
            ResamplerQuality::High,
        )?)
    };
    // Lossless output of integer audio that needs no processing stays exact.
    let exact = format.is_lossless() && mix.is_none() && resampler.is_none();

    let mut block = PcmBlock::new();
    let mut floats: Vec<f32> = Vec::new();
    let mut mixed: Vec<f32> = Vec::new();
    let mut resampled: Vec<f32> = Vec::new();
    let mut ints: Vec<i32> = Vec::new();
    let mut decoded: u64 = 0;
    let out_ch = usize::from(out_channels);
    loop {
        if cancelled() {
            return Err(TranscodeError::Cancelled);
        }
        if !decoder.read(&mut block)? {
            break;
        }
        if block.is_empty() {
            continue;
        }
        let block_spec = block.spec();
        if block_spec.rate != spec.rate || block_spec.layout.channel_count() != in_channels {
            return Err(TranscodeError::FormatChanged);
        }
        decoded += block.frames() as u64;
        if exact && let Some(()) = integer_samples(block.samples(), &mut ints) {
            encoder.write_i32(&ints)?;
            progress(decoded, total);
            continue;
        }
        floats.clear();
        block.append_f32(&mut floats);
        let mapped: &[f32] = match &mix {
            Some(matrix) => {
                mixed.clear();
                mixed.resize(block.frames() * out_ch, 0.0);
                let n = matrix.apply(&floats, &mut mixed);
                &mixed[..n * out_ch]
            }
            None => &floats,
        };
        match resampler.as_mut() {
            Some(stage) => {
                let frames = mapped.len() / out_ch;
                resampled.clear();
                resampled.resize(stage.max_output_frames(frames) * out_ch, 0.0);
                let (_, written) = stage.process(mapped, &mut resampled);
                encoder.write_f32(&resampled[..written * out_ch])?;
            }
            None => encoder.write_f32(mapped)?,
        }
        progress(decoded, total);
    }
    if decoded == 0 {
        return Err(TranscodeError::Empty);
    }
    let mut frames_out = decoded;
    if let Some(stage) = resampler.as_mut() {
        resampled.clear();
        resampled.resize(stage.max_output_frames(1).max(1024) * out_ch, 0.0);
        loop {
            let written = stage.finish(&mut resampled);
            if written == 0 {
                break;
            }
            encoder.write_f32(&resampled[..written * out_ch])?;
        }
        frames_out = u64::try_from(
            (u128::from(decoded) * u128::from(out_rate)).div_ceil(u128::from(spec.rate.hz())),
        )
        .unwrap_or(u64::MAX);
    }
    let bytes = encoder.finish()?;
    Ok(TranscodeReport {
        bytes,
        frames: frames_out,
        sample_rate: out_rate,
        channels: out_channels,
        encoder: encoder_name,
    })
}

/// The samples of an integer block as full-scale 32-bit integers into `out`;
/// `None` for float samples.
fn integer_samples(samples: &BlockSamples, out: &mut Vec<i32>) -> Option<()> {
    out.clear();
    match samples {
        BlockSamples::U8(v) => out.extend(v.iter().map(|&x| (i32::from(x) - 128) << 24)),
        BlockSamples::S16(v) => out.extend(v.iter().map(|&x| i32::from(x) << 16)),
        BlockSamples::S32(v) => out.extend_from_slice(v),
        BlockSamples::F32(_) => return None,
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lossless_depth_follows_the_source() {
        assert_eq!(lossless_bits(None, Some(16)), 16);
        assert_eq!(lossless_bits(None, Some(24)), 24);
        assert_eq!(lossless_bits(None, None), 24);
        assert_eq!(lossless_bits(Some(16), Some(24)), 16);
    }
}
