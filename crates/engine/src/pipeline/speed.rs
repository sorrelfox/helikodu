// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Playback speed, and the two kinds of time it makes.
//!
//! A track at another speed takes another time to play. Its own time (*track
//! time*: its positions, its length, its analysis, a seek, an A-B loop) stays what
//! it is; the time the listener spends (*listening time*: the frames of the ring
//! buffer, crossfades and fades, the buffer, `NearEnd`'s lead) is the track time
//! divided by the speed. [`TimeScale`] is the one place where a source's frames and
//! the ring buffer's frames are converted, so that every conversion takes the speed
//! and the two sample rates into account.
//!
//! The speed is applied per source, before the sources are mixed:
//!
//! - **Tape** (the pitch follows the speed): the source's frames are resampled as
//!   if its rate were `rate × speed` ([`SpeedPlan::played`]), so they are played
//!   faster or slower, and higher or lower.
//! - **Tempo** (the pitch stays): [`TempoStage`] stretches the audio by `1 / speed`
//!   with WSOLA at the source rate, then the usual resampler takes it to the device
//!   rate.
//!
//! At the normal speed none of this is built, and nothing changes.

use std::fmt;
use std::time::Duration;

use helikodu_model::audio::SampleRate;

use super::ResamplerQuality;
use crate::dsp::Wsola;

/// The speed asked for: the factor and whether the pitch stays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackSpeed {
    /// The factor of the normal speed (0.5 to 2); 1 plays as recorded.
    pub rate: f64,
    /// Whether the pitch stays (tempo) or follows the speed (tape).
    pub keep_pitch: bool,
}

impl PlaybackSpeed {
    /// The normal speed.
    pub const NORMAL: Self = Self {
        rate: 1.0,
        keep_pitch: true,
    };

    /// The slowest speed.
    pub const MIN: f64 = 0.5;
    /// The fastest speed.
    pub const MAX: f64 = 2.0;

    /// Whether this is the normal speed.
    pub fn is_normal(&self) -> bool {
        !self.rate.is_finite() || self.rate == 1.0
    }
}

impl Default for PlaybackSpeed {
    fn default() -> Self {
        Self::NORMAL
    }
}

/// The speed step of a path plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeedPlan {
    /// The source rate.
    pub source: SampleRate,
    /// The rate the source's frames are played at: the source rate times the
    /// speed, to the hertz. The speed is exactly `played / source`.
    pub played: SampleRate,
    /// Whether the pitch stays (a WSOLA time stretch) or follows the speed.
    pub keep_pitch: bool,
    /// The resampler quality, for the tape kind.
    pub quality: ResamplerQuality,
}

impl SpeedPlan {
    /// The step for `speed` on a source at `source`; `None` at the normal speed.
    pub fn new(
        source: SampleRate,
        speed: PlaybackSpeed,
        quality: ResamplerQuality,
    ) -> Option<Self> {
        if speed.is_normal() {
            return None;
        }
        let rate = speed.rate.clamp(PlaybackSpeed::MIN, PlaybackSpeed::MAX);
        let played = (f64::from(source.hz()) * rate).round() as u32;
        let played = SampleRate::new(played).filter(|p| *p != source)?;
        Some(Self {
            source,
            played,
            keep_pitch: speed.keep_pitch,
            quality,
        })
    }

    /// The speed, as a factor.
    pub fn rate(&self) -> f64 {
        f64::from(self.played.hz()) / f64::from(self.source.hz())
    }
}

impl fmt::Display for SpeedPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if self.keep_pitch {
            "pitch kept"
        } else {
            "tape"
        };
        write!(f, "speed {:.2}× ({kind})", self.rate())
    }
}

/// How a source's frames (track time) and the ring buffer's frames (listening
/// time) convert into each other, for one source on one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeScale {
    source: u32,
    /// The source rate times the speed.
    played: u32,
    device: u32,
    keep_pitch: bool,
}

impl TimeScale {
    /// A source at `source` played at the normal speed on a stream at `device`.
    pub fn new(source: SampleRate, device: SampleRate) -> Self {
        Self {
            source: source.hz(),
            played: source.hz(),
            device: device.hz(),
            keep_pitch: true,
        }
    }

