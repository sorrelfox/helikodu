// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The A-B loop of the loaded item.
//!
//! The worker keeps the loop and gives it to the item's [`Source`] whenever that is
//! attached (on resume, after a seek or a device switch, or opened anew), and the
//! source moves back to the loop's start each time it reaches the end, without a gap.
//! Each move back is recorded as a rewind of the item's timeline segment, so the
//! heard position follows.
//!
//! The render side works up to a buffer length ahead. When a loop is set, changed or
//! ended, the audio rendered after the heard position is compared with what the new
//! loop asks for; from the first ring position where they part (the new end passed,
//! a move back that no longer applies, the next item already joined), the buffer is
//! dropped with a `Discard` and the item is rendered again from the position the loop
//! gives there. When the heard position is already past the new end, that is at once:
//! playback jumps to the start.

use helikodu_model::audio::MediaTime;
use helikodu_model::player::{ItemId, LoopRange, TransportState};

use super::source::Source;
use super::timeline::Segment;
use super::worker::Worker;
use crate::output::DeviceAction;

/// With resampling, how far (in source frames) a recorded move back may lie from the
/// loop's end and still count as one at the end: the resampler holds some input.
const RESAMPLED_SLACK: u64 = 4096;

impl Worker {
    /// Loops `range` of the loaded item `item`, or ends the loop with `None`.
    /// Ignored when `item` is not the item heard (or paused at), or the range is
    /// empty.
    pub(super) fn set_loop(&mut self, item: ItemId, range: Option<LoopRange>) {
        if range.is_some_and(|r| r.start >= r.end) {
            return;
        }
        let loaded = match self.state {
            TransportState::Paused => self.resume.as_ref().map(|r| r.segment.item.id),
            TransportState::Stopped | TransportState::Loading => None,
            _ => self.timeline.heard().map(|s| s.item.id),
        };
        if loaded != Some(item) {
            return;
        }
        let wanted = range.map(|r| (item, r));
        if self.ab_loop == wanted {
            return;
        }
        self.ab_loop = wanted;
        if self.crossfade.is_none() && self.ramp.is_none() {
            // The decision about a crossfade at the item's end is made again.
            self.crossfade_checked = None;
        }
        match self.state {
            TransportState::Paused => self.set_loop_paused(item, range),
            TransportState::Playing | TransportState::Draining => {
                self.set_loop_playing(item, range)
            }
            _ => {}
        }
    }

    /// The loop of `item`, if it has one.
    pub(super) fn loop_of(&self, item: ItemId) -> Option<LoopRange> {
        self.ab_loop
            .filter(|(id, _)| *id == item)
            .map(|(_, range)| range)
    }

    /// Whether `item`, heard at `position`, comes back to its loop's start rather
    /// than ending.
    pub(super) fn loops_at(&self, item: ItemId, position: MediaTime) -> bool {
        self.loop_of(item).is_some_and(|range| position < range.end)
    }

    /// Forgets the loop unless it belongs to `item`, the item now heard.
    pub(super) fn keep_loop_of(&mut self, item: ItemId) {
        if self.ab_loop.is_some_and(|(id, _)| id != item) {
            self.ab_loop = None;
        }
    }

    /// Records the current source's moves back to its loop's start during the last
    /// render call, which wrote from the ring position `at`.
    pub(super) fn note_wraps(&mut self, at: u64) {
        let Some(source) = &self.current else {
            return;
        };
        if source.wraps().is_empty() {
            return;
        }
        let id = source.item().id;
        if let Some(segment) = self.timeline.segment_mut(id) {
            for &(offset, frames) in source.wraps() {
                segment.rewind(at + offset as u64, frames);
            }
        }
    }

    /// While paused: the source takes the loop when playback resumes. A paused
    /// position at or past the end moves to the start.
    fn set_loop_paused(&mut self, item: ItemId, range: Option<LoopRange>) {
        let Some(range) = range else {
            return;
        };
        let Some(resume) = &mut self.resume else {
            return;
        };
        if resume.position < range.end {
            return;
        }
        resume.position = match resume.segment.length {
            Some(length) if range.start > length => length,
            _ => range.start,
        };
        let position = resume.position;
        self.send_position_of(item, position);
    }

