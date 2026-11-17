// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A span of a source, such as a track of a CUE sheet, trimmed to the exact frame.
//!
//! The span's times are converted to frames at the decoder's rate, rounding down,
//! which is exact for CUE times (75 frames per second) at every common rate. The
//! wrapped decoder seeks to the start, and blocks are cut at the start and the end,
//! so consecutive spans of one file join without a gap or an overlap.

use helikodu_model::audio::{AudioSpec, MediaTime, SampleRate};

use super::block::PcmBlock;
use super::decoder::{Decoder, DecoderInfo};
use super::error::DecodeError;

/// A decoder restricted to the frames `start .. end` of another.
#[derive(Debug)]
pub(super) struct SectionDecoder {
    inner: Box<dyn Decoder>,
    rate: SampleRate,
    /// The first frame of the span in the inner decoder.
    start: u64,
    /// The frame after the span's last, when the span has an end.
    end: Option<u64>,
    /// The first block, decoded when opening to check that the span has audio.
    pending: Option<PcmBlock>,
}

impl SectionDecoder {
    /// Restricts `inner` to `start .. end` and moves it to `start`.
    ///
    /// The length the container states is not trusted to decide whether the span
    /// lies within the file (for some formats it is an estimate): the first block
    /// is decoded instead, and a span without audio is an error.
    pub(super) fn new(
        mut inner: Box<dyn Decoder>,
        start: MediaTime,
        end: Option<MediaTime>,
    ) -> Result<Self, DecodeError> {
        let rate = inner.spec().rate;
        let start_frame = start.frames_at(rate);
        let end_frame = end.map(|e| e.frames_at(rate));
        if let Some(end) = end_frame
            && end <= start_frame
        {
            return Err(DecodeError::InvalidSection {
                reason: format!(
                    "it ends at {} before it starts at {start}",
                    MediaTime::from_frames(end, rate)
                ),
            });
        }
        if start_frame > 0 {
            inner.seek(MediaTime::from_frames(start_frame, rate))?;
        }
        let mut section = Self {
            inner,
            rate,
            start: start_frame,
            end: end_frame,
            pending: None,
        };
        let mut first = PcmBlock::new();
        if !section.next_block(&mut first)? {
            return Err(DecodeError::InvalidSection {
                reason: format!("it starts at {start}, after the end of the file"),
            });
        }
        section.pending = Some(first);
        Ok(section)
    }

    /// Reads the next block of the inner decoder and cuts it to the span.
    fn next_block(&mut self, block: &mut PcmBlock) -> Result<bool, DecodeError> {
        loop {
            if self.end.is_some_and(|end| self.inner.position() >= end) {
                return Ok(false);
            }
            if !self.inner.read(block)? {
                return Ok(false);
            }
            let first = block.position();
            if first < self.start {
                // Never after an exact seek; kept for decoders that land early.
                let before = usize::try_from(self.start - first).unwrap_or(usize::MAX);
                block.skip_front(before);
            }
            if let Some(end) = self.end {
                let room = end.saturating_sub(block.position());
                block.truncate(usize::try_from(room).unwrap_or(usize::MAX));
            }
            if block.is_empty() {
                continue;
            }
            block.set_position(block.position() - self.start);
            return Ok(true);
        }
    }
}

impl Decoder for SectionDecoder {
    fn info(&self) -> &DecoderInfo {
        self.inner.info()
    }

    fn spec(&self) -> AudioSpec {
        self.inner.spec()
    }

    fn duration(&self) -> Option<MediaTime> {
        // The span's end, unless the file ends before it.
        let length = self.inner.duration().map(|d| d.frames_at(self.rate));
        let end = match (self.end, length) {
            (Some(end), Some(length)) => end.min(length),
            (end, length) => end.or(length)?,
        };
        Some(MediaTime::from_frames(
            end.saturating_sub(self.start),
            self.rate,
        ))
    }

    fn position(&self) -> u64 {
        if let Some(block) = &self.pending {
            return block.position();
        }
        let position = self.inner.position().saturating_sub(self.start);
        match self.end {
            Some(end) => position.min(end - self.start),
            None => position,
        }
    }

    fn read(&mut self, block: &mut PcmBlock) -> Result<bool, DecodeError> {
        if let Some(mut first) = self.pending.take() {
            // Hand over the block and keep the caller's allocation for later.
            std::mem::swap(block, &mut first);
            return Ok(true);
        }
        self.next_block(block)
    }

    fn seek(&mut self, to: MediaTime) -> Result<(), DecodeError> {
        self.pending = None;
        let mut target = self.start.saturating_add(to.frames_at(self.rate));
        if let Some(end) = self.end {
            target = target.min(end);
        }
        self.inner.seek(MediaTime::from_frames(target, self.rate))
    }
}
