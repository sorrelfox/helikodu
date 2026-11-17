// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The DSF/DFF backend, over the DSD reader of `helikodu-extra-decoders`.
//!
//! DSD files are decoded to `f32` PCM at 176.4 kHz (192 kHz for the 48 kHz
//! family), with the +6 dB that brings the DSD 0 dB reference to 0 dBFS.
//! DST-compressed DFF files are refused here and fall through to FFmpeg.

use std::path::Path;

use helikodu_extra_decoders::dsd::{self, DsdError, DsdPcmDecoder};
use helikodu_model::audio::{AudioSpec, MediaTime};

use super::block::{BlockSamples, PcmBlock};
use super::decoder::{Decoder, DecoderInfo};
use super::error::DecodeError;
use super::factory::{DecoderFactory, Probe, ProbeScore};

const NAME: &str = "dsd";

/// Frames per block.
const BLOCK_FRAMES: usize = 4096;

/// The DSF/DFF backend. It claims files that carry the DSF or DSDIFF signature,
/// whatever their name. It has no subsongs.
#[derive(Debug, Clone, Copy, Default)]
pub struct DsdFactory;

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
        if subsong.is_some() {
            return Err(DecodeError::SubsongUnsupported { backend: NAME });
        }
        let decoder = DsdPcmDecoder::open(path).map_err(|e| match e {
            DsdError::Io(e) => DecodeError::Io(e),
            other => DecodeError::Open {
                backend: NAME,
                reason: other.to_string(),
            },
        })?;
        let info = DecoderInfo {
            backend: NAME,
            container: decoder.info().container.name().to_owned(),
            codec: "dsd".to_owned(),
            bits_per_sample: None,
            tags: Vec::new(),
        };
        Ok(Box::new(DsdDecoder { decoder, info }))
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