    /// While playing: gives the source the loop, and drops what was rendered
    /// against it.
    fn set_loop_playing(&mut self, item: ItemId, range: Option<LoopRange>) {
        let Some(heard) = self.timeline.heard().cloned() else {
            return;
        };
        let Some(output) = &self.output else {
            return;
        };
        let written = output.writer.frames_written();
        let channels = output.writer.channels().max(1);
        let rendered_end = written + (self.staged.len() / channels) as u64;
        let at = self.heard_now();
        // The heard item's own audio ends where a crossfade or the next item begins.
        let own_end = heard
            .mix_from
            .or(self.timeline.follower().map(|s| s.ring_start))
            .unwrap_or(rendered_end)
            .min(rendered_end);
        let rate = heard.spec.rate;
        let frames = range.map(|r| (r.start.frames_at(rate), r.end.frames_at(rate)));
        let slack = if heard.spec.rate == heard.device_rate {
            0
        } else {
            RESAMPLED_SLACK
        };
        // Only the heard item, rendered alone, with nothing after it.
        let alone = self.timeline.len() == 1
            && self.crossfade.is_none()
            && self.ramp.is_none()
            && self
                .current
                .as_ref()
                .is_some_and(|s| s.item().id == item && !s.is_finished());
        let mut cut = departure(&heard, at, own_end, frames, slack);
        if cut.is_none() && frames.is_some() && !alone {
            // The item ends there in the buffer, but now it loops.
            cut = Some((own_end, heard.frames_at(own_end)));
        }
        if cut.is_none()
            && let Some(source) = self.current.as_mut().filter(|s| s.item().id == item)
        {
            source.set_loop(range);
            // The chain may have taken frames past the end that are not rendered
            // yet: then the loop cannot be met there.
            if let Some((_, end)) = frames
                && !source.loops()
                && source.next_frame() > end
            {
                cut = Some((written, heard.frames_at(written)));
            }
        }
        if let Some((from, frames_there)) = cut {
            self.cut_for_loop(&heard, from, frames_there, frames);
        }
    }

    /// Drops the audio rendered from the ring position `from` on and renders the
    /// heard item again from `frames` (its frames at the source rate) there, for the
    /// loop `range` (in source frames). The next item, if already joined, follows
    /// again later.
    fn cut_for_loop(&mut self, heard: &Segment, from: u64, frames: u64, range: Option<(u64, u64)>) {
        let id = heard.item.id;
        let at = self.heard_now();
        let Some(output) = &mut self.output else {
            return;
        };
        let until = output.writer.frames_written();
        // An action scheduled earlier for a later position would hold the cut back
        // until there: cut there instead.
        let mut cut = from.max(at).max(output.writer.scheduled_until()).min(until);
        if cut < until {
            // The device side may have taken the frames at the cut already: then the
            // cut comes where it stands, and playback goes on from what was heard
            // there.
            let action = DeviceAction::Discard { until, hold: false };
            if let Some(applies) = output.writer.schedule_where(cut, action) {
                cut = applies.min(until);
            }
            self.timeline.skip(cut, until);
        }
        let mut frames = if cut == from {
            frames
        } else {
            heard.frames_at(cut)
        };
        if let Some((start, end)) = range
            && frames >= end
        {
            frames = start;
        }
        // What follows the heard item in the buffer goes; the next item stays next.
        let follower = self.timeline.follower().map(|s| s.item.id);
        self.timeline.cut_after_heard();
        let incoming = self.cancel_crossfade();
        let mut source = None;
        for candidate in [self.current.take(), incoming].into_iter().flatten() {
            if candidate.item().id == id {
                source = Some(candidate);
            } else if Some(candidate.item().id) == follower {
                let mut next = candidate;
                if next.seek(MediaTime::ZERO).is_ok() {
                    self.next = Some(next);
                    self.next_answered = true;
                }
            }
        }
        self.staged = 0..0;
        self.drain = None;
        self.waiting_since = None;
        let source = match source {
            Some(source) => Ok(source),
            None => Source::open(&self.registry, heard.item.clone()),
        };
        let started = source.and_then(|mut source| {
            source.seek(MediaTime::from_frames(frames, heard.spec.rate))?;
            Ok(source)
        });
        match started {
            Ok(source) => {
                self.current = Some(source);
                if let Err(error) = self.attach_current() {
                    self.current = None;
                    self.error(Some(id), error);
                }
            }
            Err(error) => self.error(Some(id), error),
        }
        if let Some(segment) = self.timeline.segment_mut(id) {
            segment.mix_from = None;
            segment.rewind(until, frames);
        }
        let dither = self
            .current
            .as_ref()
            .and_then(Source::plan)
            .is_some_and(|plan| plan.always_dither);
        if let Some(output) = &mut self.output {
            let _ = output
                .writer
                .schedule(until, DeviceAction::AlwaysDither(dither));
        }
        if self.state == TransportState::Draining && self.current.is_some() {
            self.set_state(TransportState::Playing);
        }
    }
}

