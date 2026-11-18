// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Streams of the backends that go through cpal (PulseAudio and JACK): the audio
//! callback, the position and delay it reports, and errors.
//!
//! cpal calls the data callback on its own thread (PulseAudio's stream driver, JACK's
//! real-time process thread). The callback copies out of the ring buffer through the
//! device-side stage, converts to the device format and records the delay cpal
//! predicts until the first frame is heard. It never allocates, locks or blocks.
//! Once the stream has ended (its device went away, or it failed), it writes silence.

use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use helikodu_model::audio::{
    AudioSpec, DeviceSelection, Endianness, OutputBackendKind, SampleFormat,
};

use super::convert::Encoder;
use super::ring::{SampleReader, StreamShared, ring};
use super::{
    EventSender, OpenedStream, OutputError, OutputEvent, OutputStream, StreamConfig, StreamState,
    StreamTiming,
};

/// The most frames one callback converts in one go; larger requests are served in
/// several steps.
const SCRATCH_FRAMES: usize = 8192;

/// The sample format cpal takes for `format`, if it takes it as it is: unsigned 8-bit
/// and native-endian 16-, 24- (in 32-bit words) and 32-bit integers and floats.
pub(super) fn cpal_format(format: SampleFormat) -> Option<cpal::SampleFormat> {
    let native = Endianness::NATIVE;
    match format {
        SampleFormat::U8 => Some(cpal::SampleFormat::U8),
        SampleFormat::S16(e) if e == native => Some(cpal::SampleFormat::I16),
        SampleFormat::S24(e) if e == native => Some(cpal::SampleFormat::I24),
        SampleFormat::S32(e) if e == native => Some(cpal::SampleFormat::I32),
        SampleFormat::F32(e) if e == native => Some(cpal::SampleFormat::F32),
        _ => None,
    }
}

/// What to build a stream on.
pub(super) struct Build<'a> {
    pub(super) kind: OutputBackendKind,
    pub(super) device: &'a cpal::Device,
    /// What the engine asked for.
    pub(super) config: &'a StreamConfig,
    /// What the device will take: the ring buffer and the callback use it.
    pub(super) spec: AudioSpec,
    pub(super) buffer: cpal::BufferSize,
    pub(super) events: EventSender,
}

/// Builds a stream that does not play yet, and the writer that feeds it.
pub(super) fn build(request: Build<'_>) -> Result<(CpalStream, super::SampleWriter), OutputError> {
    let Build {
        kind,
        device,
        config,
        spec,
        buffer,
        events,
    } = request;
    let failed = |reason: String| OutputError::Backend {
        backend: kind,
        reason,
    };
    let channels = usize::from(spec.channel_count());
    let format = cpal_format(spec.format)
        .ok_or_else(|| failed(format!("cannot write {} through cpal", spec.format)))?;
    let encoder = Encoder::new(spec.format, channels)
        .ok_or_else(|| failed(format!("cannot write {}", spec.format)))?;
    let (writer, reader, shared) = ring(spec, config.buffer);
    let stream_config = cpal::StreamConfig {
        channels: spec.channel_count(),
        sample_rate: spec.rate.hz(),
        buffer_size: buffer,
    };
    let mut callback = Callback {
        reader,
        shared: shared.clone(),
        encoder,
        scratch: vec![0.0; SCRATCH_FRAMES * channels].into_boxed_slice(),
        channels,
        stride: spec.bytes_per_frame(),
        rate: u64::from(spec.rate.hz()),
    };
    let errors = ErrorHandler {
        kind,
        shared: shared.clone(),
        events,
        device: config.device.clone(),
    };
    let stream = device
        .build_output_stream_raw(
            stream_config,
            format,
            move |data: &mut cpal::Data, info: &cpal::OutputCallbackInfo| {
                callback.run(data.bytes_mut(), info);
            },
            move |error| errors.report(&error),
            Some(OPEN_TIMEOUT),
        )
        .map_err(|error| open_error(kind, &config.device, &error))?;
    shared.set_device_rate(spec.rate.hz());
    let stream = CpalStream {
        kind,
        spec,
        device: config.device.clone(),
        shared,
        stream: Some(stream),
    };
    Ok((stream, writer))
}

