// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! A copy of what the device is given, for the analyzer.
//!
//! Each stream opened with an [`AudioTap`] in its [`StreamConfig`](super::StreamConfig)
//! gets a second lock-free ring beside its sample ring. While someone wants the
//! samples ([`AudioTap::set_wanted`]), the device side copies every frame it hands
//! to the device into that ring, after volume, fades and dither: the last stage
//! before the device. It never waits: what does not fit is dropped. The ring is
//! made when the stream opens, so the real-time side neither allocates nor locks.
//!
//! The analyzer reads with [`AudioTap::drain`], from the newest stream.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use helikodu_model::audio::{AudioSpec, ChannelLayout, SampleRate};
use rtrb::{Consumer, Producer, RingBuffer};

use super::convert::s32_to_f32;

/// How much sound the tap's ring holds: the analyzer reads it several times as
/// often.
const TAP_LENGTH: Duration = Duration::from_millis(250);

/// The format of the tapped samples: interleaved `f32` in the stream's channel
/// order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TapFormat {
    /// The rate the device runs at.
    pub rate: SampleRate,
    /// The channels.
    pub layout: ChannelLayout,
}

struct TapStream {
    consumer: Consumer<f32>,
    format: TapFormat,
}

#[derive(Default)]
struct TapShared {
    wanted: AtomicBool,
    /// The newest stream's samples. Only the analyzer and the opening of a stream
    /// take this lock, never the device side.
    stream: Mutex<Option<TapStream>>,
}

/// A copy of the samples the device is given; see the module documentation.
/// Clones share one tap.
#[derive(Clone, Default)]
pub struct AudioTap(Arc<TapShared>);

impl AudioTap {
    /// A tap that nobody wants yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts or stops copying. Off, the device side does nothing but check a flag.
    pub fn set_wanted(&self, wanted: bool) {
        self.0.wanted.store(wanted, Ordering::Release);
    }

    /// Whether samples are copied.
    pub fn is_wanted(&self) -> bool {
        self.0.wanted.load(Ordering::Acquire)
    }

    /// Appends the samples copied since the last call (whole frames) to `out`, and
    /// returns their format; `None` before any stream was opened. Not real-time
    /// safe: it takes a lock the device side never takes.
    pub fn drain(&self, out: &mut Vec<f32>) -> Option<TapFormat> {
        let mut stream = self.0.stream.lock().unwrap_or_else(PoisonError::into_inner);
        let stream = stream.as_mut()?;
        let channels = usize::from(stream.format.layout.channel_count().max(1));
        let available = stream.consumer.slots() / channels * channels;
        if available > 0
            && let Ok(chunk) = stream.consumer.read_chunk(available)
        {
            let (first, second) = chunk.as_slices();
            out.extend_from_slice(first);
            out.extend_from_slice(second);
            chunk.commit_all();
        }
        Some(stream.format)
    }

    /// Drops what has been copied and not read.
    pub fn discard(&self) {
        let mut stream = self.0.stream.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(stream) = stream.as_mut() {
            let available = stream.consumer.slots();
            if let Ok(chunk) = stream.consumer.read_chunk(available) {
                chunk.commit_all();
            }
        }
    }

    /// Makes the ring for a stream of `spec` and returns its writing end, for the
    /// stream's device side. The analyzer reads this stream from now on. Called
    /// when a stream opens, never on the real-time side.
    pub(crate) fn feed(&self, spec: AudioSpec) -> TapFeed {
        let channels = usize::from(spec.channel_count().max(1));
        let frames = (TAP_LENGTH.as_secs_f64() * f64::from(spec.rate.hz())).ceil() as usize;
        let (producer, consumer) = RingBuffer::new(frames.max(1) * channels);
        let format = TapFormat {
            rate: spec.rate,
            layout: spec.layout,
        };
        *self.0.stream.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(TapStream { consumer, format });
        TapFeed {
            producer,
            channels,
            shared: Arc::clone(&self.0),
        }
    }
}

