// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSF/DFF backend, over the DSD reader of `helikodu-extra-decoders`.
//!
//! DSD files are decoded to `f32` PCM at 176.4 kHz (192 kHz for the 48 kHz
//! family), with the +6 dB that brings the DSD 0 dB reference to 0 dBFS. When the
//! options ask for DoP ([`DecodeOptions::dop`](super::DecodeOptions::dop)), they are
//! packed into DSD over PCM instead: 24-bit words (left-justified in `i32`) at the
//! DSD rate divided by 16, which the transport plays only on a path that leaves
//! them untouched and otherwise replaces by PCM ([`Decoder::pcm_fallback`]).
//! DST-compressed DFF files are refused here and fall through to FFmpeg.

use std::path::{Path, PathBuf};

use helikodu_extra_decoders::dsd::{self, DopDecoder, DsdError, DsdPcmDecoder};
use helikodu_model::audio::{
    AudioSpec, ChannelLayout, Endianness, MediaTime, SampleFormat, SampleRate,
};

use super::block::{BlockSamples, PcmBlock};
use super::decoder::{Decoder, DecoderInfo};
use super::error::DecodeError;
use super::factory::{DecoderFactory, OpenRequest, Probe, ProbeScore, SourceInput};
use super::options::SharedOptions;

const NAME: &str = "dsd";

/// Frames per block.
const BLOCK_FRAMES: usize = 4096;

/// The DSF/DFF backend. It claims files that carry the DSF or DSDIFF signature,
/// whatever their name. It has no subsongs. DSD files are always read from the
/// drive, also with whole-file buffering on.
#[derive(Debug, Clone, Default)]
pub struct DsdFactory {
    options: SharedOptions,
}

impl DsdFactory {
    /// The backend, opening DSD files as DoP when `options` say so.
    pub fn new(options: SharedOptions) -> Self {
        Self { options }
    }
}

/// An error while opening.
fn open_error(error: DsdError) -> DecodeError {
    match error {
        DsdError::Io(e) => DecodeError::Io(e),
        other => DecodeError::Open {
            backend: NAME,
            reason: other.to_string(),
        },
    }
}

/// Opens `path` converted to PCM.
fn open_pcm(path: &Path) -> Result<DsdDecoder, DecodeError> {
    let decoder = DsdPcmDecoder::open(path).map_err(open_error)?;
    let info = DecoderInfo {
        backend: NAME,
        container: decoder.info().container.name().to_owned(),
        codec: "dsd".to_owned(),
        ..DecoderInfo::default()
    };
    Ok(DsdDecoder { decoder, info })
}

impl DecoderFactory for DsdFactory {
    fn name(&self) -> &'static str {
        NAME
    }

    fn probe(&self, probe: &Probe<'_>) -> ProbeScore {
        if dsd::sniff(probe.header()).is_some() {
            ProbeScore::SIGNATURE
        } else {
            ProbeScore::NONE
        }
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
        if self.options.get().dop {
            return Ok(Box::new(DopSource::open(input.path())?));
        }
        Ok(Box::new(open_pcm(input.path())?))
    }
}

/// A DSD file packed into DoP frames.
#[derive(Debug)]
struct DopSource {
    decoder: DopDecoder,
    path: PathBuf,
    info: DecoderInfo,
    rate: SampleRate,
    layout: ChannelLayout,
}

impl DopSource {
    fn open(path: &Path) -> Result<Self, DecodeError> {
        let decoder = DopDecoder::open(path).map_err(open_error)?;
        let rate = SampleRate::new(decoder.rate()).ok_or_else(|| DecodeError::Open {
            backend: NAME,
            reason: "the DSD rate is not usable".into(),
        })?;
        let layout = decoder.info().layout;
        let info = DecoderInfo {
            backend: NAME,
            container: decoder.info().container.name().to_owned(),
            codec: "dsd".to_owned(),
            bits_per_sample: Some(24),
            dop: true,
            ..DecoderInfo::default()
        };
        Ok(Self {
            decoder,
            path: path.to_owned(),
            info,
            rate,
            layout,
        })
    }
}

impl Decoder for DopSource {
    fn info(&self) -> &DecoderInfo {
        &self.info
    }

    fn spec(&self) -> AudioSpec {
        AudioSpec::new(
            SampleFormat::S32(Endianness::NATIVE),
            self.rate,
            self.layout,
        )
    }

    fn duration(&self) -> Option<MediaTime> {
        Some(MediaTime::from_frames(self.decoder.frames(), self.rate))
    }

    fn position(&self) -> u64 {
        self.decoder.position()
    }

    fn read(&mut self, block: &mut PcmBlock) -> Result<bool, DecodeError> {
        let mut samples = match block.take_samples() {
            BlockSamples::S32(v) => v,
            _ => Vec::new(),
        };
        let position = self.decoder.position();
        let frames =
            self.decoder
                .read(BLOCK_FRAMES, &mut samples)
                .map_err(|e| DecodeError::Decode {
                    backend: NAME,
                    reason: e.to_string(),
                })?;
        block.set(self.rate, self.layout, position, BlockSamples::S32(samples));
        Ok(frames > 0)
    }

    fn seek(&mut self, to: MediaTime) -> Result<(), DecodeError> {
        self.decoder.seek(to.frames_at(self.rate));
        Ok(())
    }

    fn pcm_fallback(&self) -> Option<Result<Box<dyn Decoder>, DecodeError>> {
        let position = MediaTime::from_frames(self.decoder.position(), self.rate);
        Some(open_pcm(&self.path).and_then(|mut decoder| {
            decoder.seek(position)?;
            Ok(Box::new(decoder) as Box<dyn Decoder>)
        }))
    }
}

/// A DSD file decoded to PCM.
#[derive(Debug)]
struct DsdDecoder {
    decoder: DsdPcmDecoder,
    info: DecoderInfo,
}

impl Decoder for DsdDecoder {
    fn info(&self) -> &DecoderInfo {
        &self.info
    }

    fn spec(&self) -> AudioSpec {
        self.decoder.spec()
    }

    fn duration(&self) -> Option<MediaTime> {
        Some(MediaTime::from_frames(
            self.decoder.frames(),
            self.decoder.spec().rate,
        ))
    }

    fn position(&self) -> u64 {
        self.decoder.position()
    }

    fn read(&mut self, block: &mut PcmBlock) -> Result<bool, DecodeError> {
        let mut samples = match block.take_samples() {
            BlockSamples::F32(v) => v,
            _ => Vec::new(),
        };
        let position = self.decoder.position();
        let frames = self
            .decoder
            .read(BLOCK_FRAMES, &mut samples)
            .map_err(|e| match e {
                DsdError::Io(e) => DecodeError::Decode {
                    backend: NAME,
                    reason: format!("cannot read the file: {e}"),
                },
                other => DecodeError::Decode {
                    backend: NAME,
                    reason: other.to_string(),
                },
            })?;
        let spec = self.decoder.spec();
        block.set(spec.rate, spec.layout, position, BlockSamples::F32(samples));
        Ok(frames > 0)
    }

    fn seek(&mut self, to: MediaTime) -> Result<(), DecodeError> {
        self.decoder.seek(to.frames_at(self.decoder.spec().rate));
        Ok(())
    }
}
