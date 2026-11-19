// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Crossfades: the item that ends and the next one mixed at the device rate.
//!
//! - **From the analysis.** When both items have a track analysis and the
//!   transitions take the overlap from it (the default), the window ends where the
//!   outgoing item's sound ends (its trailing silence left out) and begins where it
//!   starts to fade out for good, at most the setting's length before that; the
//!   incoming item starts with its sound. Without a fade the window is short (about
//!   half a second). With an analysis missing on either side, the setting's length
//!   applies as below.
//! - **Between tracks.** With crossfades on, the render side mixes the next item
//!   into the last frames of the current one: outgoing gain `cos(πt/2)`, incoming
//!   `sin(πt/2)`, after each item's own gain and resampling and before the DSP chain.
//!   The window is the setting's length, shortened to half of either item for short
//!   tracks and to what is left of the current one when the next one comes late. It
//!   starts where the current item has that many frames left, which its length gives
//!   exactly. `NearEnd` comes earlier by the crossfade's length, and a freewheeling
//!   sink waits at the window for the answer.
//! - **Events.** `TrackEnded` (`Finished`) for the outgoing item and `TrackStarted`
//!   for the incoming one are sent when the middle of the crossfade is heard; the
//!   incoming item's positions count from its first frame, at the start of the
//!   window.
//! - **Policies.** No crossfade between consecutive tracks of one album while the
//!   settings keep albums gapless, nor on a bit-perfect path in exclusive mode, nor
//!   for an item whose length is not known: they join without a gap. When the next
//!   item needs another stream (another rate, format or channel count), the current
//!   one fades out over the window instead, the stream is reopened, and the next one
//!   fades in over the same length.
//! - **Skips.** With the skip crossfade on, a `Load` while playing crossfades from
//!   the end of the audio already buffered (at most the buffer length) into the new
//!   item, which takes the same stream, over the skip length (shortened to what is
//!   left of the current item). `TrackEnded` (`Interrupted`) and `TrackStarted` are
//!   sent at once, as for any `Load`, and the new item's position counts from its
//!   first frame. Otherwise the current item fades out over the skip fade.
//! - A pause, seek, stop or device switch during a crossfade ends it: playback goes
//!   on with the item heard, and a crossfade into the next one starts again if
//!   there is time left.

use std::time::Duration;

use helikodu_model::audio::{MediaTime, OutputMode};
use helikodu_model::player::{EngineEvent, PlaybackItem, TrackEndReason};
use helikodu_model::settings::TransitionSettings;

use super::device::plan_for;
use super::silence::{analysed_overlap, head_start, skips_for};
use super::source::Source;
use super::worker::Worker;
use crate::dsp::DspChain;
use crate::output::DeviceAction;
use crate::pipeline::{FadeDurations, PathPlan, equal_power};

/// The shortest crossfade; anything shorter joins without one.
const MIN_CROSSFADE: Duration = Duration::from_millis(10);

/// A crossfade under way: the incoming item, rendered alongside the current one.
#[derive(Debug)]
pub(super) struct Crossfade {
    incoming: Source,
    /// Where the incoming item starts, to put it back as the next item.
    start: MediaTime,
    /// The current item's frames still to come before the window begins.
    lead: u64,
    /// The frames of the window, and how many have been mixed.
    length: u64,
    done: u64,
    /// Incoming frames rendered but not mixed yet: `carry[at..end]`, interleaved.
    carry: Vec<f32>,
    at: usize,
    end: usize,
    channels: usize,
}

impl Crossfade {
    fn new(incoming: Source, lead: u64, length: u64, channels: usize) -> Self {
        let capacity = incoming.max_output_frames() * channels;
        let start = MediaTime::from_frames(incoming.position(), incoming.rate());
        Self {
            incoming,
            start,
            lead,
            length,
            done: 0,
            carry: vec![0.0; capacity],
            at: 0,
            end: 0,
            channels,
        }
    }

    /// The most incoming frames the carry holds.
    fn capacity(&self) -> usize {
        self.carry.len() / self.channels
    }

    fn carried(&self) -> usize {
        (self.end - self.at) / self.channels
    }