/// How long building a stream may take before the server counts as hung.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// An open stream of a cpal host.
pub(super) struct CpalStream {
    kind: OutputBackendKind,
    spec: AudioSpec,
    device: DeviceSelection,
    shared: Arc<StreamShared>,
    stream: Option<cpal::Stream>,
}

impl std::fmt::Debug for CpalStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpalStream")
            .field("kind", &self.kind)
            .field("spec", &self.spec)
            .field("device", &self.device)
            .field("state", &self.shared.state())
            .finish_non_exhaustive()
    }
}

impl CpalStream {
    /// The state the device side shares, for the device watcher.
    pub(super) fn shared(&self) -> &Arc<StreamShared> {
        &self.shared
    }

    /// Boxes the stream with its writer.
    pub(super) fn opened(self, writer: super::SampleWriter) -> OpenedStream {
        OpenedStream {
            stream: Box::new(self),
            writer,
        }
    }

    fn failed(&self, error: &cpal::Error) -> OutputError {
        OutputError::Backend {
            backend: self.kind,
            reason: error.to_string(),
        }
    }
}

impl OutputStream for CpalStream {
    fn backend(&self) -> OutputBackendKind {
        self.kind
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
        let Some(stream) = &self.stream else {
            return Err(OutputError::Closed);
        };
        if self.shared.state().is_finished() {
            return Err(OutputError::Closed);
        }
        stream.play().map_err(|error| self.failed(&error))?;
        self.shared.transition(StreamState::Running);
        Ok(())
    }

    fn pause(&mut self) -> Result<(), OutputError> {
        let Some(stream) = &self.stream else {
            return Err(OutputError::Closed);
        };
        if self.shared.state().is_finished() {
            return Err(OutputError::Closed);
        }
        stream.pause().map_err(|error| self.failed(&error))?;
        self.shared.transition(StreamState::Paused);
        Ok(())
    }

    fn drain(&mut self, timeout: Duration) -> Result<(), OutputError> {
        self.shared.wait_drained(timeout)
    }

    fn stop(&mut self) -> Result<(), OutputError> {
        // Closed first: errors cpal reports while the stream is torn down are not
        // failures.
        if !self.shared.state().is_finished() {
            self.shared.set_state(StreamState::Closed);
        }
        drop(self.stream.take());
        Ok(())
    }
}

impl Drop for CpalStream {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Why building a stream failed, in the engine's terms.
fn open_error(
    kind: OutputBackendKind,
    device: &DeviceSelection,
    error: &cpal::Error,
) -> OutputError {
    match error.kind() {
        cpal::ErrorKind::DeviceNotAvailable => OutputError::DeviceNotFound {
            backend: kind,
            device: device.clone(),
        },
        cpal::ErrorKind::DeviceBusy => OutputError::DeviceBusy {
            backend: kind,
            device: device.clone(),
            owner: None,
        },
        cpal::ErrorKind::UnsupportedConfig => OutputError::FormatUnsupported {
            backend: kind,
            device: device.clone(),
            reason: error.to_string(),
        },
        cpal::ErrorKind::HostUnavailable => OutputError::BackendUnavailable {
            backend: kind,
            reason: error.to_string(),
        },
        _ => OutputError::Backend {
            backend: kind,
            reason: error.to_string(),
        },
    }
}

/// What a stream error means for the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorEffect {
    /// The device went away.
    Lost,
    /// The stream cannot go on.
    Failed,
    /// The device skipped: one more underrun.
    Underrun,
    /// Nothing to act on.
    Ignore,
}

fn error_effect(kind: cpal::ErrorKind) -> ErrorEffect {
    match kind {
        cpal::ErrorKind::DeviceNotAvailable => ErrorEffect::Lost,
        cpal::ErrorKind::Xrun => ErrorEffect::Underrun,
        // A rerouted stream plays on; the device watcher reports what changed.
        cpal::ErrorKind::RealtimeDenied | cpal::ErrorKind::DeviceChanged => ErrorEffect::Ignore,
        _ => ErrorEffect::Failed,
    }
}

