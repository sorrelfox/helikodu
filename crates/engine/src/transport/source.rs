// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! One item being played: its decoder and its processing chain.

use helikodu_model::audio::{AudioSpec, MediaTime, SampleRate};
use helikodu_model::player::{EngineError, PlaybackItem};

use crate::decode::{Decoder, DecoderRegistry, PcmBlock};
use crate::pipeline::{PathPlan, SourceChain, SourceFormat};

/// The most input frames the processing chain takes at once.
const MAX_BLOCK: usize = 4096;

/// An item's decoder, with the chain that brings its audio to the device format.
#[derive(Debug)]
pub(super) struct Source {
    item: PlaybackItem,
    decoder: Box<dyn Decoder>,
    format: SourceFormat,
    length: Option<MediaTime>,
    block: PcmBlock,
    /// The last block as `f32`, and how much of it the chain has taken.
    decoded: Vec<f32>,
    consumed: usize,
    chain: Option<SourceChain>,
    plan: Option<PathPlan>,
    /// The decoder has no more blocks.
    eof: bool,
    /// The chain has given out everything, too.
    finished: bool,
    error: Option<EngineError>,
    /// Nothing has been rendered since the source was opened or moved.
    untouched: bool,
}

impl Source {
    /// Opens `item`.
    pub(super) fn open(
        registry: &DecoderRegistry,
        item: PlaybackItem,
    ) -> Result<Self, EngineError> {
        let decoder = registry.open(&item.source)?;
        let spec = decoder.spec();
        let mut format = SourceFormat::new(spec);
        if let Some(bits) = decoder.info().bits_per_sample {
            format = format.with_bits(bits);
        }
        let length = decoder.duration();
        Ok(Self {
            item,
            decoder,
            format,
            length,
            block: PcmBlock::new(),
            decoded: Vec::new(),
            consumed: 0,
            chain: None,
            plan: None,
            eof: false,
            finished: false,
            error: None,
            untouched: true,
        })
    }

    pub(super) fn item(&self) -> &PlaybackItem {
        &self.item
    }

    /// The decoded format.
    pub(super) fn spec(&self) -> AudioSpec {
        self.format.spec
    }

    pub(super) fn format(&self) -> SourceFormat {
        self.format
    }

    pub(super) fn rate(&self) -> SampleRate {
        self.format.spec.rate
    }

    pub(super) fn length(&self) -> Option<MediaTime> {
        self.length
    }

    /// The position of the next frame the decoder gives, in frames at the source
    /// rate.
    pub(super) fn position(&self) -> u64 {
        self.decoder.position()
    }

    /// The plan the source is processed with, once attached.
    pub(super) fn plan(&self) -> Option<&PathPlan> {
        self.plan.as_ref()
    }

    /// Moves to `to`, exactly, and returns the position in frames. The chain is
    /// cleared; a source that had ended plays again.
    pub(super) fn seek(&mut self, to: MediaTime) -> Result<u64, EngineError> {
        let frames = to.frames_at(self.rate());
        if self.untouched && self.decoder.position() == frames {
            return Ok(frames);
        }
        self.decoder.seek(to)?;
        self.decoded.clear();
        self.consumed = 0;
        self.eof = false;
        self.finished = false;
        self.error = None;
        self.untouched = true;
        if let Some(chain) = &mut self.chain {
            chain.reset();
        }
        Ok(self.decoder.position())
    }

    /// Builds the processing chain of `plan`, for the device the plan names.
    pub(super) fn attach(&mut self, plan: PathPlan) -> Result<(), EngineError> {
        let chain = SourceChain::new(&plan, MAX_BLOCK).map_err(|error| EngineError::Output {
            reason: error.to_string(),
        })?;
        self.chain = Some(chain);
        self.plan = Some(plan);
        Ok(())
    }

    /// Moves to the gain of `plan`, a plan for the same source and device that may
    /// differ from the attached one in its gain only, ramping to it on the audio
    /// rendered from now on. Does nothing before [`attach`](Self::attach).
    pub(super) fn retune(&mut self, plan: PathPlan) {
        let Some(chain) = &mut self.chain else {
            return;
        };
        chain.set_gain(plan.gain.map_or(1.0, |gain| gain.linear()));
        self.plan = Some(plan);
    }

    /// The output room, in frames, that [`render`](Self::render) needs to make
    /// progress.
    pub(super) fn max_output_frames(&self) -> usize {
        self.chain
            .as_ref()
            .map_or(MAX_BLOCK, SourceChain::max_output_frames)
    }

    /// Whether everything has been rendered, to the end or to an error.
    pub(super) fn is_finished(&self) -> bool {
        self.finished
    }

    /// The error that ended the source early, if one did.
    pub(super) fn take_error(&mut self) -> Option<EngineError> {
        self.error.take()
    }

    /// Decodes and processes frames into `out` (interleaved, in the device's rate
    /// and layout) until it is full or the source ends. Returns the number of frames
    /// written. Needs [`attach`](Self::attach) first.
    pub(super) fn render(&mut self, out: &mut [f32]) -> usize {
        let Some(chain) = &mut self.chain else {
            return 0;
        };
        let in_channels = chain.input_channels();
        let out_channels = chain.output_channels();
        let room = out.len() / out_channels;
        let mut written = 0;
        while written < room && !self.finished {
            self.untouched = false;
            if self.consumed < self.decoded.len() {
                let (taken, made) = chain.process(
                    &self.decoded[self.consumed..],
                    &mut out[written * out_channels..room * out_channels],
                );
                if taken == 0 && made == 0 {
                    break;
                }
                self.consumed += taken * in_channels;
                written += made;
                continue;
            }
            if !self.eof {
                match self.decoder.read(&mut self.block) {
                    Ok(true) => {
                        let spec = self.block.spec();
                        if spec.rate != self.format.spec.rate
                            || spec.layout != self.format.spec.layout
                        {
                            self.eof = true;
                            self.error = Some(EngineError::Decode {
                                reason: format!(
                                    "the format changed from {} to {spec} midway",
                                    self.format.spec
                                ),
                            });
                            continue;
                        }
                        self.decoded.clear();
                        self.block.append_f32(&mut self.decoded);
                        self.consumed = 0;
                    }
                    Ok(false) => self.eof = true,
                    Err(error) => {
                        self.eof = true;
                        self.error = Some(error.into());
                    }
                }
                continue;
            }
            // The decoder is done: empty the chain (the resampler's tail).
            if room - written < chain.max_output_frames() {
                break;
            }
            let made = chain.finish(&mut out[written * out_channels..room * out_channels]);
            if made == 0 {
                self.finished = true;
            }
            written += made;
        }
        written
    }
}