    /// The same source played as `speed` says (the normal speed with `None`).
    #[must_use]
    pub fn with_speed(mut self, speed: Option<&SpeedPlan>) -> Self {
        match speed {
            Some(plan) if plan.source.hz() == self.source => {
                self.played = plan.played.hz();
                self.keep_pitch = plan.keep_pitch;
            }
            _ => {
                self.played = self.source;
                self.keep_pitch = true;
            }
        }
        self
    }

    /// The source rate.
    pub fn source_rate(&self) -> SampleRate {
        SampleRate::new(self.source).unwrap_or(SampleRate::HZ_44100)
    }

    /// The rate of the ring buffer.
    pub fn device_rate(&self) -> SampleRate {
        SampleRate::new(self.device).unwrap_or(SampleRate::HZ_44100)
    }

    /// The speed, as a factor.
    pub fn speed(&self) -> f64 {
        f64::from(self.played) / f64::from(self.source.max(1))
    }

    /// Whether the source plays at the normal speed.
    pub fn is_normal_speed(&self) -> bool {
        self.played == self.source
    }

    /// Whether ring frames are source frames: the normal speed, and no resampling.
    pub fn is_identity(&self) -> bool {
        self.is_normal_speed() && self.source == self.device
    }

    /// `ring` frames of the ring buffer as frames of the source, rounded down.
    pub fn to_source(&self, ring: u64) -> u64 {
        scale_down(ring, self.played, self.device)
    }

    /// `frames` frames of the source as frames of the ring buffer, rounded up.
    pub fn to_ring(&self, frames: u64) -> u64 {
        scale_up(frames, self.device, self.played)
    }

    /// The ring frames the processing chain makes of `frames` source frames,
    /// exactly: one resampling for the tape kind, a stretch at the source rate and
    /// then the resampling for the tempo kind, each rounding up.
    pub fn rendered(&self, frames: u64) -> u64 {
        if self.keep_pitch && self.played != self.source {
            let stretched = scale_up(frames, self.source, self.played);
            scale_up(stretched, self.device, self.source)
        } else {
            scale_up(frames, self.device, self.played)
        }
    }

    /// How long `track` (a stretch of the track's own time) lasts for the listener.
    pub fn listening(&self, track: Duration) -> Duration {
        if self.is_normal_speed() {
            return track;
        }
        track.mul_f64(f64::from(self.source) / f64::from(self.played.max(1)))
    }

    /// How much of the track's own time `listening` (time of the listener) covers.
    pub fn track_time(&self, listening: Duration) -> Duration {
        if self.is_normal_speed() {
            return listening;
        }
        listening.mul_f64(self.speed())
    }
}

/// `frames × to / from`, rounded down.
fn scale_down(frames: u64, to: u32, from: u32) -> u64 {
    if to == from || from == 0 {
        return frames;
    }
    u64::try_from(u128::from(frames) * u128::from(to) / u128::from(from)).unwrap_or(u64::MAX)
}

/// `frames × to / from`, rounded up.
fn scale_up(frames: u64, to: u32, from: u32) -> u64 {
    if to == from || from == 0 {
        return frames;
    }
    u64::try_from((u128::from(frames) * u128::from(to)).div_ceil(u128::from(from)))
        .unwrap_or(u64::MAX)
}

/// Frames handed to the stretcher at once.
const CHUNK: usize = 256;

/// The tempo change that keeps the pitch: a WSOLA time stretch by `1 / speed`
/// at the source rate, with an exact length.
///
/// `n` input frames become `ceil(n × source / played)` output frames: the
/// stretcher's output is aligned with its input (frame `k` of the output comes from
/// around `k × speed` of the input), and at the end of the source the tail is
/// taken from within the source and cut to that length. Neither end is faded, so
/// tracks joined gaplessly stay at their level across the join. Output waits in a queue until the
/// stage after it takes it. Allocates when built, never while processing.
#[derive(Debug, Clone)]
pub struct TempoStage {
    channels: usize,
    source: u32,
    played: u32,
    wsola: Wsola,
    /// One hop of the stretcher's output.
    hop: Box<[f32]>,
    /// Stretched frames waiting: `queue[start..end]`, interleaved.
    queue: Box<[f32]>,
    start: usize,
    end: usize,
    /// Input frames taken, output frames made and output frames taken, since the
    /// last reset.
    frames_in: u64,
    made: u64,
    taken: u64,
    /// The tail has been flushed: no input until the next reset.
    finished: bool,
}