/// Turns cpal's stream errors into stream states and events. Runs on cpal's threads,
/// never on the real-time one for a lost device or a failure.
struct ErrorHandler {
    kind: OutputBackendKind,
    shared: Arc<StreamShared>,
    events: EventSender,
    device: DeviceSelection,
}

impl ErrorHandler {
    fn report(&self, error: &cpal::Error) {
        match error_effect(error.kind()) {
            ErrorEffect::Lost => {
                if self.shared.transition(StreamState::Lost) {
                    let _ = self.events.send(OutputEvent::DeviceLost {
                        backend: self.kind,
                        device: self.device.clone(),
                    });
                }
            }
            ErrorEffect::Failed => {
                if self.shared.transition(StreamState::Failed) {
                    let _ = self.events.send(OutputEvent::Error {
                        backend: self.kind,
                        reason: error.to_string(),
                    });
                }
            }
            ErrorEffect::Underrun => self.shared.note_device_underrun(),
            ErrorEffect::Ignore => {}
        }
    }
}

/// The real-time side of a stream.
struct Callback {
    reader: SampleReader,
    shared: Arc<StreamShared>,
    encoder: Encoder,
    scratch: Box<[f32]>,
    channels: usize,
    /// Bytes per frame.
    stride: usize,
    /// The stream's rate in hertz.
    rate: u64,
}

impl Callback {
    fn run(&mut self, bytes: &mut [u8], info: &cpal::OutputCallbackInfo) {
        let frames = bytes.len() / self.stride;
        if self.shared.state().is_finished() {
            // The device went away or the stream failed: whatever the server does
            // with the stream now (it may move it to another device), it gets
            // silence. cpal filled the buffer with silence before the call.
            return;
        }
        let scratch_frames = self.scratch.len() / self.channels;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(scratch_frames);
            let samples = &mut self.scratch[..n * self.channels];
            self.reader.read(samples);
            self.encoder.encode(
                samples,
                &mut bytes[done * self.stride..(done + n) * self.stride],
            );
            done += n;
        }
        // The first frame written now is heard after `latency`; until then the
        // listener hears what was delivered before it.
        let stamp = info.timestamp();
        let latency = stamp.playback.duration_since(stamp.callback);
        let delay = latency.as_nanos() * u128::from(self.rate) / 1_000_000_000;
        let delay = u64::try_from(delay)
            .unwrap_or(u64::MAX)
            .saturating_add(frames as u64);
        self.shared.set_delay_frames(delay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_native_formats_pass_unchanged() {
        let native = Endianness::NATIVE;
        let foreign = match native {
            Endianness::Little => Endianness::Big,
            Endianness::Big => Endianness::Little,
        };
        assert_eq!(
            cpal_format(SampleFormat::S16(native)),
            Some(cpal::SampleFormat::I16)
        );
        assert_eq!(
            cpal_format(SampleFormat::S24(native)),
            Some(cpal::SampleFormat::I24)
        );
        assert_eq!(cpal_format(SampleFormat::S16(foreign)), None);
        assert_eq!(cpal_format(SampleFormat::S24Packed(native)), None);
        assert_eq!(cpal_format(SampleFormat::F64(native)), None);
        assert_eq!(cpal_format(SampleFormat::DsdU8), None);
        // Every format cpal takes has an encoder.
        for format in SampleFormat::ALL {
            if cpal_format(format).is_some() {
                assert!(Encoder::new(format, 2).is_some(), "{format}");
            }
        }
    }

    #[test]
    fn a_vanished_device_is_lost_and_a_skip_an_underrun() {
        assert_eq!(
            error_effect(cpal::ErrorKind::DeviceNotAvailable),
            ErrorEffect::Lost
        );
        assert_eq!(
            error_effect(cpal::ErrorKind::StreamInvalidated),
            ErrorEffect::Failed
        );
        assert_eq!(
            error_effect(cpal::ErrorKind::HostUnavailable),
            ErrorEffect::Failed
        );
        assert_eq!(error_effect(cpal::ErrorKind::Xrun), ErrorEffect::Underrun);
        assert_eq!(
            error_effect(cpal::ErrorKind::RealtimeDenied),
            ErrorEffect::Ignore
        );
    }
}
