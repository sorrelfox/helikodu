// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding a whole file through the engine's decoder registry.

use std::path::Path;

use helikodu_engine::decode::{DecodeError, Decoder, DecoderRegistry, PcmBlock};
use helikodu_model::library::MediaSource;

/// A file decoded to its end.
#[derive(Debug)]
pub(crate) struct Decoded {
    pub(crate) codec: String,
    pub(crate) rate: u32,
    pub(crate) channels: usize,
    /// Interleaved samples as `f32` (exact for 8-, 16- and 24-bit integers).
    pub(crate) samples: Vec<f32>,
}

impl Decoded {
    pub(crate) fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }
}

/// Opens `path` with the default registry.
pub(crate) fn open(path: &Path) -> Result<Box<dyn Decoder>, DecodeError> {
    DecoderRegistry::default().open(&MediaSource::file(path))
}

/// Decodes the rest of `decoder` into `out`.
pub(crate) fn read_to_end(
    decoder: &mut dyn Decoder,
    out: &mut Vec<f32>,
) -> Result<(), DecodeError> {
    let mut block = PcmBlock::new();
    while decoder.read(&mut block)? {
        block.append_f32(out);
    }
    Ok(())
}

/// Opens and decodes `path` to its end.
pub(crate) fn decode(path: &Path) -> Result<Decoded, DecodeError> {
    let mut decoder = open(path)?;
    let spec = decoder.spec();
    let mut samples = Vec::new();
    read_to_end(decoder.as_mut(), &mut samples)?;
    Ok(Decoded {
        codec: decoder.info().codec.clone(),
        rate: spec.rate.hz(),
        channels: usize::from(spec.channel_count()),
        samples,
    })
}