impl TempoStage {
    /// The stage for `plan`, for `channels` channels and input blocks of at most
    /// `max_block` frames.
    pub fn new(plan: &SpeedPlan, channels: usize, max_block: usize) -> Self {
        let channels = channels.max(1);
        let source = plan.source.hz();
        let played = plan.played.hz();
        let rate = f64::from(source);
        let hop = ((rate * 0.010).round() as usize).max(16);
        let tolerance = ((rate * 0.003).round() as usize).max(4);
        let stretch = f64::from(source) / f64::from(played);
        let wsola = Wsola::new(channels, hop, tolerance, stretch, CHUNK);
        // A full block makes at most twice its length (the slowest speed), and the
        // input the stretcher holds back adds a few hops.
        let capacity = 2 * max_block.max(CHUNK) + 2 * tolerance + 10 * hop + 2 * CHUNK;
        Self {
            channels,
            source,
            played,
            wsola,
            hop: vec![0.0; hop * channels].into_boxed_slice(),
            queue: vec![0.0; capacity * channels].into_boxed_slice(),
            start: 0,
            end: 0,
            frames_in: 0,
            made: 0,
            taken: 0,
            finished: false,
        }
    }

    /// The output frames the input so far comes to in all.
    fn target(&self) -> u64 {
        scale_up(self.frames_in, self.source, self.played)
    }

    /// Frames waiting to be taken.
    pub fn pending(&self) -> usize {
        (self.end - self.start) / self.channels
    }

    /// The most frames that can wait at once.
    pub fn capacity(&self) -> usize {
        self.queue.len() / self.channels
    }

    /// Whether everything has been taken after [`finish`](Self::finish).
    pub fn is_done(&self) -> bool {
        self.finished && self.taken >= self.target()
    }

    /// Takes the interleaved frames of `input` (at most the `max_block` the stage
    /// was built for) and stretches them. Only when nothing is waiting.
    pub fn push(&mut self, input: &[f32]) {
        if self.finished {
            self.reset();
        }
        debug_assert_eq!(self.pending(), 0);
        self.start = 0;
        self.end = 0;
        let channels = self.channels;
        for chunk in input.chunks(CHUNK * channels) {
            self.wsola.push(chunk);
            self.frames_in += (chunk.len() / channels) as u64;
            self.pull();
        }
    }

    /// Moves the stretcher's output into the queue.
    fn pull(&mut self) {
        let hop = self.hop.len();
        while self.end + hop <= self.queue.len() && self.wsola.pull(&mut self.hop) {
            self.queue[self.end..self.end + hop].copy_from_slice(&self.hop);
            self.end += hop;
            self.made += (hop / self.channels) as u64;
        }
    }

