// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Streams whose "device" is a thread of ours: the null and WAV sinks.
//!
//! The thread takes the place of a device's clock. It pulls one period at a time
//! from the ring buffer, encodes it and hands the bytes to a [`SinkTarget`].

use std::io;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helikodu_model::audio::{AudioSpec, OutputBackendKind};

use super::convert::Encoder;
use super::ring::{SampleReader, StreamShared, ring};
use super::{OpenedStream, OutputError, OutputStream, StreamConfig, StreamState, StreamTiming};

/// The length of one period of a software sink.
const PERIOD: Duration = Duration::from_millis(10);
/// How long an idle sink thread sleeps before it looks at its state again.
const IDLE_WAIT: Duration = Duration::from_millis(50);
/// A real-time sink that falls further behind than this skips ahead instead of
/// catching up in a burst.
const MAX_LAG: Duration = Duration::from_millis(100);

/// How a software sink takes samples from the ring buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pacing {
    /// Like a device: one period every period length, with silence and an underrun
    /// when the ring buffer is short.
    #[default]
    RealTime,
    /// As fast as samples arrive, never inserting silence. For tests and for
    /// writing files faster than real time.
    Freewheel,
}

/// Where a software sink's bytes go.
pub(crate) trait SinkTarget: Send + 'static {
    /// Takes encoded frames.
    fn write(&mut self, bytes: &[u8]) -> io::Result<()>;
    /// Called once when the stream stops normally.
    fn finish(&mut self) -> io::Result<()>;
}

/// A stream served by a sink thread.
#[derive(Debug)]
pub(crate) struct SoftwareStream {
    backend: OutputBackendKind,
    spec: AudioSpec,
    shared: Arc<StreamShared>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl SoftwareStream {
    /// Opens a stream that writes `spec` to `target` on a new thread.
    ///
    /// `spec` is the negotiated format; its rate and channel count size the ring
    /// buffer.
    pub(crate) fn open(
        backend: OutputBackendKind,
        config: &StreamConfig,
        spec: AudioSpec,
        pacing: Pacing,
        target: impl SinkTarget,
    ) -> Result<OpenedStream, OutputError> {
        let channels = usize::from(spec.channel_count());
        let encoder =
            Encoder::new(spec.format, channels).ok_or_else(|| OutputError::FormatUnsupported {
                backend,
                device: config.device.clone(),
                reason: format!("{} cannot be written", spec.format),
            })?;
        let (writer, reader, shared) = ring(spec, config.buffer);
        let worker = Worker {
            reader,
            shared: shared.clone(),
            encoder,
            pacing,
            period_frames: period_frames(spec),
        };
        let thread = thread::Builder::new()
            .name(format!("helikodu-{backend}-sink"))
            .spawn(move || worker.run(target))?;
        let stream = Self {
            backend,
            spec,
            shared,
            thread: Some(thread),
        };
        Ok(OpenedStream {
            stream: Box::new(stream),
            writer,
        })
    }

    fn wake(&self) {
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }
}

fn period_frames(spec: AudioSpec) -> usize {
    let frames = PERIOD.as_secs_f64() * f64::from(spec.rate.hz());
    (frames.ceil() as usize).max(1)
}

impl OutputStream for SoftwareStream {
    fn backend(&self) -> OutputBackendKind {
        self.backend
    }

    fn spec(&self) -> AudioSpec {
        self.spec
    }

    fn state(&self) -> StreamState {
        self.shared.state()
    }

    fn timing(&self) -> StreamTiming {
        self.shared.timing()
    }

    fn start(&mut self) -> Result<(), OutputError> {
        if !self.shared.transition(StreamState::Running) {
            return Err(OutputError::Closed);
        }
        self.wake();
        Ok(())
    }

    fn pause(&mut self) -> Result<(), OutputError> {
        if !self.shared.transition(StreamState::Paused) {
            return Err(OutputError::Closed);
        }
        Ok(())
    }

    fn drain(&mut self, timeout: Duration) -> Result<(), OutputError> {
        self.shared.wait_drained(timeout)
    }

