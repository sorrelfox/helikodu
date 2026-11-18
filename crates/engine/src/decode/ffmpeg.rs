// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The FFmpeg backend, over `helikodu-ffmpeg-bridge`.
//!
//! The bridge opens the file with every offline lock, probes it by content, picks
//! the audio stream (the one marked default, otherwise FFmpeg's best), trims encoder
//! delay and padding and seeks to the exact frame. This backend only copies its
//! blocks into [`PcmBlock`]s, keeping integer samples as integers.

use std::path::Path;

use helikodu_ffmpeg_bridge::{
    AudioDecoder, Error, MediaFile, PcmBuffer, PcmSamples, StreamChoice, StreamInfo,
};
use helikodu_model::audio::{AudioSpec, MediaTime};
use helikodu_model::player::AudioStreamInfo;

use super::block::{BlockSamples, PcmBlock};
use super::decoder::{Decoder, DecoderInfo};
use super::error::DecodeError;
use super::factory::{DecoderFactory, OpenRequest, Probe, ProbeScore, SourceInput};

const NAME: &str = "ffmpeg";

/// The FFmpeg backend. It recognises files by their content when it opens them, so
/// it probes every file with [`ProbeScore::FALLBACK`] and takes whatever no
/// dedicated backend claims. It has no subsongs; of a file with several audio
/// streams it decodes the one asked for ([`OpenRequest::audio_stream`]), otherwise
/// the default one, and lists them all in [`DecoderInfo::streams`].
#[derive(Debug, Clone, Copy, Default)]
pub struct FfmpegFactory;

impl DecoderFactory for FfmpegFactory {
    fn name(&self) -> &'static str {
        NAME
    }

    fn probe(&self, _probe: &Probe<'_>) -> ProbeScore {
        ProbeScore::FALLBACK
    }

    fn open(&self, path: &Path, subsong: Option<u32>) -> Result<Box<dyn Decoder>, DecodeError> {
        self.open_input(
            &SourceInput::file(path),
            &OpenRequest {
                subsong,
                audio_stream: None,
            },
        )
    }

    fn open_input(
        &self,
        input: &SourceInput<'_>,
        request: &OpenRequest,
    ) -> Result<Box<dyn Decoder>, DecodeError> {
        if request.subsong.is_some() {
            return Err(DecodeError::SubsongUnsupported { backend: NAME });
        }
        let media = match input.data() {
            Some(data) => MediaFile::open_bytes(data.clone()),
            None => MediaFile::open(input.path()),
        }
        .map_err(open_error)?;
        let streams: Vec<AudioStreamInfo> = media.audio_streams().map(stream_info).collect();
        let choice = request.audio_stream.map_or(StreamChoice::Auto, |index| {
            StreamChoice::Index(index as usize)
        });
        let decoder = media.into_decoder(choice).map_err(open_error)?;
        let mut decoder = FfmpegDecoder::new(decoder);
        decoder.info.stream = u32::try_from(decoder.decoder.stream_index()).ok();
        if streams.len() > 1 {
            decoder.info.streams = streams;
        }
        decoder.info.in_memory = input.data().is_some();
        Ok(Box::new(decoder))
    }
}

/// The stream selector's description of an audio stream.
fn stream_info(stream: &StreamInfo) -> AudioStreamInfo {
    AudioStreamInfo {
        index: u32::try_from(stream.index).unwrap_or(u32::MAX),
        codec: stream.codec.clone(),
        language: stream.language.clone(),
        title: stream.title.clone(),
        channels: stream.channels,
        sample_rate: stream.sample_rate,
        default: stream.disposition.is_default(),
    }
}

/// An error while opening.
fn open_error(error: Error) -> DecodeError {
    match error {
        Error::Io(e) => DecodeError::Io(e),
        other => DecodeError::Open {
            backend: NAME,
            reason: other.to_string(),
        },
    }
}

/// An error after opening.
fn decode_error(error: &Error) -> DecodeError {
    DecodeError::Decode {
        backend: NAME,
        reason: error.to_string(),
    }
}

/// One audio stream decoded by FFmpeg.
#[derive(Debug)]
struct FfmpegDecoder {
    decoder: AudioDecoder,
    info: DecoderInfo,
}

impl FfmpegDecoder {
    fn new(decoder: AudioDecoder) -> Self {
        let stream = decoder.stream();
        let info = DecoderInfo {
            backend: NAME,
            container: decoder.media().format_name().to_owned(),
            codec: stream.codec.clone(),
            bits_per_sample: stream.bits_per_sample,
            tags: decoder
                .media()
                .tags_with_stream(decoder.stream_index())
                .iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            ..DecoderInfo::default()
        };
        Self { decoder, info }
    }
}

/// Copies a bridge block into `block`, reusing its allocation.
fn copy_block(from: &PcmBuffer, block: &mut PcmBlock) {
    let samples = match (from.samples(), block.take_samples()) {
        (PcmSamples::U8(src), BlockSamples::U8(mut v)) => {
            v.clear();
            v.extend_from_slice(src);
            BlockSamples::U8(v)
        }
        (PcmSamples::S16(src), BlockSamples::S16(mut v)) => {
            v.clear();
            v.extend_from_slice(src);
            BlockSamples::S16(v)
        }
        (PcmSamples::S32(src), BlockSamples::S32(mut v)) => {
            v.clear();
            v.extend_from_slice(src);
            BlockSamples::S32(v)
        }
        (PcmSamples::F32(src), BlockSamples::F32(mut v)) => {
            v.clear();
            v.extend_from_slice(src);
            BlockSamples::F32(v)
        }
        (PcmSamples::U8(src), _) => BlockSamples::U8(src.clone()),
        (PcmSamples::S16(src), _) => BlockSamples::S16(src.clone()),
        (PcmSamples::S32(src), _) => BlockSamples::S32(src.clone()),
        (PcmSamples::F32(src), _) => BlockSamples::F32(src.clone()),
    };
    let spec = from.spec();
    block.set(spec.rate, spec.layout, from.position(), samples);
}

impl Decoder for FfmpegDecoder {
    fn info(&self) -> &DecoderInfo {
        &self.info
    }

    fn spec(&self) -> AudioSpec {
        self.decoder.spec()
    }

    fn duration(&self) -> Option<MediaTime> {
        self.decoder.duration()
    }

    fn position(&self) -> u64 {
        self.decoder.position()
    }

    fn read(&mut self, block: &mut PcmBlock) -> Result<bool, DecodeError> {
        match self.decoder.read() {
            Ok(Some(buffer)) => {
                copy_block(buffer, block);
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(e) => Err(decode_error(&e)),
        }
    }

    fn seek(&mut self, to: MediaTime) -> Result<(), DecodeError> {
        self.decoder.seek(to).map_err(|e| decode_error(&e))
    }

    fn skipped_errors(&self) -> (u64, Option<u64>) {
        self.decoder.errors_skipped()
    }
}