    /// Renders more of the incoming item when the carry is empty.
    fn refill(&mut self) {
        if self.carried() > 0 || self.incoming.is_finished() {
            return;
        }
        let frames = self.incoming.render(&mut self.carry);
        self.at = 0;
        self.end = frames * self.channels;
    }

    /// Mixes the incoming item into `out`, the outgoing item's frames of the window.
    /// Returns the frames mixed: all of `out`, or fewer when the window ends. An
    /// incoming item that has nothing more counts as silence.
    fn mix(&mut self, out: &mut [f32]) -> usize {
        let channels = self.channels;
        let frames = out.len() / channels;
        let mut k = 0;
        while k < frames && self.done < self.length {
            self.refill();
            let carried = self.carried();
            let left = usize::try_from(self.length - self.done).unwrap_or(usize::MAX);
            let mut m = (frames - k).min(left);
            if carried > 0 {
                m = m.min(carried);
            }
            for i in 0..m {
                let (outgoing, incoming) = equal_power(self.done + i as u64, self.length);
                let frame = &mut out[(k + i) * channels..(k + i + 1) * channels];
                if carried > 0 {
                    let from = self.at + i * channels;
                    for (o, x) in frame.iter_mut().zip(&self.carry[from..from + channels]) {
                        *o = (f64::from(*o) * outgoing + f64::from(*x) * incoming) as f32;
                    }
                } else {
                    for o in frame {
                        *o = (f64::from(*o) * outgoing) as f32;
                    }
                }
            }
            if carried > 0 {
                self.at += m * channels;
            }
            k += m;
            self.done += m as u64;
        }
        k
    }

    /// Moves the incoming frames rendered already into the start of `out`, through
    /// `ramp` if given. Returns the frames moved.
    fn drain(&mut self, out: &mut [f32], ramp: Option<&mut Ramp>) -> usize {
        let frames = self.carried().min(out.len() / self.channels);
        let n = frames * self.channels;
        out[..n].copy_from_slice(&self.carry[self.at..self.at + n]);
        self.at += n;
        if let Some(ramp) = ramp {
            ramp.apply(&mut out[..n], self.channels);
        }
        frames
    }
}

/// An equal-power gain curve on the current item alone: the rise of an incoming
/// item whose crossfade partner ended early, or the fall of an item that fades out
/// before the stream is reopened for the next one.
#[derive(Debug, Clone, Copy)]
pub(super) struct Ramp {
    /// Frames to pass untouched before the curve begins.
    lead: u64,
    done: u64,
    length: u64,
    rising: bool,
}

impl Ramp {
    /// Applies the curve to `block`. After a fall, frames are silent; after a rise,
    /// untouched. Returns whether a rise has ended.
    fn apply(&mut self, block: &mut [f32], channels: usize) -> bool {
        let mut frames = block.chunks_exact_mut(channels.max(1));
        while self.lead > 0 && frames.next().is_some() {
            self.lead -= 1;
        }
        for frame in frames {
            if self.done >= self.length {
                if self.rising {
                    break;
                }
                frame.fill(0.0);
                continue;
            }
            let (outgoing, incoming) = equal_power(self.done, self.length);
            let gain = if self.rising { incoming } else { outgoing };
            for sample in frame {
                *sample = (f64::from(*sample) * gain) as f32;
            }
            self.done += 1;
        }
        self.rising && self.done >= self.length
    }

    /// Frames of `block` (of `frames` frames) the curve changes, from the first.
    fn touches(&self, frames: usize) -> Option<u64> {
        (self.lead < frames as u64 && (self.done < self.length || !self.rising))
            .then_some(self.lead)
    }
}

/// The frames `duration` lasts at `rate`.
fn frames_of(duration: Duration, rate: u32) -> u64 {
    (duration.as_secs_f64() * f64::from(rate)).round() as u64
}

impl Worker {
    /// Replaces the crossfade and fade settings. A crossfade under way finishes as
    /// it began.
    pub(super) fn set_transitions(&mut self, transitions: TransitionSettings) {
        self.config.transitions = transitions;
        self.config.processing.fades = FadeDurations::from_settings(&transitions);
        if self.crossfade.is_none() && self.ramp.is_none() {
            self.crossfade_checked = None;
        }
        // Silence is left out as the new settings ask, from now on.
        self.update_skips();
        self.joins_planned = None;
        if self.next_answered {
            self.plan_joins();
        }
    }

