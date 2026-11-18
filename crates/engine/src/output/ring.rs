// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The ring buffer between the render thread and the device, and the counters
//! both sides share.
//!
//! The ring holds interleaved 32-bit words, one per sample. For most streams a word
//! is an `f32` sample. When the device takes 32-bit integers, a word is an `i32`
//! sample instead, because a float carries only 24 bits exactly: integer sources
//! then reach the device unchanged ([`SampleWriter::write_i32`]), and float samples
//! are converted on the way in exactly as the encoder would convert them on the way
//! out. The writer only ever adds whole frames, so the reader always finds whole
//! frames. The reader side runs in the real-time context: it copies, runs the
//! [`DeviceStage`] (volume, fades, dither), fills any shortfall with silence,
//! encodes for the device and updates atomic counters, nothing else. While the
//! device stage leaves the samples untouched, integer words go to the encoder
//! without passing through a float.
//!
//! Beside the samples, a second lock-free queue carries [`DeviceAction`]s from the
//! writer to the reader, each tagged with the ring position at which it applies.
//! Positions count frames written since the stream was opened.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use helikodu_model::audio::{AudioSpec, SampleFormat, SampleRate};
use helikodu_model::player::Volume;
use rtrb::{Consumer, Producer, RingBuffer};

use super::convert::{Encoder, f32_to_s32, s32_to_f32};
use super::device_stage::{DeviceAction, DeviceStage};
use super::tap::{AudioTap, TapFeed};
use super::{OutputError, StreamState, StreamTiming};
use crate::pipeline::SharedVolume;

/// The shortest ring buffer, so that one device period always fits.
const MIN_BUFFER: Duration = Duration::from_millis(20);
/// The longest ring buffer.
const MAX_BUFFER: Duration = Duration::from_secs(5);
/// How many device actions can wait to be applied.
const ACTION_QUEUE: usize = 64;
/// The DoP markers of even and odd frames.
const DOP_MARKERS: [u8; 2] = [0x05, 0xFA];
/// The DSD idle pattern, the byte of DSD silence.
const DSD_IDLE: u8 = 0x69;
/// In [`StreamShared::claimed`]: an action has been scheduled since the device side
/// last claimed frames.
const NEW_ACTION: u64 = 1 << 63;

/// State and counters shared by a stream's handle, its writer and its device side.
#[derive(Debug)]
pub(crate) struct StreamShared {
    channels: usize,
    /// The ring's words are `i32` samples rather than `f32` ones.
    integer: bool,
    rate: SampleRate,
    state: AtomicU8,
    frames_written: AtomicU64,
    frames_delivered: AtomicU64,
    frames_discarded: AtomicU64,
    silent_frames: AtomicU64,
    underruns: AtomicU64,
    delay_frames: AtomicU64,
    device_rate: AtomicU32,
    /// Set while draining: an empty ring is the end of the audio, not an underrun.
    draining: AtomicBool,
    /// The number of device actions the reader has applied.
    actions_applied: AtomicU64,
    /// Whether the device side's fader is shut.
    silent: AtomicBool,
    /// The ring position of the first frame the last fade out silenced completely.
    silent_since: AtomicU64,
    /// The ring position up to which the device side takes frames before it looks
    /// at the scheduled actions again, plus [`NEW_ACTION`] once the writer has
    /// scheduled one since. Tells the writer where an action scheduled now applies
    /// at the earliest.
    claimed: AtomicU64,
    /// The software volume the device side follows.
    volume: SharedVolume,
}

impl StreamShared {
    pub(crate) fn state(&self) -> StreamState {
        StreamState::from_u8(self.state.load(Ordering::Acquire))
    }

    pub(crate) fn set_state(&self, state: StreamState) {
        self.state.store(state.to_u8(), Ordering::Release);
    }

