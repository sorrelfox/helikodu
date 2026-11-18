// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! ALSA streams: the handle, and the writer thread that feeds the device.

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ::alsa as alsa_lib;
use alsa_lib::pcm::{ChmapPosition, PCM, State};
use helikodu_model::audio::{
    AudioSpec, ChannelLayout, ChannelPosition, DeviceSelection, OutputBackendKind,
};

use super::super::convert::Encoder;
use super::super::ring::{SampleReader, StreamShared, ring_tapped};
use super::super::{
    EventSender, OpenedStream, OutputError, OutputEvent, OutputStream, ReservationGuard,
    StreamConfig, StreamState, StreamTiming,
};
use super::{KIND, libc_errno};

/// How long an idle writer sleeps before it looks at its state again.
const IDLE_WAIT: Duration = Duration::from_millis(50);
/// How long the writer waits for room in the device buffer at most, in ms.
const DEVICE_WAIT_MS: u32 = 50;

/// What [`negotiate`](super::negotiate) settled on.
pub(super) struct Negotiated {
    pub(super) spec: AudioSpec,
    pub(super) encoder: Encoder,
    pub(super) period_frames: usize,
    pub(super) can_pause: bool,
}

/// For each device channel, the index of the layout channel it plays, from the
/// device's channel map. `None` if the device has no map or the map names
/// positions the layout does not have; the order is then left as it is.
pub(super) fn channel_map(pcm: &PCM, layout: ChannelLayout) -> Option<Vec<usize>> {
    let chmap = pcm.get_chmap().ok()?;
    let positions: Vec<ChmapPosition> = (&chmap).into();
    if positions.len() != usize::from(layout.channel_count()) || layout.is_unspecified() {
        return None;
    }
    positions
        .into_iter()
        .map(|p| layout.index_of(channel_position(p)?))
        .collect()
}

fn channel_position(position: ChmapPosition) -> Option<ChannelPosition> {
    Some(match position {
        ChmapPosition::FL => ChannelPosition::FrontLeft,
        ChmapPosition::FR => ChannelPosition::FrontRight,
        ChmapPosition::FC | ChmapPosition::Mono => ChannelPosition::FrontCenter,
        ChmapPosition::LFE => ChannelPosition::LowFrequency,
        ChmapPosition::RL => ChannelPosition::BackLeft,
        ChmapPosition::RR => ChannelPosition::BackRight,
        ChmapPosition::FLC => ChannelPosition::FrontLeftOfCenter,
        ChmapPosition::FRC => ChannelPosition::FrontRightOfCenter,
        ChmapPosition::RC => ChannelPosition::BackCenter,
        ChmapPosition::SL => ChannelPosition::SideLeft,
        ChmapPosition::SR => ChannelPosition::SideRight,
        ChmapPosition::TC => ChannelPosition::TopCenter,
        ChmapPosition::TFL => ChannelPosition::TopFrontLeft,
        ChmapPosition::TFC => ChannelPosition::TopFrontCenter,
        ChmapPosition::TFR => ChannelPosition::TopFrontRight,
        ChmapPosition::TRL => ChannelPosition::TopBackLeft,
        ChmapPosition::TRC => ChannelPosition::TopBackCenter,
        ChmapPosition::TRR => ChannelPosition::TopBackRight,
        _ => return None,
    })
}

/// An open ALSA stream.
#[derive(Debug)]
pub(super) struct AlsaStream {
    spec: AudioSpec,
    shared: Arc<StreamShared>,
    thread: Option<JoinHandle<()>>,
    /// The card taken from its holder, given back once the device is closed.
    reservation: Option<ReservationGuard>,
}

impl AlsaStream {
    pub(super) fn spawn(
        pcm: PCM,
        config: &StreamConfig,
        negotiated: Negotiated,
        events: EventSender,
        reservation: Option<ReservationGuard>,
    ) -> Result<OpenedStream, OutputError> {
        let spec = negotiated.spec;
        let (writer, reader, shared) = ring_tapped(spec, config.buffer, config.tap.as_ref());
        let worker = Writer {
            pcm,
            reader,
            shared: shared.clone(),
            encoder: negotiated.encoder,
            period_frames: negotiated.period_frames,
            can_pause: negotiated.can_pause,
            events,
            device: config.device.clone(),
        };
        let thread = thread::Builder::new()
            .name("helikodu-alsa".into())
            .spawn(move || worker.run())?;
        Ok(OpenedStream {
            stream: Box::new(Self {
                spec,
                shared,
                thread: Some(thread),
                reservation,
            }),
            writer,
        })
    }

    fn wake(&self) {
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }
}

impl OutputStream for AlsaStream {
    fn backend(&self) -> OutputBackendKind {
        KIND
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
        self.wake();
        Ok(())
    }

    fn drain(&mut self, timeout: Duration) -> Result<(), OutputError> {
        self.shared.wait_drained(timeout)
    }

