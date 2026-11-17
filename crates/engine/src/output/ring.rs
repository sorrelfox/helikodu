// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The ring buffer between the render thread and the device, and the counters
//! both sides share.
//!
//! The ring holds interleaved `f32` samples. The writer only ever adds whole
//! frames, so the reader always finds whole frames. The reader side runs in the
//! real-time context: it copies, fills any shortfall with silence and updates
//! atomic counters, nothing else.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use helikodu_model::audio::SampleRate;
use rtrb::{Consumer, Producer, RingBuffer};

use super::{OutputError, StreamState, StreamTiming};

/// The shortest ring buffer, so that one device period always fits.
const MIN_BUFFER: Duration = Duration::from_millis(20);
/// The longest ring buffer.
const MAX_BUFFER: Duration = Duration::from_secs(5);

/// State and counters shared by a stream's handle, its writer and its device side.
#[derive(Debug)]
pub(crate) struct StreamShared {
    channels: usize,
    rate: SampleRate,
    state: AtomicU8,
    frames_written: AtomicU64,
    frames_delivered: AtomicU64,
    silent_frames: AtomicU64,
    underruns: AtomicU64,
    delay_frames: AtomicU64,
    device_rate: AtomicU32,
    /// Set while draining: an empty ring is the end of the audio, not an underrun.
    draining: AtomicBool,
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
    #[cfg_attr(not(any(feature = "pipewire", feature = "alsa")), allow(dead_code))]
    pub(crate) fn set_delay_frames(&self, frames: u64) {
        self.delay_frames.store(frames, Ordering::Relaxed);
    }

    /// Records the rate the device runs at. Real-time safe.
    #[cfg_attr(not(feature = "pipewire"), allow(dead_code))]
    pub(crate) fn set_device_rate(&self, hz: u32) {
        self.device_rate.store(hz, Ordering::Relaxed);
    }

    /// Counts an underrun the device itself reported. Real-time safe.
    #[cfg_attr(not(feature = "alsa"), allow(dead_code))]
    pub(crate) fn note_device_underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn timing(&self) -> StreamTiming {
        StreamTiming {
            rate: self.rate,
            frames_written: self.frames_written.load(Ordering::Relaxed),
            frames_delivered: self.frames_delivered.load(Ordering::Relaxed),
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

/// Creates a ring buffer of `buffer` length for `channels` interleaved channels at
/// `rate`, and the state shared by its two ends.
pub(crate) fn ring(
    channels: u16,
    rate: SampleRate,
    buffer: Duration,
) -> (SampleWriter, SampleReader, Arc<StreamShared>) {
    let channels = usize::from(channels.max(1));
    let buffer = buffer.clamp(MIN_BUFFER, MAX_BUFFER);
    let frames = (buffer.as_secs_f64() * f64::from(rate.hz())).ceil() as usize;
    let (producer, consumer) = RingBuffer::new(frames.max(1) * channels);
    let shared = Arc::new(StreamShared {
        channels,
        rate,
        state: AtomicU8::new(StreamState::Ready.to_u8()),
        frames_written: AtomicU64::new(0),
        frames_delivered: AtomicU64::new(0),
        silent_frames: AtomicU64::new(0),
        underruns: AtomicU64::new(0),
        delay_frames: AtomicU64::new(0),
        device_rate: AtomicU32::new(0),
        draining: AtomicBool::new(false),
    });
    let writer = SampleWriter {
        producer,
        shared: shared.clone(),
    };
    let reader = SampleReader {
        consumer,
        shared: shared.clone(),
    };
    (writer, reader, shared)
}

/// The render side of a stream's ring buffer.
///
/// Takes interleaved 32-bit float samples, nominally between -1.0 and 1.0, in the
/// canonical (WAVE) channel order of the stream's layout, at the stream's
/// negotiated rate ([`OutputStream::spec`](super::OutputStream::spec)).
#[derive(Debug)]
pub struct SampleWriter {
    producer: Producer<f32>,
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

    /// Writes as many whole frames of `samples` as fit, and returns how many
    /// frames that was. Never blocks. A trailing partial frame is not written.
    pub fn write(&mut self, samples: &[f32]) -> usize {
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
        first.copy_from_slice(&samples[..split]);
        second.copy_from_slice(&samples[split..len]);
        chunk.commit_all();
        self.shared.draining.store(false, Ordering::Release);
        self.shared
            .frames_written
            .fetch_add(frames as u64, Ordering::Relaxed);
        frames
    }

    /// Whether the device side has gone: the stream was stopped or dropped.
    pub fn is_closed(&self) -> bool {
        self.producer.is_abandoned() || self.shared.state().is_finished()
    }
}

/// The device side of a stream's ring buffer. Every method is real-time safe.
#[derive(Debug)]
pub(crate) struct SampleReader {
    consumer: Consumer<f32>,
    shared: Arc<StreamShared>,
}

impl SampleReader {
    /// The number of whole frames waiting in the ring.
    pub(crate) fn available_frames(&self) -> usize {
        self.consumer.slots() / self.shared.channels
    }

    /// Fills `out` (a whole number of frames) from the ring, and the rest with
    /// silence. Returns the number of frames taken from the ring.
    ///
    /// A shortfall counts as one underrun, unless the stream is draining.
    pub(crate) fn read(&mut self, out: &mut [f32]) -> usize {
        let channels = self.shared.channels;
        debug_assert_eq!(out.len() % channels, 0, "partial frame requested");
        let wanted = out.len() / channels;
        let frames = self.available_frames().min(wanted);
        let len = frames * channels;
        if let Ok(chunk) = self.consumer.read_chunk(len) {
            let (first, second) = chunk.as_slices();
            out[..first.len()].copy_from_slice(first);
            out[first.len()..len].copy_from_slice(second);
            chunk.commit_all();
        }
        out[len..].fill(0.0);
        let shared = &self.shared;
        shared
            .frames_delivered
            .fetch_add(frames as u64, Ordering::Relaxed);
        if frames < wanted && !shared.draining.load(Ordering::Acquire) {
            shared.underruns.fetch_add(1, Ordering::Relaxed);
            shared
                .silent_frames
                .fetch_add((wanted - frames) as u64, Ordering::Relaxed);
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_ring(buffer_ms: u64) -> (SampleWriter, SampleReader, Arc<StreamShared>) {
        ring(2, SampleRate::HZ_48000, Duration::from_millis(buffer_ms))
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
}
