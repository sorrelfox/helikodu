// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! One item being played: its decoder and its processing chain.

use helikodu_model::audio::{AudioSpec, MediaTime, SampleRate};
use helikodu_model::player::{EngineError, LoopRange, PlaybackItem};

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
    /// The A-B loop, when one is set.
    looping: Option<Loop>,
    /// Where the last render call moved back to the loop's start, or jumped over
    /// silence: the output frames written by then, and the source frame it went on
    /// at.
    wraps: Vec<(usize, u64)>,
    /// The source frame where the source ends early (its trailing silence left
    /// out), if it does.
    end: Option<u64>,
    /// Silent stretches to jump over: from the first source frame to the second.
    skips: Vec<(u64, u64)>,
    /// The block in `decoded` was cut where a skip begins: once the chain has
    /// taken it, the decoder moves to this frame.
    jump: Option<u64>,
}

/// An A-B loop over the source frames `start .. end`, and where playback stands in
/// it.
#[derive(Debug, Clone, Copy)]
struct Loop {
    range: LoopRange,
    start: u64,
    end: u64,
    /// The block in `decoded` ends at `end`: once the chain has taken it, the decoder
    /// moves back to `start`.
    at_end: bool,
    /// Nothing has been decoded since the last move back to `start`. The end of the
    /// source is then no reason to move back again (a start past the end).
    fresh: bool,
}