    fn stop(&mut self) -> Result<(), OutputError> {
        let failed = self.shared.state() == StreamState::Failed;
        self.shared.set_state(StreamState::Closed);
        self.wake();
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        let result = thread
            .join()
            .map_err(|_| OutputError::Backend {
                backend: self.backend,
                reason: "the sink thread panicked".into(),
            })?
            .map_err(OutputError::Io);
        if failed && result.is_ok() {
            return Err(OutputError::Closed);
        }
        result
    }
}

impl Drop for SoftwareStream {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct Worker {
    reader: SampleReader,
    shared: Arc<StreamShared>,
    encoder: Encoder,
    pacing: Pacing,
    period_frames: usize,
}

impl Worker {
    fn run(mut self, mut target: impl SinkTarget) -> io::Result<()> {
        let channels = self.shared.channels();
        let mut samples = vec![0.0f32; self.period_frames * channels];
        let mut bytes = vec![0u8; self.period_frames * self.encoder.bytes_per_frame()];
        let period =
            Duration::from_secs_f64(self.period_frames as f64 / f64::from(self.shared.rate().hz()));
        let mut next_period: Option<Instant> = None;
        loop {
            match self.shared.state() {
                StreamState::Closed => break,
                StreamState::Lost | StreamState::Failed => return Ok(()),
                StreamState::Ready | StreamState::Paused => {
                    next_period = None;
                    thread::park_timeout(IDLE_WAIT);
                    continue;
                }
                StreamState::Running => {}
            }
            let frames = match self.pacing {
                Pacing::RealTime => {
                    let now = Instant::now();
                    let due = *next_period.get_or_insert(now);
                    if due > now {
                        thread::park_timeout(due - now);
                        continue;
                    }
                    let base = if now - due > MAX_LAG { now } else { due };
                    next_period = Some(base + period);
                    self.period_frames
                }
                Pacing::Freewheel => {
                    let frames = self.reader.read_available(&mut samples);
                    if frames == 0 {
                        thread::park_timeout(Duration::from_millis(1));
                        continue;
                    }
                    frames
                }
            };
            let samples = &mut samples[..frames * channels];
            if self.pacing == Pacing::RealTime {
                self.reader.read(samples);
            }
            let len = self.encoder.encode(samples, &mut bytes);
            if let Err(error) = target.write(&bytes[..len]) {
                self.shared.set_state(StreamState::Failed);
                return Err(error);
            }
        }
        target.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use helikodu_model::audio::{ChannelLayout, Endianness, SampleFormat, SampleRate};

    use super::*;

    /// Collects everything written, shared with the test.
    #[derive(Clone, Default)]
    struct Collect(Arc<Mutex<(Vec<u8>, bool)>>);

    impl SinkTarget for Collect {
        fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.0.lock().unwrap().0.extend_from_slice(bytes);
            Ok(())
        }
        fn finish(&mut self) -> io::Result<()> {
            self.0.lock().unwrap().1 = true;
            Ok(())
        }
    }

    struct Broken;

    impl SinkTarget for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<()> {
            Err(io::Error::other("disk full"))
        }
        fn finish(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn spec() -> AudioSpec {
        AudioSpec::new(
            SampleFormat::S16(Endianness::Little),
            SampleRate::new(8000).unwrap(),
            ChannelLayout::STEREO,
        )
    }

    #[test]
    fn real_time_pacing_plays_silence_when_starved() {
        let target = Collect::default();
        let config = StreamConfig::new(spec());
        let mut opened = SoftwareStream::open(
            OutputBackendKind::Null,
            &config,
            spec(),
            Pacing::RealTime,
            target.clone(),
        )
        .unwrap();
        opened.stream.start().unwrap();
        thread::sleep(Duration::from_millis(100));
        opened.stream.pause().unwrap();
        let timing = opened.stream.timing();
        assert!(timing.underruns > 0, "{timing:?}");
        assert_eq!(timing.frames_delivered, 0);
        opened.stream.stop().unwrap();
        let (bytes, finished) = target.0.lock().unwrap().clone();
        assert!(finished);
        assert!(!bytes.is_empty());
        assert!(bytes.iter().all(|b| *b == 0));
        assert_eq!(opened.stream.state(), StreamState::Closed);
        assert!(opened.stream.start().is_err());
        assert!(opened.writer.is_closed());
    }

    #[test]
    fn write_errors_fail_the_stream() {
        let config = StreamConfig::new(spec());
        let mut opened = SoftwareStream::open(
            OutputBackendKind::Wav,
            &config,
            spec(),
            Pacing::Freewheel,
            Broken,
        )
        .unwrap();
        opened.writer.write(&[0.0; 64]);
        opened.stream.start().unwrap();
        let start = Instant::now();
        while opened.stream.state() != StreamState::Failed {
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(1));
        }
        assert!(matches!(opened.stream.stop(), Err(OutputError::Io(_))));
    }
}