    /// The end of the source: takes the stretcher's last frames from within the
    /// source (not from silence after it, which would fade the end out) until the
    /// output has its full length. Only when nothing is waiting; the next
    /// [`push`](Self::push) starts a new stream.
    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        debug_assert_eq!(self.pending(), 0);
        self.start = 0;
        self.end = 0;
        self.finished = true;
        self.wsola.finish();
        let hop = self.hop.len();
        while self.made < self.target() && self.end + hop <= self.queue.len() {
            self.wsola.pull(&mut self.hop);
            self.queue[self.end..self.end + hop].copy_from_slice(&self.hop);
            self.end += hop;
            self.made += (hop / self.channels) as u64;
        }
    }

    /// The frames that may be taken now, interleaved: what waits, but never past
    /// the full length of the output.
    pub fn ready(&self) -> &[f32] {
        let allowed = self.target().saturating_sub(self.taken);
        let frames = (self.pending() as u64).min(allowed);
        let n = usize::try_from(frames).unwrap_or(usize::MAX) * self.channels;
        &self.queue[self.start..self.start + n]
    }

    /// Marks `frames` frames of [`ready`](Self::ready) as taken.
    pub fn consume(&mut self, frames: usize) {
        self.start = (self.start + frames * self.channels).min(self.end);
        self.taken += frames as u64;
        if self.is_done() {
            // What is left is past the full length.
            self.start = self.end;
        }
    }

    /// Clears all state, for a seek or a new stream.
    pub fn reset(&mut self) {
        self.wsola.reset();
        self.start = 0;
        self.end = 0;
        self.frames_in = 0;
        self.made = 0;
        self.taken = 0;
        self.finished = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(hz: u32) -> SampleRate {
        SampleRate::new(hz).unwrap()
    }

    fn plan(source: u32, speed: f64, keep_pitch: bool) -> SpeedPlan {
        SpeedPlan::new(
            rate(source),
            PlaybackSpeed {
                rate: speed,
                keep_pitch,
            },
            ResamplerQuality::Medium,
        )
        .unwrap()
    }

    #[test]
    fn the_normal_speed_plans_nothing() {
        let normal = SpeedPlan::new(rate(44_100), PlaybackSpeed::NORMAL, ResamplerQuality::High);
        assert_eq!(normal, None);
        let p = plan(44_100, 1.25, false);
        assert_eq!(p.played.hz(), 55_125);
        assert_eq!(p.rate(), 1.25);
        assert_eq!(p.to_string(), "speed 1.25× (tape)");
        assert_eq!(plan(48_000, 9.0, true).played.hz(), 96_000);
    }

    #[test]
    fn scales_convert_between_track_and_listening_time() {
        let normal = TimeScale::new(rate(44_100), rate(48_000));
        assert_eq!(normal.to_ring(44_100), 48_000);
        assert_eq!(normal.to_source(48_000), 44_100);
        assert!(!normal.is_identity() && normal.is_normal_speed());
        let fast = normal.with_speed(Some(&plan(44_100, 1.5, true)));
        // 1.5 s of the track in one second of the ring.
        assert_eq!(fast.to_source(48_000), 66_150);
        assert_eq!(fast.to_ring(66_150), 48_000);
        assert_eq!(
            fast.listening(Duration::from_secs(3)),
            Duration::from_secs(2)
        );
        assert_eq!(
            fast.track_time(Duration::from_secs(2)),
            Duration::from_secs(3)
        );
        // Tempo: a stretch to 29 400 frames at the source rate, then 32 000.
        assert_eq!(fast.rendered(44_100), 32_000);
        let tape = normal.with_speed(Some(&plan(44_100, 0.75, false)));
        assert_eq!(tape.rendered(44_100), 64_000);
        // A plan for another source rate does not apply.
        assert!(
            TimeScale::new(rate(48_000), rate(48_000))
                .with_speed(Some(&plan(44_100, 2.0, false)))
                .is_identity()
        );
    }

    fn run(stage: &mut TempoStage, input: &[f32], block: usize) -> Vec<f32> {
        let channels = stage.channels;
        let mut out = Vec::new();
        for piece in input.chunks(block * channels) {
            stage.push(piece);
            let n = stage.ready().len();
            out.extend_from_slice(stage.ready());
            stage.consume(n / channels);
        }
        stage.finish();
        let n = stage.ready().len();
        out.extend_from_slice(stage.ready());
        stage.consume(n / channels);
        assert!(stage.is_done());
        out
    }

    #[test]
    fn the_tempo_stage_has_an_exact_length() {
        for (speed, frames) in [(1.5, 10_007usize), (0.5, 4_096), (0.75, 1), (2.0, 48_000)] {
            let p = plan(48_000, speed, true);
            let mut stage = TempoStage::new(&p, 2, 4096);
            let input = vec![0.25f32; frames * 2];
            let out = run(&mut stage, &input, 1000);
            let expected = (frames as u64 * 48_000).div_ceil(u64::from(p.played.hz()));
            assert_eq!(out.len() as u64, expected * 2, "{speed}, {frames}");
            // A new stream starts after the end.
            let again = run(&mut stage, &input, 4096);
            assert_eq!(again.len(), out.len());
        }
    }
}
