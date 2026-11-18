// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which item each part of the ring buffer belongs to.
//!
//! The render thread works ahead of the listener by up to the buffer length, and at
//! a gapless join the next item's first frames follow the last ones of the item
//! before in the same buffer. The timeline records, for every ring position where an
//! item starts (or restarts after a seek), the item and its position there. The
//! position the listener hears, from the stream's counters, then gives the playing
//! item and its position, and crossing a start gives the `TrackEnded` and
//! `TrackStarted` events at the moment the join is heard.
//!
//! At a crossfade the next item's segment starts at the middle of the crossfade,
//! with the item's position there; the item before records where its audio began
//! to be mixed with it (`mix_from`).
//!
//! An A-B loop moves an item back within its segment: the segment records each ring
//! position where that happens (a rewind) and the item's position there.

use std::collections::VecDeque;

use helikodu_model::audio::{AudioSpec, MediaTime, SampleRate};
use helikodu_model::player::{EngineError, EngineEvent, ItemId, PlaybackItem, TrackEndReason};

/// Where an item starts in the ring buffer.
#[derive(Debug, Clone)]
pub(super) struct Segment {
    /// The ring position of the item's first frame here.
    pub(super) ring_start: u64,
    pub(super) item: PlaybackItem,
    /// The decoded format.
    pub(super) spec: AudioSpec,
    pub(super) length: Option<MediaTime>,
    /// The item's position at `ring_start`, in frames at the source rate.
    pub(super) start_frames: u64,
    /// The rate of the ring buffer.
    pub(super) device_rate: SampleRate,
    /// The error that ended the item early, once the render side has met it.
    pub(super) error: Option<EngineError>,
    /// The ring position from which the item is crossfaded into the next one.
    pub(super) mix_from: Option<u64>,
    /// Where the item moved back (an A-B loop): ring positions in ascending order,
    /// and the item's position there, in frames at the source rate.
    pub(super) rewinds: Vec<(u64, u64)>,
}

impl Segment {
    /// The item's position at the ring position `at`.
    pub(super) fn position_at(&self, at: u64) -> MediaTime {
        let time = MediaTime::from_frames(self.frames_at(at), self.spec.rate);
        match self.length {
            Some(length) if time > length => length.at_rate(self.spec.rate),
            _ => time,
        }
    }

    /// The item's position at the ring position `at`, in frames at the source rate,
    /// not limited to its length.
    pub(super) fn frames_at(&self, at: u64) -> u64 {
        let (ring, frames) = self.base_at(at);
        frames.saturating_add(self.to_source(at.saturating_sub(ring)))
    }

    /// The latest start or rewind at or before the ring position `at`.
    pub(super) fn base_at(&self, at: u64) -> (u64, u64) {
        self.rewinds
            .iter()
            .rev()
            .find(|&&(ring, _)| ring <= at)
            .copied()
            .unwrap_or((self.ring_start, self.start_frames))
    }

    /// `frames` ring frames as frames at the source rate, rounded down.
    pub(super) fn to_source(&self, frames: u64) -> u64 {
        let scaled = u128::from(frames) * u128::from(self.spec.rate.hz())
            / u128::from(self.device_rate.hz());
        u64::try_from(scaled).unwrap_or(u64::MAX)
    }

    /// `frames` frames at the source rate as ring frames, rounded up.
    pub(super) fn to_ring(&self, frames: u64) -> u64 {
        let scaled = (u128::from(frames) * u128::from(self.device_rate.hz()))
            .div_ceil(u128::from(self.spec.rate.hz()));
        u64::try_from(scaled).unwrap_or(u64::MAX)
    }

    /// Records that the item goes on at `frames` from the ring position `at`.
    /// Rewinds at or after `at` are replaced.
    pub(super) fn rewind(&mut self, at: u64, frames: u64) {
        let keep = self.rewinds.partition_point(|&(ring, _)| ring < at);
        self.rewinds.truncate(keep);
        self.rewinds.push((at, frames));
    }

    /// Forgets the rewinds before the latest one at or before the heard position
    /// `at`: positions before it are not asked for again.
    fn prune(&mut self, at: u64) {
        let passed = self.rewinds.partition_point(|&(ring, _)| ring <= at);
        if passed > 1 {
            self.rewinds.drain(..passed - 1);
        }
    }