    /// Moves to `state` unless the stream has already ended. Returns whether it
    /// moved.
    pub(crate) fn transition(&self, state: StreamState) -> bool {
        let mut current = self.state.load(Ordering::Acquire);
        loop {
            if StreamState::from_u8(current).is_finished() {
                return false;
            }
            match self.state.compare_exchange_weak(
                current,
                state.to_u8(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }

    pub(crate) fn channels(&self) -> usize {
        self.channels
    }

    pub(crate) fn rate(&self) -> SampleRate {
        self.rate
    }

    /// Records the device delay. Real-time safe.
    #[cfg_attr(
        not(any(
            feature = "pipewire",
            feature = "alsa",
            feature = "pulseaudio",
            feature = "jack"
        )),
        allow(dead_code)
    )]
    pub(crate) fn set_delay_frames(&self, frames: u64) {
        self.delay_frames.store(frames, Ordering::Relaxed);
    }

    /// Records the rate the device runs at. Real-time safe.
    #[cfg_attr(
        not(any(feature = "pipewire", feature = "pulseaudio", feature = "jack")),
        allow(dead_code)
    )]
    pub(crate) fn set_device_rate(&self, hz: u32) {
        self.device_rate.store(hz, Ordering::Relaxed);
    }

    /// Counts an underrun the device itself reported. Real-time safe.
    #[cfg_attr(
        not(any(feature = "alsa", feature = "pulseaudio", feature = "jack")),
        allow(dead_code)
    )]
    pub(crate) fn note_device_underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn timing(&self) -> StreamTiming {
        StreamTiming {
            rate: self.rate,
            frames_written: self.frames_written.load(Ordering::Relaxed),
            frames_delivered: self.frames_delivered.load(Ordering::Relaxed),
            frames_discarded: self.frames_discarded.load(Ordering::Relaxed),
            delay_frames: self.delay_frames.load(Ordering::Relaxed),
            underruns: self.underruns.load(Ordering::Relaxed),
            silent_frames: self.silent_frames.load(Ordering::Relaxed),
            device_rate: SampleRate::new(self.device_rate.load(Ordering::Relaxed)),
        }
    }

    /// Waits until every written frame has been delivered and the device delay
    /// has passed. Fails if the stream ends or `timeout` passes first.
    pub(crate) fn wait_drained(&self, timeout: Duration) -> Result<(), OutputError> {
        let deadline = Instant::now() + timeout;
        self.draining.store(true, Ordering::Release);
        loop {
            let state = self.state();
            if state.is_finished() {
                return Err(OutputError::Closed);
            }
            let timing = self.timing();
            if timing.queued_frames() == 0 {
                // The rest is in the device. Wait for the delay it reports, once.
                let delay = Duration::from_secs_f64(
                    timing.delay_frames as f64 / f64::from(timing.rate.hz()),
                );
                let now = Instant::now();
                if now + delay > deadline {
                    std::thread::sleep(deadline.saturating_duration_since(now));
                    return Err(OutputError::Timeout("the stream to drain"));
                }
                std::thread::sleep(delay);
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(OutputError::Timeout("the stream to drain"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// A device action and the ring position at which it applies.
#[derive(Debug, Clone, Copy)]
struct Scheduled {
    at: u64,
    action: DeviceAction,
}

/// Creates a ring buffer of `buffer` length for the negotiated `spec`, and the
/// state shared by its two ends.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn ring(
    spec: AudioSpec,
    buffer: Duration,
) -> (SampleWriter, SampleReader, Arc<StreamShared>) {
    ring_tapped(spec, buffer, None)
}

/// Like [`ring`], and the device side copies what it hands the device to `tap`
/// while that is wanted.
pub(crate) fn ring_tapped(
    spec: AudioSpec,
    buffer: Duration,
    tap: Option<&AudioTap>,
) -> (SampleWriter, SampleReader, Arc<StreamShared>) {
    let channels = usize::from(spec.channel_count().max(1));
    let rate = spec.rate;
    let buffer = buffer.clamp(MIN_BUFFER, MAX_BUFFER);
    let frames = (buffer.as_secs_f64() * f64::from(rate.hz())).ceil() as usize;
    let (producer, consumer) = RingBuffer::new(frames.max(1) * channels);
    let integer = matches!(spec.format, SampleFormat::S32(_));
    let (actions_in, actions_out) = RingBuffer::new(ACTION_QUEUE);
    let volume = SharedVolume::new(Volume::FULL);
    let shared = Arc::new(StreamShared {
        channels,
        integer,
        rate,
        state: AtomicU8::new(StreamState::Ready.to_u8()),
        frames_written: AtomicU64::new(0),
        frames_delivered: AtomicU64::new(0),
        frames_discarded: AtomicU64::new(0),
        silent_frames: AtomicU64::new(0),
        underruns: AtomicU64::new(0),
        delay_frames: AtomicU64::new(0),
        device_rate: AtomicU32::new(0),
        draining: AtomicBool::new(false),
        actions_applied: AtomicU64::new(0),
        silent: AtomicBool::new(false),
        silent_since: AtomicU64::new(0),
        claimed: AtomicU64::new(0),
        volume: volume.clone(),
    });
    let writer = SampleWriter {
        producer,
        actions: actions_in,
        scheduled: 0,
        scheduled_until: 0,
        discarded_until: 0,
        shared: shared.clone(),
    };
    let reader = SampleReader {
        consumer,
        actions: actions_out,
        stage: DeviceStage::new(spec, volume),
        taken: 0,
        shared: shared.clone(),
        tap: tap.map(|tap| tap.feed(spec)),
        dop: false,
        dop_marker: DOP_MARKERS[0],
    };
    (writer, reader, shared)
}

/// The render side of a stream's ring buffer.
///
/// Takes interleaved 32-bit float samples, nominally between -1.0 and 1.0, in the
/// canonical (WAVE) channel order of the stream's layout, at the stream's
/// negotiated rate ([`OutputStream::spec`](super::OutputStream::spec)). Streams whose
/// device takes 32-bit integers also take integer samples
/// ([`write_i32`](Self::write_i32)) and carry them exactly
/// ([`carries_integers`](Self::carries_integers)).
///
/// The device side applies software volume ([`volume`](Self::volume)), anti-click
/// fades and dither after the ring buffer, so that they take effect within one
/// device period. Until told otherwise with [`schedule`](Self::schedule), the fader
/// is open, the volume full and nothing is dithered: samples reach the device
/// exactly as written.
#[derive(Debug)]
pub struct SampleWriter {
    producer: Producer<u32>,
    actions: Producer<Scheduled>,
    scheduled: u64,
    /// The latest ring position an action was scheduled at.
    scheduled_until: u64,
    /// The latest ring position a [`DeviceAction::Discard`] drops up to.
    discarded_until: u64,
    shared: Arc<StreamShared>,
}

impl SampleWriter {
    /// The number of interleaved channels.
    pub fn channels(&self) -> usize {
        self.shared.channels
    }

    /// The ring buffer's size in frames.
    pub fn capacity_frames(&self) -> usize {
        self.producer.buffer().capacity() / self.shared.channels
    }

    /// The number of frames that can be written now without blocking.
    pub fn free_frames(&self) -> usize {
        self.producer.slots() / self.shared.channels
    }

    /// The frames written since the stream was opened: the ring position of the
    /// next frame.
    pub fn frames_written(&self) -> u64 {
        self.shared.frames_written.load(Ordering::Relaxed)
    }

    /// Whether the ring holds 32-bit integer samples, so that
    /// [`write_i32`](Self::write_i32) reaches the device exactly. True when the
    /// negotiated device format is 32-bit integer.
    pub fn carries_integers(&self) -> bool {
        self.shared.integer
    }

    /// Writes as many whole frames of `samples` as fit, and returns how many
    /// frames that was. Never blocks. A trailing partial frame is not written.
    pub fn write(&mut self, samples: &[f32]) -> usize {
        if self.shared.integer {
            self.write_words(samples, |x| f32_to_s32(x) as u32)
        } else {
            self.write_words(samples, f32::to_bits)
        }
    }

    /// Writes as many whole frames of 32-bit integer samples (full scale at 2^31) as
    /// fit, and returns how many frames that was. Never blocks. Exact when the ring
    /// [carries integers](Self::carries_integers); otherwise the samples become
    /// floats, exact for up to 24 significant bits.
    pub fn write_i32(&mut self, samples: &[i32]) -> usize {
        if self.shared.integer {
            self.write_words(samples, |v| v as u32)
        } else {
            self.write_words(samples, |v| s32_to_f32(v).to_bits())
        }
    }

    fn write_words<T: Copy>(&mut self, samples: &[T], word: impl Fn(T) -> u32) -> usize {
        let channels = self.shared.channels;
        let frames = (samples.len() / channels).min(self.free_frames());
        if frames == 0 {
            return 0;
        }
        let len = frames * channels;
        let Ok(mut chunk) = self.producer.write_chunk(len) else {
            return 0;
        };
        let (first, second) = chunk.as_mut_slices();
        let split = first.len();
        for (dst, src) in first.iter_mut().zip(&samples[..split]) {
            *dst = word(*src);
        }
        for (dst, src) in second.iter_mut().zip(&samples[split..len]) {
            *dst = word(*src);
        }
        chunk.commit_all();
        self.shared.draining.store(false, Ordering::Release);
        self.shared
            .frames_written
            .fetch_add(frames as u64, Ordering::Relaxed);
        frames
    }

    /// Says that nothing more will be written for now: an empty ring is the end of
    /// the audio, not an underrun. Writing again ends this.
    pub fn end_of_audio(&self) {
        self.shared.draining.store(true, Ordering::Release);
    }

    /// Has the device side carry out `action` when it reaches the ring position
    /// `at` (in frames written since the stream was opened). Actions apply in the
    /// order they were scheduled; one whose position has passed applies at once.
    ///
    /// Returns `false` when too many actions are waiting; the device side applies
    /// them once per period, so this means the device is not running.
    #[must_use]
    pub fn schedule(&mut self, at: u64, action: DeviceAction) -> bool {
        if self.actions.push(Scheduled { at, action }).is_err() {
            return false;
        }
        self.scheduled += 1;
        self.scheduled_until = self.scheduled_until.max(at);
        if let DeviceAction::Discard { until, .. } = action {
            self.discarded_until = self.discarded_until.max(until);
        }
        true
    }

    /// Like [`schedule`](Self::schedule), and returns the ring position at which the
    /// device side will apply `action`: `at`, or later when the device has taken
    /// the frame at `at` already, or an action scheduled before holds it back.
    /// `None` when too many actions are waiting.
    pub fn schedule_where(&mut self, at: u64, action: DeviceAction) -> Option<u64> {
        let before = self.scheduled_until.max(self.discarded_until);
        if !self.schedule(at, action) {
            return None;
        }
        // The device side takes the frames it has claimed before it looks at the
        // actions again; from now on it looks before it claims more.
        let claimed = self.shared.claimed.fetch_or(NEW_ACTION, Ordering::AcqRel) & !NEW_ACTION;
        Some(at.max(before).max(claimed))
    }

    /// The latest ring position an action has been scheduled at. Actions apply in
    /// the order they were scheduled, so one scheduled now for an earlier position
    /// applies only there.
    pub fn scheduled_until(&self) -> u64 {
        self.scheduled_until
    }

    /// Whether the device side has applied every scheduled action.
    pub fn is_settled(&self) -> bool {
        self.shared.actions_applied.load(Ordering::Acquire) >= self.scheduled
    }

    /// Whether every scheduled action is applied and the fader is shut: a fade out
    /// has finished and the device hears silence.
    pub fn is_silent(&self) -> bool {
        self.is_settled() && self.shared.silent.load(Ordering::Acquire)
    }

    /// The ring position of the first frame that the last fade out silenced
    /// completely: where playback should continue after it.
    pub fn silent_since(&self) -> u64 {
        self.shared.silent_since.load(Ordering::Acquire)
    }

    /// The software volume the device side applies. Full volume leaves the samples
    /// untouched; changes are ramped.
    pub fn volume(&self) -> &SharedVolume {
        &self.shared.volume
    }

    /// Whether the device side has gone: the stream was stopped or dropped.
    pub fn is_closed(&self) -> bool {
        self.producer.is_abandoned() || self.shared.state().is_finished()
    }
}

/// The device side of a stream's ring buffer. Every method is real-time safe.
#[derive(Debug)]
pub(crate) struct SampleReader {
    consumer: Consumer<u32>,
    actions: Consumer<Scheduled>,
    stage: DeviceStage,
    /// The ring position of the next frame: frames delivered plus discarded.
    taken: u64,
    shared: Arc<StreamShared>,
    /// Where a copy of the delivered frames goes, for the analyzer.
    tap: Option<TapFeed>,
    /// The frames are DSD over PCM ([`DeviceAction::Dop`]).
    dop: bool,
    /// The DoP marker the next frame carries.
    dop_marker: u8,
}

impl SampleReader {
    /// The number of whole frames waiting in the ring.
    pub(crate) fn available_frames(&self) -> usize {
        self.consumer.slots() / self.shared.channels
    }

    /// Fills `out` (a whole number of frames) from the ring, and the rest with
    /// silence. Returns the number of frames taken from the ring.
    ///
    /// A shortfall counts as one underrun, unless the stream is draining or the
    /// device side is silent on purpose (faded out, or holding for a refill).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn read(&mut self, out: &mut [f32]) -> usize {
        self.read_into(out, None)
    }

    /// Like [`read`](Self::read), and encodes the frames for the device into the
    /// start of `bytes` (as many frames as `scratch` holds). `scratch` is working
    /// room; integer samples that nothing changes skip it and reach `bytes` exactly.
    pub(crate) fn read_encoded(
        &mut self,
        scratch: &mut [f32],
        encoder: &Encoder,
        bytes: &mut [u8],
    ) -> usize {
        self.read_into(scratch, Some(&mut Sink { encoder, bytes }))
    }

    /// Like [`read_available`](Self::read_available), and encodes the frames taken
    /// into the start of `bytes`.
    pub(crate) fn read_available_encoded(
        &mut self,
        scratch: &mut [f32],
        encoder: &Encoder,
        bytes: &mut [u8],
    ) -> usize {
        self.take(scratch, Some(&mut Sink { encoder, bytes }))
    }

    fn read_into(&mut self, out: &mut [f32], mut sink: Option<&mut Sink<'_>>) -> usize {
        let done = self.take(out, sink.as_deref_mut());
        let channels = self.shared.channels;
        let wanted = out.len() / channels;
        if done < wanted && self.dop {
            // DSD silence keeps the DAC in DSD mode where PCM zeros would not.
            let rest = &mut out[done * channels..];
            self.dop_idle(rest);
            if let Some(sink) = sink {
                sink.encode(done, rest);
            }
            self.publish();
            let quiet = self.stage.is_held() || self.stage.is_silent();
            let shared = &self.shared;
            if !quiet && !shared.draining.load(Ordering::Acquire) {
                shared.underruns.fetch_add(1, Ordering::Relaxed);
                shared
                    .silent_frames
                    .fetch_add((wanted - done) as u64, Ordering::Relaxed);
            }
        } else if done < wanted {
            let rest = &mut out[done * channels..];
            rest.fill(0.0);
            // The fader moves on through the silence, so a fade out completes even
            // when the ring has run dry.
            self.note_silence_start(wanted - done);
            self.stage.apply(rest);
            if let Some(sink) = sink {
                sink.encode(done, rest);
            }
            self.publish();
            let quiet = self.stage.is_held() || self.stage.is_silent();
            let shared = &self.shared;
            if !quiet && !shared.draining.load(Ordering::Acquire) {
                shared.underruns.fetch_add(1, Ordering::Relaxed);
                shared
                    .silent_frames
                    .fetch_add((wanted - done) as u64, Ordering::Relaxed);
            }
        }
        done
    }

    /// Fills the start of `out` with as many frames as the ring holds, without
    /// padding, and returns their number. For sinks that do not keep time.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn read_available(&mut self, out: &mut [f32]) -> usize {
        self.take(out, None)
    }

    /// Takes frames into the start of `out`, applying the scheduled actions on the
    /// way, until `out` is full, the ring is empty or the device side holds.
    fn take(&mut self, out: &mut [f32], mut sink: Option<&mut Sink<'_>>) -> usize {
        let channels = self.shared.channels;
        debug_assert_eq!(out.len() % channels, 0, "partial frame requested");
        let wanted = out.len() / channels;
        let mut done = 0;
        loop {
            self.apply_due_actions();
            if done == wanted || self.stage.is_held() {
                break;
            }
            let mut frames = (wanted - done).min(self.available_frames());
            if let Ok(next) = self.actions.peek() {
                // Stop at the next action's position, so it applies to the right
                // frame.
                let until =
                    usize::try_from(next.at.saturating_sub(self.taken)).unwrap_or(usize::MAX);
                frames = frames.min(until);
            }
            if frames == 0 {
                break;
            }
            if !self.claim(frames) {
                // An action came: look at it first.
                continue;
            }
            let block = &mut out[done * channels..(done + frames) * channels];
            // Integers that nothing changes go to the encoder as they are. DoP
            // frames are checked one by one instead.
            let exact =
                self.shared.integer && sink.is_some() && !self.dop && self.stage.is_transparent();
            if let Ok(chunk) = self.consumer.read_chunk(block.len()) {
                let (first, second) = chunk.as_slices();
                match sink.as_deref_mut() {
                    Some(sink) if exact => {
                        sink.encode_s32(done, first, second);
                        if let Some(tap) = &mut self.tap {
                            tap.push_s32(first, second);
                        }
                    }
                    _ => {
                        let (a, b) = block.split_at_mut(first.len());
                        if self.shared.integer {
                            words_to_f32(first, a, |w| s32_to_f32(w as i32));
                            words_to_f32(second, b, |w| s32_to_f32(w as i32));
                        } else {
                            words_to_f32(first, a, f32::from_bits);
                            words_to_f32(second, b, f32::from_bits);
                        }
                    }
                }
                chunk.commit_all();
            }
            self.note_silence_start(frames);
            if self.dop {
                self.dop_frames(block);
                if let Some(sink) = sink.as_deref_mut() {
                    sink.encode(done, block);
                }
                if let Some(tap) = &mut self.tap {
                    tap.push(block);
                }
            } else if !exact {
                self.stage.apply(block);
                if let Some(sink) = sink.as_deref_mut() {
                    sink.encode(done, block);
                }
                if let Some(tap) = &mut self.tap {
                    tap.push(block);
                }
            }
            self.taken += frames as u64;
            done += frames;
            self.publish();
        }
        self.shared
            .frames_delivered
            .fetch_add(done as u64, Ordering::Relaxed);
        done
    }

    /// Claims the next `frames` frames for taking, so that an action scheduled from
    /// now on applies after them at the earliest. Fails, claiming nothing, when an
    /// action has been scheduled since the last claim: it may apply within them.
    fn claim(&mut self, frames: usize) -> bool {
        let end = self.taken + frames as u64;
        let claimed = &self.shared.claimed;
        let mut current = claimed.load(Ordering::Acquire);
        loop {
            let fresh = current & NEW_ACTION == 0;
            let next = if fresh { end } else { self.taken };
            match claimed.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return fresh,
                Err(actual) => current = actual,
            }
        }
    }

    /// Applies the actions whose position has been reached.
    fn apply_due_actions(&mut self) {
        while let Ok(next) = self.actions.peek() {
            if next.at > self.taken {
                break;
            }
            let mut action = next.action;
            let _ = self.actions.pop();
            match action {
                DeviceAction::Discard { until, .. } => self.discard(until),
                DeviceAction::Dop(on) => self.dop = on,
                // A fade would change the DSD bits: DoP is cut instead.
                DeviceAction::Fade { open, .. } if self.dop => {
                    action = DeviceAction::SetOpen(open);
                }
                _ => {}
            }
            let was_silent = self.stage.is_silent();
            self.stage.act(action);
            if !was_silent && self.stage.is_silent() {
                self.shared
                    .silent_since
                    .store(self.taken, Ordering::Release);
            }
            self.publish();
            self.shared.actions_applied.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Drops the frames up to the ring position `until`.
    fn discard(&mut self, until: u64) {
        let wanted = usize::try_from(until.saturating_sub(self.taken)).unwrap_or(usize::MAX);
        let frames = wanted.min(self.available_frames());
        if frames == 0 {
            return;
        }
        if let Ok(chunk) = self.consumer.read_chunk(frames * self.shared.channels) {
            chunk.commit_all();
        }
        self.taken += frames as u64;
        self.shared
            .frames_discarded
            .fetch_add(frames as u64, Ordering::Relaxed);
    }

    /// Fills `block` with the DoP idle pattern: DSD silence under markers that go
    /// on alternating from the last frame played.
    fn dop_idle(&mut self, block: &mut [f32]) {
        let channels = self.shared.channels;
        for frame in block.chunks_exact_mut(channels) {
            let word = i32::from_be_bytes([self.dop_marker, DSD_IDLE, DSD_IDLE, 0]);
            frame.fill(s32_to_f32(word));
            self.dop_marker = other_marker(self.dop_marker);
        }
    }

    /// Passes the DoP frames of `block` while the fader is open, and replaces the
    /// rest, and any frame without a DoP marker (the silence before the first
    /// frame of a stream), by the idle pattern. The DSD bits are never touched;
    /// only a marker is changed where frames were left out (a pause, a seek), so
    /// that the markers keep alternating.
    fn dop_frames(&mut self, block: &mut [f32]) {
        let channels = self.shared.channels;
        let open = self.stage.is_open() && !self.stage.is_held();
        for frame in block.chunks_exact_mut(channels) {
            let marker = (f32_to_s32(frame[0]) >> 24) as u8;
            if !open || !DOP_MARKERS.contains(&marker) {
                self.dop_idle(frame);
                continue;
            }
            if marker != self.dop_marker {
                for sample in frame.iter_mut() {
                    let bits = (f32_to_s32(*sample) & 0x00FF_FFFF) as u32;
                    let word = (u32::from(self.dop_marker) << 24) | bits;
                    *sample = s32_to_f32(word as i32);
                }
            }
            self.dop_marker = other_marker(self.dop_marker);
        }
    }

    /// Records where a fade out ends if it ends within the next `frames` frames.
    fn note_silence_start(&self, frames: usize) {
        if let Some(left) = self.stage.frames_to_silence()
            && left <= frames
        {
            self.shared
                .silent_since
                .store(self.taken + left as u64, Ordering::Release);
        }
    }

    fn publish(&self) {
        self.shared
            .silent
            .store(self.stage.is_silent(), Ordering::Release);
    }
}

/// The DoP marker that follows `marker`.
fn other_marker(marker: u8) -> u8 {
    if marker == DOP_MARKERS[0] {
        DOP_MARKERS[1]
    } else {
        DOP_MARKERS[0]
    }
}

/// Where encoded frames go: the encoder and the device's bytes, from frame 0.
struct Sink<'a> {
    encoder: &'a Encoder,
    bytes: &'a mut [u8],
}

impl Sink<'_> {
    /// Encodes float frames starting at frame `at` of the output.
    fn encode(&mut self, at: usize, samples: &[f32]) {
        let start = at * self.encoder.bytes_per_frame();
        if let Some(out) = self.bytes.get_mut(start..) {
            self.encoder.encode(samples, out);
        }
    }

    /// Encodes integer words, in two parts (the ring's wrap), starting at frame `at`.
    fn encode_s32(&mut self, at: usize, first: &[u32], second: &[u32]) {
        let start = at * self.encoder.bytes_per_frame();
        if let Some(out) = self.bytes.get_mut(start..) {
            let split = self.encoder.encode_s32(first, out);
            if let Some(out) = out.get_mut(split..) {
                self.encoder.encode_s32(second, out);
            }
        }
    }
}

fn words_to_f32(words: &[u32], out: &mut [f32], sample: impl Fn(u32) -> f32) {
    for (dst, word) in out.iter_mut().zip(words) {
        *dst = sample(*word);
    }
}

#[cfg(test)]
mod tests {
    use helikodu_model::audio::{ChannelLayout, Endianness, SampleFormat};

    use super::*;

    fn stereo_ring(buffer_ms: u64) -> (SampleWriter, SampleReader, Arc<StreamShared>) {
        let spec = AudioSpec::new(
            SampleFormat::F32(Endianness::NATIVE),
            SampleRate::HZ_48000,
            ChannelLayout::STEREO,
        );
        ring(spec, Duration::from_millis(buffer_ms))
    }

    #[test]
    fn capacity_follows_the_buffer_length() {
        let (writer, _, _) = stereo_ring(200);
        assert_eq!(writer.capacity_frames(), 9600);
        assert_eq!(writer.free_frames(), 9600);
        // Clamped to the minimum.
        let (writer, _, _) = stereo_ring(1);
        assert_eq!(writer.capacity_frames(), 960);
    }

    #[test]
    fn copies_whole_frames_in_order_across_the_wrap() {
        let (mut writer, mut reader, shared) = stereo_ring(20);
        let mut next = 0.0f32;
        let mut expected = 0.0f32;
        let mut out = vec![0.0; 700 * 2];
        // 700 of 960 frames per round, so the positions wrap around.
        for _ in 0..10 {
            let samples: Vec<f32> = (0..1400)
                .map(|_| {
                    next += 1.0;
                    next
                })
                .collect();
            assert_eq!(writer.write(&samples), 700);
            assert_eq!(reader.read(&mut out), 700);
            for value in &out {
                expected += 1.0;
                assert_eq!(*value, expected);
            }
        }
        let timing = shared.timing();
        assert_eq!(timing.frames_written, 7000);
        assert_eq!(timing.frames_delivered, 7000);
        assert_eq!(timing.underruns, 0);
    }

    #[test]
    fn the_tap_gets_what_the_device_gets() {
        let spec = AudioSpec::new(
            SampleFormat::F32(Endianness::NATIVE),
            SampleRate::HZ_48000,
            ChannelLayout::STEREO,
        );
        let tap = AudioTap::new();
        let (mut writer, mut reader, _) = ring_tapped(spec, Duration::from_millis(20), Some(&tap));
        let mut out = vec![0.0; 8];
        writer.write(&[0.5; 8]);
        reader.read(&mut out);
        let mut copied = Vec::new();
        tap.drain(&mut copied);
        assert!(copied.is_empty(), "not copied until wanted");
        tap.set_wanted(true);
        writer.write(&[0.1, 0.2, 0.3, 0.4]);
        reader.read(&mut out);
        tap.drain(&mut copied);
        // Only the frames taken from the ring, not the silence after them.
        assert_eq!(copied, [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(&out[..4], &copied[..]);
    }

    #[test]
    fn partial_frames_are_not_written() {
        let (mut writer, mut reader, _) = stereo_ring(20);
        assert_eq!(writer.write(&[0.5, 0.5, 0.25]), 1);
        let mut out = [9.0; 4];
        assert_eq!(reader.read(&mut out), 1);
        assert_eq!(out, [0.5, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn write_stops_when_full() {
        let (mut writer, _, shared) = stereo_ring(20);
        let samples = vec![0.1; 2000 * 2];
        assert_eq!(writer.write(&samples), 960);
        assert_eq!(writer.write(&samples), 0);
        assert_eq!(writer.free_frames(), 0);
        assert_eq!(shared.timing().frames_written, 960);
    }

    #[test]
    fn shortfall_is_silence_and_one_underrun() {
        let (mut writer, mut reader, shared) = stereo_ring(20);
        writer.write(&[0.5; 20]);
        let mut out = [1.0; 32];
        assert_eq!(reader.read(&mut out), 10);
        assert!(out[..20].iter().all(|v| *v == 0.5));
        assert!(out[20..].iter().all(|v| *v == 0.0));
        assert_eq!(reader.read(&mut out), 0);
        let timing = shared.timing();
        assert_eq!(timing.underruns, 2);
        assert_eq!(timing.silent_frames, 6 + 16);
        assert_eq!(timing.frames_delivered, 10);
    }

    #[test]
    fn no_underrun_while_draining() {
        let (mut writer, mut reader, shared) = stereo_ring(20);
        shared.set_state(StreamState::Running);
        writer.write(&[0.5; 8]);
        let waiter = std::thread::spawn({
            let shared = shared.clone();
            move || shared.wait_drained(Duration::from_secs(5))
        });
        while !shared.draining.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        let mut out = [0.0; 16];
        reader.read(&mut out);
        reader.read(&mut out);
        waiter.join().unwrap().unwrap();
        assert_eq!(shared.timing().underruns, 0);
        // Writing again ends draining.
        writer.write(&[0.5; 2]);
        reader.read(&mut out);
        assert_eq!(shared.timing().underruns, 1);
    }

    #[test]
    fn drain_times_out_when_nothing_is_read() {
        let (mut writer, _reader, shared) = stereo_ring(20);
        writer.write(&[0.5; 8]);
        let result = shared.wait_drained(Duration::from_millis(20));
        assert!(matches!(result, Err(OutputError::Timeout(_))));
    }

    #[test]
    fn finished_states_are_final() {
        let (_, _, shared) = stereo_ring(20);
        assert!(shared.transition(StreamState::Running));
        shared.set_state(StreamState::Lost);
        assert!(!shared.transition(StreamState::Running));
        assert_eq!(shared.state(), StreamState::Lost);
    }

    #[test]
    fn writer_sees_a_dropped_reader() {
        let (writer, reader, _) = stereo_ring(20);
        assert!(!writer.is_closed());
        drop(reader);
        assert!(writer.is_closed());
    }

    #[test]
    fn actions_apply_at_their_ring_position() {
        let (mut writer, mut reader, _) = stereo_ring(20);
        writer.write(&[1.0; 2 * 100]);
        assert!(writer.schedule(40, DeviceAction::SetOpen(false)));
        assert!(writer.schedule(60, DeviceAction::SetOpen(true)));
        assert!(!writer.is_settled());
        let mut out = [9.0; 2 * 100];
        assert_eq!(reader.read(&mut out), 100);
        assert!(out[..80].iter().all(|v| *v == 1.0));
        assert!(out[80..120].iter().all(|v| *v == 0.0));
        assert!(out[120..].iter().all(|v| *v == 1.0));
        assert!(writer.is_settled());
        assert!(!writer.is_silent());
        assert_eq!(writer.silent_since(), 40);
    }

    #[test]
    fn a_fade_out_reports_where_silence_starts() {
        let (mut writer, mut reader, _) = stereo_ring(20);
        writer.write(&[0.5; 2 * 900]);
        let fade = DeviceAction::Fade {
            open: false,
            duration: Duration::from_millis(10),
        };
        assert!(writer.schedule(100, fade));
        let mut out = [0.0; 2 * 300];
        reader.read(&mut out);
        assert!(!writer.is_silent());
        reader.read(&mut out);
        assert!(writer.is_silent());
        assert_eq!(writer.silent_since(), 100 + 480);
    }

    #[test]
    fn discard_drops_frames_and_holds_for_a_refill() {
        let (mut writer, mut reader, shared) = stereo_ring(20);
        writer.write(&[0.5; 2 * 300]);
        assert!(writer.schedule(0, DeviceAction::SetOpen(false)));
        let until = writer.frames_written();
        assert!(writer.schedule(0, DeviceAction::Discard { until, hold: true }));
        let mut out = [9.0; 2 * 50];
        assert_eq!(reader.read(&mut out), 0);
        assert!(out.iter().all(|v| *v == 0.0));
        // Holding: new frames wait for the fade in, and silence is no underrun.
        writer.write(&[0.25; 2 * 100]);
        assert_eq!(reader.read(&mut out), 0);
        assert!(writer.schedule(until, DeviceAction::SetOpen(true)));
        assert_eq!(reader.read(&mut out), 50);
        assert!(out.iter().all(|v| *v == 0.25));
        let timing = shared.timing();
        assert_eq!(timing.frames_discarded, 300);
        assert_eq!(timing.frames_delivered, 50);
        assert_eq!(timing.underruns, 0);
        assert_eq!(timing.queued_frames(), 50);
    }

    #[test]
    fn an_action_says_where_it_applies() {
        let (mut writer, mut reader, shared) = stereo_ring(20);
        writer.write(&[0.5; 2 * 400]);
        // Ahead of the device side: where it was asked for.
        assert_eq!(
            writer.schedule_where(30, DeviceAction::AlwaysDither(false)),
            Some(30)
        );
        let mut out = [0.0; 2 * 100];
        assert_eq!(reader.read(&mut out), 100);
        // Behind it: where the device side stands, and that is where it applies.
        let until = writer.frames_written();
        let action = DeviceAction::Discard { until, hold: false };
        assert_eq!(writer.schedule_where(50, action), Some(100));
        // Another one waits behind the discard.
        assert_eq!(
            writer.schedule_where(60, DeviceAction::AlwaysDither(false)),
            Some(until)
        );
        assert_eq!(reader.read(&mut out), 0);
        assert_eq!(shared.timing().frames_discarded, 300);
    }

    #[test]
    fn a_32_bit_ring_carries_integers_exactly() {
        let spec = AudioSpec::new(
            SampleFormat::S32(Endianness::Little),
            SampleRate::HZ_48000,
            ChannelLayout::STEREO,
        );
        let (mut writer, mut reader, _) = ring(spec, Duration::from_millis(20));
        assert!(writer.carries_integers());
        let encoder = Encoder::new(spec.format, 2).unwrap();
        let words: Vec<i32> = (0..2 * 700i32)
            .map(|i| 0x7654_3211i32.wrapping_sub(i.wrapping_mul(0x0101_0101)))
            .collect();
        let mut scratch = vec![0.0f32; 2 * 300];
        let mut bytes = vec![0u8; 8 * 300];
        let mut out = Vec::new();
        // Across the wrap, in reads of 300 frames.
        for chunk in words.chunks(2 * 350) {
            assert_eq!(writer.write_i32(chunk), 350);
            for _ in 0..2 {
                let frames = reader.read_available_encoded(&mut scratch, &encoder, &mut bytes);
                out.extend_from_slice(&bytes[..frames * 8]);
            }
        }
        let back: Vec<i32> = out
            .chunks(4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        assert_eq!(back, words);
        // Floats become the integers the encoder would make of them.
        writer.write(&[0.5, -1.0]);
        assert_eq!(
            reader.read_encoded(&mut scratch[..2], &encoder, &mut bytes),
            1
        );
        assert_eq!(i32::from_le_bytes(bytes[..4].try_into().unwrap()), 1 << 30);
        assert_eq!(
            i32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            i32::MIN
        );
        // A fade goes through floats, and the shortfall is silence.
        writer.write_i32(&[1 << 30, 1 << 30]);
        assert!(writer.schedule(0, DeviceAction::SetOpen(false)));
        assert_eq!(
            reader.read_encoded(&mut scratch[..4], &encoder, &mut bytes),
            1
        );
        assert!(bytes[..16].iter().all(|b| *b == 0));
        // A float ring takes integers as floats.
        let (mut writer, mut reader, _) = stereo_ring(20);
        assert!(!writer.carries_integers());
        writer.write_i32(&[1 << 30, i32::MIN]);
        let mut out = [0.0f32; 2];
        reader.read(&mut out);
        assert_eq!(out, [0.5, -1.0]);
    }

    #[test]
    fn read_available_never_pads() {
        let (mut writer, mut reader, shared) = stereo_ring(20);
        writer.write(&[0.5; 2 * 10]);
        let mut out = [9.0; 2 * 50];
        assert_eq!(reader.read_available(&mut out), 10);
        assert_eq!(out[20], 9.0);
        assert_eq!(shared.timing().underruns, 0);
    }
}