impl PartialEq for AudioTap {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for AudioTap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioTap")
            .field("wanted", &self.is_wanted())
            .finish_non_exhaustive()
    }
}

/// The writing end of a stream's tap, held by its device side. Every method is
/// real-time safe: no allocation, no lock, no wait.
pub(crate) struct TapFeed {
    producer: Producer<f32>,
    channels: usize,
    shared: Arc<TapShared>,
}

impl TapFeed {
    /// The number of whole frames of `len` samples that fit now, or 0 when nobody
    /// wants them.
    fn room(&self, len: usize) -> usize {
        if !self.shared.wanted.load(Ordering::Relaxed) {
            return 0;
        }
        len.min(self.producer.slots()) / self.channels * self.channels
    }

    /// Copies float samples (whole frames); what does not fit is dropped.
    pub(crate) fn push(&mut self, samples: &[f32]) {
        let n = self.room(samples.len());
        if n > 0
            && let Ok(chunk) = self.producer.write_chunk_uninit(n)
        {
            chunk.fill_from_iter(samples[..n].iter().copied());
        }
    }

    /// Copies 32-bit integer words, in two parts (the sample ring's wrap).
    pub(crate) fn push_s32(&mut self, first: &[u32], second: &[u32]) {
        let n = self.room(first.len() + second.len());
        if n > 0
            && let Ok(chunk) = self.producer.write_chunk_uninit(n)
        {
            let words = first.iter().chain(second).take(n);
            chunk.fill_from_iter(words.map(|&w| s32_to_f32(w as i32)));
        }
    }
}

impl fmt::Debug for TapFeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TapFeed")
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use helikodu_model::audio::{Endianness, SampleFormat};

    use super::*;

    fn spec() -> AudioSpec {
        AudioSpec::new(
            SampleFormat::F32(Endianness::NATIVE),
            SampleRate::HZ_48000,
            ChannelLayout::STEREO,
        )
    }

    #[test]
    fn copies_only_while_wanted_and_drops_what_does_not_fit() {
        let tap = AudioTap::new();
        let mut out = Vec::new();
        assert_eq!(tap.drain(&mut out), None);
        let mut feed = tap.feed(spec());
        feed.push(&[0.5, -0.5]);
        assert!(tap.drain(&mut out).is_some());
        assert!(out.is_empty(), "nothing is copied unless wanted");

        tap.set_wanted(true);
        feed.push(&[0.25, -0.25, 0.125]);
        tap.drain(&mut out);
        assert_eq!(out, [0.25, -0.25], "only whole frames");

        // The ring holds 250 ms; more is dropped, not waited for.
        let lots = vec![0.1; 48_000 * 2];
        feed.push(&lots);
        out.clear();
        tap.drain(&mut out);
        assert_eq!(out.len(), 12_000 * 2);

        feed.push_s32(&[1 << 30], &[(-(1i32 << 30)) as u32]);
        out.clear();
        tap.drain(&mut out);
        assert_eq!(out, [0.5, -0.5]);
    }

    #[test]
    fn a_new_stream_replaces_the_old_one() {
        let tap = AudioTap::new();
        tap.set_wanted(true);
        let mut old = tap.feed(spec());
        old.push(&[0.5, 0.5]);
        let mono = AudioSpec::new(
            SampleFormat::F32(Endianness::NATIVE),
            SampleRate::HZ_44100,
            ChannelLayout::MONO,
        );
        let mut new = tap.feed(mono);
        new.push(&[0.75]);
        let mut out = Vec::new();
        let format = tap.drain(&mut out).unwrap();
        assert_eq!(format.layout, ChannelLayout::MONO);
        assert_eq!(out, [0.75]);
        // The old stream's side keeps working; its samples go nowhere.
        old.push(&[0.5, 0.5]);
        tap.discard();
        out.clear();
        tap.drain(&mut out);
        assert!(out.is_empty());
    }
}