    /// The `TrackStarted` event of the item.
    pub(super) fn started_event(&self) -> EngineEvent {
        EngineEvent::TrackStarted {
            item: self.item.id,
            spec: self.spec,
            length: self.length,
        }
    }

    /// The events of the item's end: an `Error` first if it failed.
    fn end_events(self, events: &mut Vec<EngineEvent>) {
        let reason = match self.error {
            Some(error) => {
                events.push(EngineEvent::Error {
                    item: Some(self.item.id),
                    error,
                });
                TrackEndReason::Failed
            }
            None => TrackEndReason::Finished,
        };
        events.push(EngineEvent::TrackEnded {
            item: self.item.id,
            reason,
        });
    }
}

/// The items of the ring buffer, from the one being heard on.
#[derive(Debug, Default)]
pub(super) struct Timeline {
    segments: VecDeque<Segment>,
    /// Heard positions below this belong to audio that was dropped.
    floor: u64,
    /// Ring ranges that were dropped after the item before them was written.
    skips: Vec<(u64, u64)>,
}

impl Timeline {
    /// Forgets everything; the next segment starts at `floor`.
    pub(super) fn reset(&mut self, floor: u64) {
        self.segments.clear();
        self.skips.clear();
        self.floor = floor;
    }

    pub(super) fn push(&mut self, segment: Segment) {
        self.segments.push_back(segment);
    }

    /// The number of items in the ring buffer, the heard one included.
    pub(super) fn len(&self) -> usize {
        self.segments.len()
    }

    /// The item being heard.
    pub(super) fn heard(&self) -> Option<&Segment> {
        self.segments.front()
    }

    /// The item written after the one being heard.
    pub(super) fn follower(&self) -> Option<&Segment> {
        self.segments.get(1)
    }

    /// The last item written.
    pub(super) fn last_mut(&mut self) -> Option<&mut Segment> {
        self.segments.back_mut()
    }

    /// The last item `id` written, the heard one included.
    pub(super) fn segment_mut(&mut self, id: ItemId) -> Option<&mut Segment> {
        self.segments.iter_mut().rev().find(|s| s.item.id == id)
    }

    /// Forgets the items, but not the dropped audio, and starts again with
    /// `segment`: positions before its start are its first one.
    pub(super) fn restart_with(&mut self, segment: Segment) {
        self.segments.clear();
        self.segments.push_back(segment);
    }

    /// Removes the items after the heard one and returns the ring position where
    /// they started.
    pub(super) fn cut_after_heard(&mut self) -> Option<u64> {
        let start = self.segments.get(1)?.ring_start;
        self.segments.truncate(1);
        Some(start)
    }

    /// Records that the ring range `from .. until` was dropped.
    pub(super) fn skip(&mut self, from: u64, until: u64) {
        if from < until {
            self.skips.push((from, until));
        }
    }

    /// The heard ring position, corrected for dropped audio.
    pub(super) fn correct(&self, raw: u64) -> u64 {
        let mut at = raw.max(self.floor);
        for &(from, until) in &self.skips {
            if (from..until).contains(&at) {
                at = until;
            }
        }
        at
    }

    /// The item heard at the ring position `at`, and its position there.
    pub(super) fn position(&self, at: u64) -> Option<(&Segment, MediaTime)> {
        let segment = self.heard()?;
        Some((segment, segment.position_at(at)))
    }

    /// Moves the heard position to `at`: every item start passed gives a
    /// `TrackEnded` for the item before it and a `TrackStarted` for it.
    pub(super) fn advance(&mut self, at: u64, events: &mut Vec<EngineEvent>) {
        while self
            .segments
            .get(1)
            .is_some_and(|next| next.ring_start <= at)
        {
            if let Some(ended) = self.segments.pop_front() {
                ended.end_events(events);
            }
            if let Some(started) = self.segments.front() {
                events.push(started.started_event());
            }
        }
        self.skips.retain(|&(_, until)| until > at);
        if let Some(heard) = self.segments.front_mut() {
            heard.prune(at);
        }
    }