    /// How much earlier than [`EngineConfig::near_end`](super::EngineConfig::near_end)
    /// `NearEnd` comes: the length of the crossfade between tracks, when it is on.
    pub(super) fn crossfade_lead(&self) -> Duration {
        self.config
            .transitions
            .track_crossfade()
            .unwrap_or_default()
    }

    /// Whether the next `budget` frames of the current item are not mixed with
    /// anything: no crossfade or ramp is under way, and no crossfade window can
    /// begin in them. Integer sources go into the ring untouched only then.
    pub(super) fn renders_unmixed(&self, budget: usize) -> bool {
        if self.crossfade.is_some() || self.ramp.is_some() {
            return false;
        }
        let (Some(current), Some(output)) = (&self.current, &self.output) else {
            return true;
        };
        // The trailing silence may still be left out: its start is a window too.
        let cut = self.pending_tail_cut();
        let wanted = self
            .config
            .transitions
            .track_crossfade()
            .filter(|_| self.crossfade_checked != Some(current.item().id));
        if wanted.is_none() && cut.is_none() {
            return true;
        }
        let ahead = wanted.map_or(0, |wanted| frames_of(wanted, output.spec().rate.hz()))
            + cut.unwrap_or(0)
            + (budget + self.staging.len() / output.writer.channels().max(1)) as u64;
        current
            .remaining_frames()
            .is_none_or(|remaining| remaining > ahead)
    }

    /// Whether the item `id` is rendered to its end (or not rendered at all).
    pub(super) fn rendered_to_end(&self, id: helikodu_model::player::ItemId) -> bool {
        let incoming = self.crossfade.as_ref().map(|c| &c.incoming);
        [self.current.as_ref(), incoming]
            .into_iter()
            .flatten()
            .find(|source| source.item().id == id)
            .is_none_or(Source::is_finished)
    }

    /// Renders the next block of the current item into `staging`: alone, through a
    /// ramp, or mixed with the incoming item of a crossfade. Returns the frames
    /// rendered, or `None` to wait at the start of a crossfade window for the answer
    /// to `NearEnd`. Needs `staged` empty: the block goes to the ring position
    /// `frames_written`.
    pub(super) fn render_block(&mut self, channels: usize) -> Option<usize> {
        let at = self.output.as_ref()?.writer.frames_written();
        let limit = match self.plan_crossfade(channels, at) {
            Some(limit) => Some(limit),
            None if self.crossfade.is_none() => self.trim_wait(channels),
            None => None,
        };
        if self.crossfade.is_some() {
            return Some(self.render_mixed(channels, at));
        }
        let room = self.staging.len() / channels;
        let room = limit.map_or(room, |l| room.min(usize::try_from(l).unwrap_or(room)));
        let source = self.current.as_mut()?;
        let frames = source.render(&mut self.staging[..room * channels]);
        if frames == 0 && limit.is_some() && !source.is_finished() {
            return None;
        }
        self.note_wraps(at);
        if let Some(mut ramp) = self.ramp {
            let touches = ramp.touches(frames);
            let ended = ramp.apply(&mut self.staging[..frames * channels], channels);
            self.ramp = (!ended).then_some(ramp);
            if let Some(from) = touches {
                self.schedule(at + from, DeviceAction::AlwaysDither(true));
            }
            if ended {
                self.dither_as_planned(at + frames as u64);
            }
        }
        Some(frames)
    }