impl Loop {
    fn new(range: LoopRange, rate: SampleRate) -> Self {
        Self {
            range,
            start: range.start.frames_at(rate),
            end: range.end.frames_at(rate),
            at_end: false,
            fresh: false,
        }
    }
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
            looping: None,
            wraps: Vec::new(),
            end: None,
            skips: Vec::new(),
            jump: None,
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
        // The PCM decoder may run at another rate.
        self.looping = self
            .looping
            .map(|l| Loop::new(l.range, self.format.spec.rate));
        self.end = None;
        self.skips.clear();
        self.jump = None;
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
        self.jump = None;
        let at = self.decoder.position();
        if let Some(chain) = &mut self.chain {
            chain.reset();
            chain.locate(at);
        }
        // The loop stays; where playback stands in it starts anew.
        if let Some(l) = &mut self.looping {
            l.at_end = false;
            l.fresh = false;
        }
        self.counted_from = self.decoder.position();
        self.rendered = 0;
        Ok(self.decoder.position())
    }

    /// Builds the processing chain of `plan`, for the device the plan names.
    pub(super) fn attach(&mut self, plan: PathPlan) -> Result<(), EngineError> {
        let mut chain = SourceChain::with_analysis(&plan, MAX_BLOCK, self.item.analysis.clone())
            .map_err(|error| EngineError::Output {
                reason: error.to_string(),
            })?;
        chain.locate(self.next_frame());
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
    /// `None` too while an A-B loop will bring playback back to its start.
    pub(super) fn remaining_frames(&self) -> Option<u64> {
        if self.loops() {
            return None;
        }
        let total = self.output_frames(self.counted_from)?;
        Some(total.saturating_sub(self.rendered))
    }

    /// The frames of the whole source at the device rate of the attached plan, or
    /// at the source rate before one is attached.
    pub(super) fn total_frames(&self) -> Option<u64> {
        self.output_frames(0)
    }

    /// The device frames made from the source's frames `from` to its end (or where
    /// it ends early), less the silence it jumps over.
    fn output_frames(&self, from: u64) -> Option<u64> {
        let rate = self.rate();
        let mut last = self.length?.frames_at(rate);
        if let Some(end) = self.end {
            last = last.min(end);
        }
        // Only stretches that begin ahead are jumped over.
        let skipped: u64 = self
            .skips
            .iter()
            .filter(|&&(a, _)| a >= from)
            .map(|&(a, b)| b.min(last).saturating_sub(a))
            .sum();
        let frames = last.saturating_sub(from).saturating_sub(skipped);
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
        chain.set_leveller(plan.leveller);
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
        self.wraps.clear();
        let Some(chain) = &self.chain else {
            return 0;
        };
        let in_channels = chain.input_channels();
        let out_channels = chain.output_channels();
        let room = out.len() / out_channels;
        let mut written = 0;
        // Where the frames counted in `rendered` begin in `out`.
        let mut counted = 0;
        while written < room && !self.finished {
            self.untouched = false;
            if self.consumed < self.decoded.len() {
                let Some(chain) = &mut self.chain else {
                    break;
                };
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
            if self.jump_due() {
                if self.jump(written) {
                    counted = written;
                }
                continue;
            }
            if self.wrap_due() {
                if self.wrap(written) {
                    counted = written;
                }
                continue;
            }
            if !self.eof {
                self.decode_next();
                continue;
            }
            // The decoder is done: empty the chain (the resampler's tail).
            let Some(chain) = &mut self.chain else {
                break;
            };
            if room - written < chain.max_output_frames() {
                break;
            }
            let made = chain.finish(&mut out[written * out_channels..room * out_channels]);
            if made == 0 {
                self.finished = true;
            }
            written += made;
        }
        self.rendered += (written - counted) as u64;
        written
    }
}

/// The A-B loop.
///
/// Each decoded block that starts before the loop's end and reaches it is cut there.
/// Once the chain has taken the cut block, the decoder moves back to the loop's
/// start without the chain being reset, so the start follows the end as the next
/// frames, without a gap. The end of the source inside the loop moves back too.
/// Blocks that start at or after the end are not cut: a seek past the end leaves
/// the loop.
impl Source {
    /// Loops `range`, or ends the loop with `None`. A block already decoded is cut
    /// at the new end when the chain has not taken frames past it; one cut at an
    /// earlier end is made whole again.
    pub(super) fn set_loop(&mut self, range: Option<LoopRange>) {
        if self.looping.map(|l| l.range) == range {
            return;
        }
        if self.looping.is_some_and(|l| l.at_end) {
            // The block was cut at the old end: the rest of it plays now.
            self.decoded.clear();
            self.block.append_f32(&mut self.decoded);
            block_words(&self.block, self.format, &mut self.words);
        }
        self.looping = range
            .filter(|r| r.start < r.end)
            .map(|r| Loop::new(r, self.rate()))
            .filter(|l| l.start < l.end);
        if !self.decoded.is_empty() {
            self.cut_at_loop_end();
        }
    }

    /// Whether the loop will bring playback back to its start: the block in hand
    /// ends at the loop's end, or the next frame for the chain comes before it.
    pub(super) fn loops(&self) -> bool {
        self.looping
            .is_some_and(|l| l.at_end || self.next_frame() < l.end)
    }

    /// The source frame the chain takes next.
    pub(super) fn next_frame(&self) -> u64 {
        if self.consumed < self.decoded.len() {
            let channels = usize::from(self.format.spec.channel_count()).max(1);
            self.block.position() + (self.consumed / channels) as u64
        } else {
            self.decoder.position()
        }
    }

    /// Where the last render call moved back to the loop's start: the output frames
    /// written by then, and the source frame it went on at.
    pub(super) fn wraps(&self) -> &[(usize, u64)] {
        &self.wraps
    }

    /// Cuts the block in `decoded` at the loop's end, when it starts before the end,
    /// reaches it, and the chain has not taken frames past it.
    fn cut_at_loop_end(&mut self) {
        let channels = usize::from(self.format.spec.channel_count()).max(1);
        let Some(l) = &mut self.looping else {
            return;
        };
        let first = self.block.position();
        let frames = (self.decoded.len() / channels) as u64;
        if frames > 0 {
            l.fresh = false;
        }
        let taken = (self.consumed / channels) as u64;
        if first < l.end && l.end <= first + frames && first + taken <= l.end {
            let keep = usize::try_from(l.end - first).unwrap_or(usize::MAX) * channels;
            self.decoded.truncate(keep);
            if !self.words.is_empty() {
                self.words.truncate(keep);
            }
            l.at_end = true;
        }
    }

    /// Whether the decoder moves back to the loop's start now: the block cut at the
    /// end has been taken, or the source ended inside the loop after something was
    /// decoded since the last move.
    fn wrap_due(&self) -> bool {
        self.consumed >= self.decoded.len()
            && self.looping.is_some_and(|l| {
                l.at_end
                    || (self.eof
                        && !l.fresh
                        && self.error.is_none()
                        && self.decoder.position() < l.end)
            })
    }

    /// Moves the decoder back to the loop's start, keeping the chain, and notes the
    /// move at the output frame `written`. Returns whether it moved.
    fn wrap(&mut self, written: usize) -> bool {
        let Some(l) = &mut self.looping else {
            return false;
        };
        l.at_end = false;
        l.fresh = true;
        let start = MediaTime::from_frames(l.start, self.format.spec.rate);
        if let Err(error) = self.decoder.seek(start) {
            self.eof = true;
            self.error = Some(error.into());
            return false;
        }
        self.decoded.clear();
        self.words.clear();
        self.consumed = 0;
        self.eof = false;
        let at = self.decoder.position();
        self.wraps.push((written, at));
        self.counted_from = at;
        self.rendered = 0;
        if let Some(chain) = &mut self.chain {
            chain.locate(at);
        }
        true
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
                !self.eof || self.wrap_due() || self.jump_due()
            }
    }

    /// Decodes into `writer` as integers, up to `max_frames` frames or until the
    /// ring is full, and returns the number of frames written. Only while
    /// [`renders_exact`](Self::renders_exact); the end of the source is left to
    /// [`render`](Self::render).
    pub(super) fn render_exact(&mut self, writer: &mut SampleWriter, max_frames: usize) -> usize {
        self.wraps.clear();
        let channels = usize::from(self.format.spec.channel_count()).max(1);
        let mut written = 0;
        let mut counted = 0;
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
            if self.jump_due() {
                if self.jump(written) {
                    counted = written;
                }
                continue;
            }
            if self.wrap_due() {
                if self.wrap(written) {
                    counted = written;
                }
                continue;
            }
            if self.eof || !self.decode_next() {
                break;
            }
        }
        self.rendered += (written - counted) as u64;
        written
    }

    /// Reads the next block into `decoded` and `words`, cut at the loop's end.
    /// Returns whether there is one; at the end of the source or an error, sets
    /// `eof`.
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
                self.cut_at_loop_end();
                self.cut_at_trims();
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