    /// Plays everything out: the events of every item start, then the end of the
    /// last item.
    pub(super) fn finish(&mut self, events: &mut Vec<EngineEvent>) {
        self.advance(u64::MAX, events);
        if let Some(last) = self.segments.pop_front() {
            last.end_events(events);
        }
        self.skips.clear();
    }
}

#[cfg(test)]
mod tests {
    use helikodu_model::audio::{ChannelLayout, Endianness, SampleFormat};
    use helikodu_model::library::MediaSource;

    use super::*;

    fn segment(id: u64, ring_start: u64, start_frames: u64) -> Segment {
        Segment {
            ring_start,
            item: PlaybackItem::new(ItemId::new(id), MediaSource::file("/x.flac")),
            spec: AudioSpec::new(
                SampleFormat::S16(Endianness::Little),
                SampleRate::HZ_44100,
                ChannelLayout::STEREO,
            ),
            length: Some(MediaTime::from_frames(100_000, SampleRate::HZ_44100)),
            start_frames,
            device_rate: SampleRate::HZ_48000,
            error: None,
            mix_from: None,
            rewinds: Vec::new(),
        }
    }

    #[test]
    fn positions_follow_the_rate_ratio_and_stop_at_the_end() {
        let s = segment(1, 1000, 441);
        assert_eq!(
            s.position_at(1000 + 480),
            MediaTime::from_frames(882, SampleRate::HZ_44100)
        );
        assert_eq!(
            s.position_at(u64::MAX / 2),
            MediaTime::from_frames(100_000, SampleRate::HZ_44100)
        );
    }

    #[test]
    fn rewinds_move_the_position_back() {
        // At the source rate, so that ring frames are item frames.
        let mut s = segment(1, 1000, 0);
        s.device_rate = SampleRate::HZ_44100;
        s.rewind(31_000, 10_000);
        s.rewind(51_000, 10_000);
        let at = |s: &Segment, ring| s.position_at(ring).frames_at(SampleRate::HZ_44100);
        assert_eq!(at(&s, 30_999), 29_999);
        assert_eq!(at(&s, 31_000), 10_000);
        assert_eq!(at(&s, 50_999), 29_999);
        assert_eq!(at(&s, 51_005), 10_005);
        // A rewind replaces those at or after it.
        s.rewind(41_000, 5_000);
        assert_eq!(s.rewinds, [(31_000, 10_000), (41_000, 5_000)]);
        assert_eq!(at(&s, 51_005), 15_005);
        // Pruning keeps the one the heard position counts from.
        s.prune(45_000);
        assert_eq!(s.rewinds, [(41_000, 5_000)]);
        assert_eq!(at(&s, 45_000), 9_000);
        assert_eq!((s.to_ring(441), s.to_source(441)), (441, 441));
    }

    #[test]
    fn crossing_a_start_ends_one_item_and_starts_the_next() {
        let mut timeline = Timeline::default();
        timeline.reset(0);
        timeline.push(segment(1, 0, 0));
        let mut failed = segment(2, 500, 0);
        failed.error = Some(EngineError::Decode {
            reason: "bad".into(),
        });
        timeline.push(failed);
        timeline.push(segment(3, 800, 0));
        let mut events = Vec::new();
        timeline.advance(499, &mut events);
        assert!(events.is_empty());
        timeline.advance(900, &mut events);
        let kinds: Vec<_> = events
            .iter()
            .map(|e| match e {
                EngineEvent::TrackEnded { item, reason } => format!("end {item} {reason:?}"),
                EngineEvent::TrackStarted { item, .. } => format!("start {item}"),
                EngineEvent::Error { .. } => "error".to_owned(),
                _ => "other".to_owned(),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "end 1 Finished",
                "start 2",
                "error",
                "end 2 Failed",
                "start 3"
            ]
        );
        events.clear();
        timeline.finish(&mut events);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn dropped_ranges_and_the_floor_are_skipped() {
        let mut timeline = Timeline::default();
        timeline.reset(100);
        timeline.skip(200, 300);
        assert_eq!(timeline.correct(50), 100);
        assert_eq!(timeline.correct(250), 300);
        assert_eq!(timeline.correct(310), 310);
    }
}
