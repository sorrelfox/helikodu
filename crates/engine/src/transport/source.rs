// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! One item being played: its decoder and its processing chain.

use helikodu_model::audio::{AudioSpec, MediaTime, SampleRate};
use helikodu_model::player::{EngineError, PlaybackItem};

use crate::decode::{BlockSamples, Decoder, DecoderInfo, DecoderRegistry, PcmBlock};
use crate::output::SampleWriter;
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
    /// The last block as 32-bit integers too, for sources with more bits than a
    /// float carries exactly (see [`render_exact`](Self::render_exact)); otherwise
    /// empty. Valid while it is as long as `decoded`.
    words: Vec<i32>,
    chain: Option<SourceChain>,
    plan: Option<PathPlan>,
    /// The decoder has no more blocks.
    eof: bool,
    /// The chain has given out everything, too.
    finished: bool,
    error: Option<EngineError>,
    /// Nothing has been rendered since the source was opened or moved.
    untouched: bool,
    /// The decoder position the output count starts from, and the frames rendered
    /// since then at the device rate.
    counted_from: u64,
    rendered: u64,
}

impl Source {
    /// Opens `item`.
    pub(super) fn open(
        registry: &DecoderRegistry,
        item: PlaybackItem,
    ) -> Result<Self, EngineError> {
        let decoder = registry.open_stream(&item.source, item.audio_stream)?;
        let (format, length) = Self::format_of(decoder.as_ref());
        Ok(Self {
            item,
            decoder,
            format,
            length,
            block: PcmBlock::new(),
            decoded: Vec::new(),
            consumed: 0,
            words: Vec::new(),
            chain: None,
            plan: None,
            eof: false,
            finished: false,
            error: None,
            untouched: true,
            counted_from: 0,
            rendered: 0,
        })
    }

    /// The decoded format and length of `decoder`.
    fn format_of(decoder: &dyn Decoder) -> (SourceFormat, Option<MediaTime>) {
        let mut format = SourceFormat::new(decoder.spec());
        if let Some(bits) = decoder.info().bits_per_sample {
            format = format.with_bits(bits);
        }
        (format, decoder.duration())
    }

    pub(super) fn item(&self) -> &PlaybackItem {
        &self.item
    }

    /// What the decoder decodes.
    pub(super) fn decoder_info(&self) -> &DecoderInfo {
        self.decoder.info()
    }

    /// Replaces a DoP decoder by the same file converted to PCM, at the same
    /// position, before anything was rendered. Does nothing for other decoders.
    pub(super) fn fall_back_to_pcm(&mut self) -> Result<(), EngineError> {
        let Some(fallback) = self.decoder.pcm_fallback() else {
            return Ok(());
        };
        let decoder = fallback?;
        let (format, length) = Self::format_of(decoder.as_ref());
        self.decoder = decoder;
        self.format = format;
        self.length = length;
        self.decoded.clear();
        self.consumed = 0;
        self.words.clear();
        self.chain = None;
        self.plan = None;
        self.counted_from = self.decoder.position();
        self.rendered = 0;
        Ok(())
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
        self.counted_from = self.decoder.position();
        self.rendered = 0;
        Ok(self.decoder.position())
    }

    /// Builds the processing chain of `plan`, for the device the plan names.
    pub(super) fn attach(&mut self, plan: PathPlan) -> Result<(), EngineError> {
        let chain = SourceChain::new(&plan, MAX_BLOCK).map_err(|error| EngineError::Output {
            reason: error.to_string(),
        })?;
        self.chain = Some(chain);
        self.plan = Some(plan);
        if self.untouched {
            self.counted_from = self.decoder.position();
            self.rendered = 0;
        }
        Ok(())
    }

    /// The frames, at the device rate of the attached plan, that the source has
    /// still to give from now to its end; `None` when its length is not known.
    /// Resampling makes `n` frames `ceil(n × to / from)`, so this is exact for
    /// sources whose length is.
    pub(super) fn remaining_frames(&self) -> Option<u64> {
        let total = self.output_frames(self.counted_from)?;
        Some(total.saturating_sub(self.rendered))
    }

    /// The frames of the whole source at the device rate of the attached plan, or
    /// at the source rate before one is attached.
    pub(super) fn total_frames(&self) -> Option<u64> {
        self.output_frames(0)
    }