/// Where the audio of `segment` rendered from the heard ring position `at` up to
/// `until` first parts from what the loop `range` (in source frames) asks for, and
/// the frame playback goes on at from there; `None` where it does not. `slack`
/// allows recorded moves back to lie that far from the loop's end.
fn departure(
    segment: &Segment,
    at: u64,
    until: u64,
    range: Option<(u64, u64)>,
    slack: u64,
) -> Option<(u64, u64)> {
    let (mut ring, mut position) = (at, segment.frames_at(at));
    let ahead = segment
        .rewinds
        .iter()
        .filter(|&&(w, _)| w > at && w < until);
    for &(w, frames) in ahead {
        let reached = position + segment.to_source(w - ring);
        let Some((start, end)) = range else {
            // Without a loop nothing moves back.
            return Some((w, reached));
        };
        if position >= end {
            return Some((ring, start));
        }
        if reached.abs_diff(end) <= slack {
            // A move back at the end: right if it goes to the start.
            if frames != start {
                return Some((w, start));
            }
        } else if reached > end {
            return Some((ring + segment.to_ring(end - position), start));
        } else {
            // A move back before the end, which is later now.
            return Some((w, reached));
        }
        (ring, position) = (w, frames);
    }
    let (start, end) = range?;
    if position >= end {
        return Some((ring, start));
    }
    let passed = ring + segment.to_ring(end - position);
    (passed < until).then_some((passed, start))
}

#[cfg(test)]
mod tests {
    use helikodu_model::audio::{AudioSpec, ChannelLayout, Endianness, SampleFormat, SampleRate};
    use helikodu_model::library::MediaSource;
    use helikodu_model::player::PlaybackItem;

    use super::*;

    fn segment() -> Segment {
        Segment {
            ring_start: 0,
            item: PlaybackItem::new(ItemId::new(1), MediaSource::file("/x.wav")),
            spec: AudioSpec::new(
                SampleFormat::S16(Endianness::Little),
                SampleRate::HZ_44100,
                ChannelLayout::STEREO,
            ),
            length: None,
            start_frames: 0,
            device_rate: SampleRate::HZ_44100,
            error: None,
            mix_from: None,
            rewinds: Vec::new(),
        }
    }

    #[test]
    fn the_buffer_is_cut_where_it_parts_from_the_loop() {
        let mut s = segment();
        let range = Some((10_000, 30_000));
        // Not rendered up to the end yet: nothing to cut.
        assert_eq!(departure(&s, 20_000, 29_000, range, 0), None);
        // Rendered past it: cut at the end, go on at the start.
        assert_eq!(
            departure(&s, 20_000, 35_000, range, 0),
            Some((30_000, 10_000))
        );
        // Heard past it: at once.
        assert_eq!(
            departure(&s, 31_000, 35_000, range, 0),
            Some((31_000, 10_000))
        );
        // Moved back at the end already, as the loop asks.
        s.rewind(30_000, 10_000);
        assert_eq!(departure(&s, 20_000, 35_000, range, 0), None);
        // A later end: the move back no longer applies.
        assert_eq!(
            departure(&s, 20_000, 35_000, Some((10_000, 40_000)), 0),
            Some((30_000, 30_000))
        );
        // Another start.
        assert_eq!(
            departure(&s, 20_000, 35_000, Some((5_000, 30_000)), 0),
            Some((30_000, 5_000))
        );
        // No loop: playback goes on past the end.
        assert_eq!(
            departure(&s, 20_000, 35_000, None, 0),
            Some((30_000, 30_000))
        );
        assert_eq!(departure(&s, 31_000, 35_000, None, 0), None);
    }
}