    /// Decides, once per item and as its end comes near, whether it is crossfaded
    /// into the next one, and starts the crossfade. Returns how many frames may be
    /// rendered before the window when a freewheeling sink waits there for the next
    /// item.
    fn plan_crossfade(&mut self, channels: usize, at: u64) -> Option<u64> {
        if self.crossfade.is_some() || self.ramp.is_some() {
            return None;
        }
        let wanted = self.config.transitions.track_crossfade()?;
        let current = self.current.as_ref()?;
        let id = current.item().id;
        if self.crossfade_checked == Some(id) {
            return None;
        }
        let output = self.output.as_ref()?;
        let rate = output.spec().rate.hz();
        let freewheel = output.freewheel;
        let bit_perfect = output.mode == OutputMode::Exclusive
            && current.plan().is_some_and(PathPlan::is_bit_perfect);
        let Some(remaining) = current.remaining_frames().filter(|_| !bit_perfect) else {
            self.crossfade_checked = Some(id);
            return None;
        };
        let mut length = frames_of(wanted, rate).min(current.total_frames()? / 2);
        let room = (self.staging.len() / channels) as u64;
        // Before the next item is known, the window may end where the sound does.
        let horizon = remaining.saturating_sub(self.pending_tail_cut().unwrap_or(0));
        if horizon > length + room {
            return None;
        }
        let Some(next) = &self.next else {
            if self.next_answered {
                self.crossfade_checked = Some(id);
            } else if freewheel {
                if self.waits_for_next() {
                    return Some(horizon.saturating_sub(length));
                }
                self.crossfade_checked = Some(id);
            }
            // In real time a later answer still gets a shorter crossfade.
            return None;
        };
        self.crossfade_checked = Some(id);
        if self.config.transitions.keep_albums_gapless && current.item().same_album(next.item()) {
            return None;
        }
        let next_total = next.total_frames().map(|frames| {
            let frames = u128::from(frames) * u128::from(rate) / u128::from(next.rate().hz());
            u64::try_from(frames).unwrap_or(u64::MAX)
        })?;
        // From the analysis: the outgoing item's fade, its trailing silence left out
        // (the end set when the next item became known).
        if self.config.transitions.analysed_crossfade()
            && current.end().is_some()
            && next.item().analysis.is_some()
            && let Some(overlap) = analysed_overlap(current, rate)
        {
            length = length.min(overlap.max(frames_of(MIN_CROSSFADE, rate)));
        }
        length = length.min(next_total / 2).min(remaining);
        if length < frames_of(MIN_CROSSFADE, rate) {
            return None;
        }
        let lead = remaining - length;
        if self.stream_config(next).spec != output.requested {
            // Another stream: fade out here, and in after the reopening.
            self.ramp = Some(Ramp {
                lead,
                done: 0,
                length,
                rising: false,
            });
            self.reopen_fade = Some(Duration::from_secs_f64(length as f64 / f64::from(rate)));
            return None;
        }
        let mut incoming = self.next.take()?;
        if !self.attach_incoming(&mut incoming) {
            return None;
        }
        let window = at + lead;
        // The incoming item is heard from the middle of the window on.
        let half = length / 2;
        let mut segment = self.segment_of(&incoming, window + half);
        let source_rate = u128::from(incoming.rate().hz());
        segment.start_frames +=
            u64::try_from(u128::from(half) * source_rate / u128::from(rate)).unwrap_or(u64::MAX);
        if let Some(outgoing) = self.timeline.segment_mut(id) {
            outgoing.mix_from = Some(window);
        }
        self.timeline.push(segment);
        self.start_crossfade(incoming, lead, length, channels);
        None
    }

    /// Builds the processing of `incoming` for the open stream. A failure reports
    /// the item as failed and asks for another with `NearEnd`.
    fn attach_incoming(&mut self, incoming: &mut Source) -> bool {
        let Some(output) = &self.output else {
            return false;
        };
        let dsp = self.dsp.as_ref().map(DspChain::plan).unwrap_or_default();
        let plan = plan_for(&self.config.processing, output, incoming, dsp);
        let bit_perfect = output.mode == OutputMode::Exclusive && plan.is_bit_perfect();
        match incoming.attach(plan) {
            Ok(()) => {
                if !bit_perfect {
                    incoming.set_skips(skips_for(&self.config.transitions, incoming));
                }
                true
            }
            Err(error) => {
                let id = incoming.item().id;
                self.error(Some(id), error);
                self.emit(EngineEvent::TrackEnded {
                    item: id,
                    reason: TrackEndReason::Failed,
                });
                self.next_answered = false;
                self.near_end_sent = None;
                false
            }
        }
    }