    /// The device frames made from the source's frames `from` to its end.
    fn output_frames(&self, from: u64) -> Option<u64> {
        let rate = self.rate();
        let frames = self.length?.frames_at(rate).saturating_sub(from);
        let to = self.plan.as_ref().map_or(rate, |plan| plan.device.rate);
        let total = (u128::from(frames) * u128::from(to.hz())).div_ceil(u128::from(rate.hz()));
        Some(u64::try_from(total).unwrap_or(u64::MAX))
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
                        block_words(&self.block, self.format, &mut self.words);
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
        self.rendered += written as u64;
        written
    }
}

/// The exact path for 32-bit integer sources.
///
/// A float carries 24 bits exactly, so the processing chain cannot pass a 32-bit
/// source unchanged. When the plan changes nothing on the render side (no gain,
/// channel mixing, resampling, crossfade or DSP), the samples go from the decoder
/// into the ring buffer as integers instead, and a ring that carries integers (a
/// 32-bit device format) takes them to the device exactly. Both paths take from the
/// same decoded block, so playback can move between them at any frame.
impl Source {
    /// The decoder's codec and backend, for the signal path.
    pub(super) fn codec(&self) -> (String, &'static str) {
        let info = self.decoder.info();
        (info.codec.clone(), info.backend)
    }

    /// Whether [`render_exact`](Self::render_exact) can take over now.
    pub(super) fn renders_exact(&self) -> bool {
        let plain = self.plan.as_ref().is_some_and(|plan| {
            plan.gain.is_none()
                && plan.channel_mix.is_none()
                && plan.resample.is_none()
                && !plan.crossfade
                && !plan.dsp
        });
        let left = self.consumed < self.decoded.len();
        needs_words(self.format)
            && plain
            && !self.finished
            // What is left of the block is there as integers, or a new block is due.
            && if left {
                self.words.len() == self.decoded.len()
            } else {
                !self.eof
            }
    }

    /// Decodes into `writer` as integers, up to `max_frames` frames or until the
    /// ring is full, and returns the number of frames written. Only while
    /// [`renders_exact`](Self::renders_exact); the end of the source is left to
    /// [`render`](Self::render).
    pub(super) fn render_exact(&mut self, writer: &mut SampleWriter, max_frames: usize) -> usize {
        let channels = usize::from(self.format.spec.channel_count()).max(1);
        let mut written = 0;
        while written < max_frames && !self.finished {
            self.untouched = false;
            if self.consumed < self.decoded.len() {
                if self.words.len() != self.decoded.len() {
                    break;
                }
                let left = (self.decoded.len() - self.consumed) / channels;
                let n = left.min(max_frames - written);
                let made =
                    writer.write_i32(&self.words[self.consumed..self.consumed + n * channels]);
                if made == 0 {
                    break;
                }
                self.consumed += made * channels;
                written += made;
                continue;
            }
            if self.eof || !self.decode_next() {
                break;
            }
        }
        self.rendered += written as u64;
        written
    }

    /// Reads the next block into `decoded` and `words`. Returns whether there is one.
    fn decode_next(&mut self) -> bool {
        match self.decoder.read(&mut self.block) {
            Ok(true) => {
                let spec = self.block.spec();
                if spec.rate != self.format.spec.rate || spec.layout != self.format.spec.layout {
                    self.eof = true;
                    self.error = Some(EngineError::Decode {
                        reason: format!(
                            "the format changed from {} to {spec} midway",
                            self.format.spec
                        ),
                    });
                    return false;
                }
                self.decoded.clear();
                self.block.append_f32(&mut self.decoded);
                block_words(&self.block, self.format, &mut self.words);
                self.consumed = 0;
                true
            }
            Ok(false) => {
                self.eof = true;
                false
            }
            Err(error) => {
                self.eof = true;
                self.error = Some(error.into());
                false
            }
        }
    }
}

/// Whether `format` has more bits than a float carries exactly.
fn needs_words(format: SourceFormat) -> bool {
    format.spec.format.is_integer() && format.significant_bits() > 24
}

/// Keeps `words` in step with the block just decoded: its samples as integers when
/// the source needs them, otherwise nothing.
fn block_words(block: &PcmBlock, format: SourceFormat, words: &mut Vec<i32>) {
    words.clear();
    if needs_words(format)
        && let BlockSamples::S32(samples) = block.samples()
    {
        words.extend_from_slice(samples);
    }
}