/// Silence left out: an early end (the trailing silence) and jumps over long silent
/// stretches inside the source.
///
/// A block that reaches the end is cut there and the source ends after it. A block
/// in which a skip begins is cut where it begins; once the chain has taken it, the
/// decoder moves to the skip's end without the chain being reset, as for the A-B
/// loop, and the move is reported in [`wraps`](Source::wraps), so that positions
/// stay those of the file. Neither applies while an A-B loop is set.
impl Source {
    /// Whether nothing has been played and the source stands at its first frame.
    pub(super) fn is_at_start(&self) -> bool {
        self.untouched && self.decoder.position() == 0
    }

    /// The source frame where the source ends early, if it does.
    pub(super) fn end(&self) -> Option<u64> {
        self.end
    }

    /// Ends the source at the source frame `end`, or at its real end with `None`.
    /// Does nothing once the source has passed `end`, or has already ended at an
    /// earlier one.
    pub(super) fn set_end(&mut self, end: Option<u64>) {
        if self.end == end || (self.end.is_some() && self.eof && self.error.is_none()) {
            return;
        }
        if end.is_some_and(|end| self.next_frame() > end) {
            return;
        }
        self.end = end;
        if end.is_some() && self.jump.is_none() && self.consumed < self.decoded.len() {
            self.cut_at_end();
        }
    }

    /// Jumps over the source frames of each of `skips` (in order, not overlapping)
    /// from now on.
    pub(super) fn set_skips(&mut self, skips: Vec<(u64, u64)>) {
        self.skips = skips;
    }

    /// Cuts the block just decoded where a skip begins in it, or at the end.
    fn cut_at_trims(&mut self) {
        if self.looping.is_some() {
            return;
        }
        let channels = usize::from(self.format.spec.channel_count()).max(1);
        let first = self.block.position();
        let frames = (self.decoded.len() / channels) as u64;
        let skip = self
            .skips
            .iter()
            .find(|&&(from, _)| from >= first && from < first + frames)
            .copied();
        if let Some((from, to)) = skip {
            let keep = usize::try_from(from - first).unwrap_or(usize::MAX) * channels;
            self.decoded.truncate(keep);
            if !self.words.is_empty() {
                self.words.truncate(keep);
            }
            self.jump = Some(to);
            return;
        }
        self.cut_at_end();
    }

    /// Cuts the block in `decoded` at the end, when it reaches past it, and ends the
    /// source after it. Frames the chain has taken stay.
    fn cut_at_end(&mut self) {
        let Some(end) = self.end else {
            return;
        };
        let channels = usize::from(self.format.spec.channel_count()).max(1);
        let first = self.block.position();
        let frames = (self.decoded.len() / channels) as u64;
        if first + frames <= end {
            return;
        }
        let keep = usize::try_from(end.saturating_sub(first))
            .unwrap_or(usize::MAX)
            .saturating_mul(channels)
            .max(self.consumed);
        self.decoded.truncate(keep);
        if !self.words.is_empty() {
            self.words.truncate(keep);
        }
        self.eof = true;
    }

    /// Whether the decoder jumps over a silent stretch now: the block cut where it
    /// begins has been taken.
    fn jump_due(&self) -> bool {
        self.jump.is_some() && self.consumed >= self.decoded.len()
    }

    /// Moves the decoder to the end of the silent stretch, keeping the chain, and
    /// notes the move at the output frame `written`. Returns whether it moved.
    fn jump(&mut self, written: usize) -> bool {
        let Some(to) = self.jump.take() else {
            return false;
        };
        if let Err(error) = self
            .decoder
            .seek(MediaTime::from_frames(to, self.format.spec.rate))
        {
            self.eof = true;
            self.error = Some(error.into());
            return false;
        }
        self.decoded.clear();
        self.words.clear();
        self.consumed = 0;
        self.eof = false;
        let at = self.decoder.position();
        self.wraps.push((written, at));
        self.counted_from = at;
        self.rendered = 0;
        if let Some(chain) = &mut self.chain {
            chain.locate(at);
        }
        true
    }
}