    fn start_crossfade(&mut self, incoming: Source, lead: u64, length: u64, channels: usize) {
        let crossfade = Crossfade::new(incoming, lead, length, channels);
        let outgoing = self
            .current
            .as_ref()
            .map_or(0, Source::max_output_frames)
            .max(4096);
        let needed = (outgoing + crossfade.capacity()) * channels;
        if self.staging.len() < needed {
            self.staging.resize(needed, 0.0);
        }
        self.crossfade = Some(crossfade);
        self.next_answered = false;
        self.waiting_since = None;
    }

    /// Renders a block of a crossfade at the ring position `at`.
    fn render_mixed(&mut self, channels: usize, at: u64) -> usize {
        let (Some(crossfade), Some(current)) = (&mut self.crossfade, &mut self.current) else {
            return 0;
        };
        let room = (self.staging.len() / channels).saturating_sub(crossfade.capacity());
        let n = current.render(&mut self.staging[..room * channels]);
        let ended = current.is_finished();
        self.note_wraps(at);
        let Some(crossfade) = &mut self.crossfade else {
            return n;
        };
        let lead = usize::try_from(crossfade.lead.min(n as u64)).unwrap_or(n);
        crossfade.lead -= lead as u64;
        let began = crossfade.done == 0;
        let mixed = crossfade.mix(&mut self.staging[lead * channels..n * channels]);
        let end = lead + mixed;
        if began && mixed > 0 {
            self.schedule(at + lead as u64, DeviceAction::AlwaysDither(true));
        }
        let Some(crossfade) = &mut self.crossfade else {
            return n;
        };
        if crossfade.done == crossfade.length {
            // The outgoing item is silent from here: the incoming one takes over.
            let extra = crossfade.drain(&mut self.staging[end * channels..], None);
            self.take_over(None);
            self.dither_as_planned(at + (end + extra) as u64);
            return end + extra;
        }
        if !ended {
            return n;
        }
        if crossfade.done == 0 {
            // The outgoing item ended before the window: a join without a gap.
            let start = crossfade.incoming.position();
            let id = crossfade.incoming.item().id;
            if let Some(segment) = self.timeline.segment_mut(id) {
                segment.ring_start = at + n as u64;
                segment.start_frames = start;
            }
            self.take_over(None);
            self.dither_as_planned(at + n as u64);
            return n;
        }
        // The outgoing item ended early: the incoming one rises on alone.
        let mut ramp = Ramp {
            lead: 0,
            done: crossfade.done,
            length: crossfade.length,
            rising: true,
        };
        let extra = crossfade.drain(&mut self.staging[n * channels..], Some(&mut ramp));
        self.take_over(Some(ramp));
        n + extra
    }

    /// Ends a crossfade: the incoming item becomes the current one, rising on
    /// through `ramp` if it has not reached full level.
    fn take_over(&mut self, ramp: Option<Ramp>) {
        let Some(crossfade) = self.crossfade.take() else {
            return;
        };
        if let Some(mut outgoing) = self.current.replace(crossfade.incoming)
            && let Some(error) = outgoing.take_error()
            && let Some(segment) = self.timeline.segment_mut(outgoing.item().id)
        {
            segment.error = Some(error);
        }
        self.ramp = ramp;
        self.crossfade_checked = None;
    }

    /// Ends a crossfade or ramp under way, for a new start, and returns the incoming
    /// item, back where it started.
    pub(super) fn cancel_crossfade(&mut self) -> Option<Source> {
        self.ramp = None;
        self.reopen_fade = None;
        self.crossfade_checked = None;
        let crossfade = self.crossfade.take()?;
        let mut incoming = crossfade.incoming;
        incoming.seek(crossfade.start).ok()?;
        Some(incoming)
    }

    /// Forgets any crossfade or ramp, with the incoming item.
    pub(super) fn drop_crossfade(&mut self) {
        self.crossfade = None;
        self.ramp = None;
        self.reopen_fade = None;
        self.crossfade_checked = None;
    }