    fn stop(&mut self) -> Result<(), OutputError> {
        if !self.shared.state().is_finished() {
            self.shared.set_state(StreamState::Closed);
        }
        self.wake();
        // The writer thread owns the device and closes it when it ends.
        let panicked = self
            .thread
            .take()
            .is_some_and(|thread| thread.join().is_err());
        self.shared.set_state(StreamState::Closed);
        // Only now that the device is closed, give the card back.
        self.reservation = None;
        if panicked {
            return Err(OutputError::Backend {
                backend: KIND,
                reason: "the writer thread panicked".into(),
            });
        }
        Ok(())
    }
}

impl Drop for AlsaStream {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// The writer thread: the ALSA counterpart of an audio callback. The device is
/// opened non-blocking; the thread waits for room with `poll` and writes one
/// period at a time.
struct Writer {
    pcm: PCM,
    reader: SampleReader,
    shared: Arc<StreamShared>,
    encoder: Encoder,
    period_frames: usize,
    can_pause: bool,
    events: EventSender,
    device: DeviceSelection,
}

impl Writer {
    fn run(mut self) {
        let channels = self.shared.channels();
        let frame_bytes = self.encoder.bytes_per_frame();
        let mut samples = vec![0.0f32; self.period_frames * channels];
        let mut bytes = vec![0u8; self.period_frames * frame_bytes];
        let mut device_paused = false;
        loop {
            match self.shared.state() {
                StreamState::Closed => {
                    // Stop at once; what is in the device buffer is dropped.
                    let _ = self.pcm.drop();
                    return;
                }
                StreamState::Lost | StreamState::Failed => return,
                StreamState::Ready => {
                    thread::park_timeout(IDLE_WAIT);
                    continue;
                }
                StreamState::Paused => {
                    if !device_paused {
                        self.pause_device();
                        device_paused = true;
                    }
                    thread::park_timeout(IDLE_WAIT);
                    continue;
                }
                StreamState::Running => {
                    if device_paused {
                        if let Err(error) = self.resume_device() {
                            self.handle_error(&error);
                        }
                        device_paused = false;
                    }
                }
            }

            let room = match self.pcm.avail_update() {
                Ok(frames) => usize::try_from(frames).unwrap_or(0),
                Err(error) => {
                    self.handle_error(&error);
                    continue;
                }
            };
            if room < self.period_frames {
                if let Err(error) = self.pcm.wait(Some(DEVICE_WAIT_MS)) {
                    self.handle_error(&error);
                }
                continue;
            }

            self.reader
                .read_encoded(&mut samples, &self.encoder, &mut bytes);
            self.write_all(&bytes, frame_bytes);
            if let Ok(delay) = self.pcm.delay() {
                self.shared
                    .set_delay_frames(u64::try_from(delay).unwrap_or(0));
            }
        }
    }

    /// Writes all of `bytes`, waiting for room if the device takes only part.
    fn write_all(&self, mut bytes: &[u8], frame_bytes: usize) {
        let io = self.pcm.io_bytes();
        while !bytes.is_empty() && !self.shared.state().is_finished() {
            match io.writei(bytes) {
                Ok(frames) => bytes = &bytes[(frames * frame_bytes).min(bytes.len())..],
                Err(error) if error.errno() == libc_errno::EAGAIN => {
                    let _ = self.pcm.wait(Some(DEVICE_WAIT_MS));
                }
                Err(error) => {
                    // After a recovered underrun the rest of this period is
                    // dropped; the next one starts the device again.
                    self.handle_error(&error);
                    return;
                }
            }
        }
    }

    fn pause_device(&self) {
        if self.can_pause && self.pcm.state() == State::Running && self.pcm.pause(true).is_ok() {
            return;
        }
        // Without hardware pause, stop the device; what was in its buffer is lost.
        let _ = self.pcm.drop();
    }

    fn resume_device(&self) -> Result<(), alsa_lib::Error> {
        match self.pcm.state() {
            State::Paused => self.pcm.pause(false),
            State::Setup | State::XRun => self.pcm.prepare(),
            _ => Ok(()),
        }
    }

    /// Recovers from an underrun or a suspend; anything else ends the stream.
    fn handle_error(&self, error: &alsa_lib::Error) {
        let errno = error.errno();
        if errno == libc_errno::EAGAIN {
            return;
        }
        if (errno == libc_errno::EPIPE || errno == libc_errno::ESTRPIPE)
            && self.pcm.recover(-errno, true).is_ok()
        {
            if errno == libc_errno::EPIPE {
                self.shared.note_device_underrun();
            }
            return;
        }
        if errno == libc_errno::ENODEV || errno == libc_errno::EBADFD {
            if self.shared.transition(StreamState::Lost) {
                let _ = self.events.send(OutputEvent::DeviceLost {
                    backend: KIND,
                    device: self.device.clone(),
                });
            }
        } else if self.shared.transition(StreamState::Failed) {
            let _ = self.events.send(OutputEvent::Error {
                backend: KIND,
                reason: format!("writing to {}: {error}", self.device),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alsa_positions_map_to_wave_positions() {
        assert_eq!(
            channel_position(ChmapPosition::RL),
            Some(ChannelPosition::BackLeft)
        );
        assert_eq!(
            channel_position(ChmapPosition::Mono),
            Some(ChannelPosition::FrontCenter)
        );
        assert_eq!(channel_position(ChmapPosition::NA), None);
    }
}