    /// A `Load` while playing: crossfades into `item` if the skip crossfade is on
    /// and the item takes the same stream. Returns whether it did; otherwise the
    /// caller loads the item the usual way.
    pub(super) fn skip_crossfade(&mut self, item: &PlaybackItem, start: MediaTime) -> bool {
        let Some(wanted) = self.config.transitions.skip_crossfade() else {
            return false;
        };
        let (Some(output), Some(current), Some(heard)) =
            (&self.output, &self.current, self.timeline.heard())
        else {
            return false;
        };
        let bit_perfect = output.mode == OutputMode::Exclusive
            && current.plan().is_some_and(PathPlan::is_bit_perfect);
        if bit_perfect
            || self.timeline.len() != 1
            || heard.item.id != current.item().id
            || current.is_finished()
            || self.crossfade.is_some()
            || self.ramp.is_some()
            || self.drain.is_some()
        {
            return false;
        }
        let outgoing = heard.item.id;
        let rate = output.spec().rate.hz();
        let mut length = frames_of(wanted, rate);
        if let Some(remaining) = current.remaining_frames() {
            length = length.min(remaining);
        }
        let trim = self.config.transitions.trim_silence && start.is_zero();
        let Ok(mut incoming) = Source::open(&self.registry, item.clone()).and_then(|mut s| {
            // From the beginning: after the leading silence, when it is trimmed.
            let start = match head_start(&s).filter(|_| trim) {
                Some(frames) => MediaTime::from_frames(frames, s.rate()),
                None => start,
            };
            s.seek(start)?;
            Ok(s)
        }) else {
            return false;
        };
        if self.stream_config(&incoming).spec != output.requested {
            return false;
        }
        let dsp = self.dsp.as_ref().map(DspChain::plan).unwrap_or_default();
        let plan = plan_for(&self.config.processing, output, &incoming, dsp);
        if incoming.attach(plan).is_err() {
            return false;
        }
        incoming.set_skips(skips_for(&self.config.transitions, &incoming));
        if let Some(remaining) = incoming.remaining_frames() {
            length = length.min(remaining / 2);
        }
        if length < frames_of(MIN_CROSSFADE, rate) {
            return false;
        }
        let channels = output.writer.channels().max(1);
        let at = output.writer.frames_written() + (self.staged.len() / channels) as u64;
        self.emit(EngineEvent::TrackEnded {
            item: outgoing,
            reason: TrackEndReason::Interrupted,
        });
        let segment = self.segment_of(&incoming, at);
        let started = segment.started_event();
        self.keep_loop_of(item.id);
        self.timeline.restart_with(segment);
        self.next = None;
        self.next_answered = false;
        self.near_end_sent = None;
        self.crossfade_checked = Some(outgoing);
        let position = MediaTime::from_frames(incoming.position(), incoming.rate());
        self.start_crossfade(incoming, 0, length, channels);
        self.emit(started);
        self.send_position_of(item.id, position);
        true
    }

    /// Schedules on the device side the dither the current item's plan asks for,
    /// from the ring position `at`.
    fn dither_as_planned(&mut self, at: u64) {
        let dither = self
            .current
            .as_ref()
            .and_then(Source::plan)
            .is_some_and(|plan| plan.always_dither);
        self.schedule(at, DeviceAction::AlwaysDither(dither));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fall_ends_in_silence_and_a_rise_at_full_level() {
        let mut fall = Ramp {
            lead: 2,
            done: 0,
            length: 4,
            rising: false,
        };
        let mut block = [1.0f32; 8];
        assert!(!fall.apply(&mut block, 1));
        assert_eq!(&block[..2], &[1.0, 1.0]);
        assert!(block[2] > block[3] && block[3] > block[4] && block[5] > 0.0);
        assert_eq!(&block[6..], &[0.0, 0.0]);

        let mut rise = Ramp {
            lead: 0,
            done: 2,
            length: 4,
            rising: true,
        };
        let mut block = [1.0f32; 4];
        assert!(rise.apply(&mut block, 1));
        assert!(block[0] < block[1] && block[1] < 1.0);
        assert_eq!(&block[2..], &[1.0, 1.0]);
    }
}
