// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! PipeWire playback streams: the handle other threads hold, and the stream
//! itself on the PipeWire thread with its real-time process callback.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use ::pipewire as pw;
use helikodu_model::audio::{
    AudioSpec, ChannelPosition, DeviceSelection, Endianness, OutputBackendKind, OutputMode,
};
use pw::spa;
use pw::spa::pod::Pod;
use pw::stream::{StreamFlags, StreamListener, StreamRc};

use super::super::convert::Encoder;
use super::super::ring::{SampleReader, StreamShared};
use super::super::{
    EventSender, OutputError, OutputEvent, OutputStream, StreamConfig, StreamState, StreamTiming,
};
use super::connection::{Command, OpenRequest};
use super::{COMMAND_TIMEOUT, KIND, Link, OPEN_TIMEOUT};

/// The most frames one process callback converts in one go. Larger requests are
/// served in several steps.
const SCRATCH_FRAMES: usize = 8192;

type Reply = mpsc::Sender<Result<(), OutputError>>;

/// The handle to a PipeWire stream, used from any thread.
pub(super) struct PipeWireStream {
    id: u64,
    link: Arc<Link>,
    spec: AudioSpec,
    shared: Arc<StreamShared>,
    device: DeviceSelection,
    closed: bool,
}

impl std::fmt::Debug for PipeWireStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipeWireStream")
            .field("id", &self.id)
            .field("spec", &self.spec)
            .field("device", &self.device)
            .field("state", &self.shared.state())
            .finish_non_exhaustive()
    }
}

impl PipeWireStream {
    pub(super) fn new(
        id: u64,
        link: Arc<Link>,
        spec: AudioSpec,
        shared: Arc<StreamShared>,
        device: DeviceSelection,
    ) -> Self {
        Self {
            id,
            link,
            spec,
            shared,
            device,
            closed: false,
        }
    }

    /// Creates the stream on the PipeWire thread and waits until it is linked to
    /// its device, or has failed.
    pub(super) fn open(&self, request: OpenRequest) -> Result<(), OutputError> {
        let id = self.id;
        let request = Box::new(request);
        self.link
            .call(OPEN_TIMEOUT, "the stream to connect", |reply| {
                Command::Open { id, request, reply }
            })
    }

    fn command(
        &self,
        what: &'static str,
        command: impl FnOnce(Reply) -> Command,
    ) -> Result<(), OutputError> {
        if self.closed || self.shared.state().is_finished() {
            return Err(OutputError::Closed);
        }
        self.link.call(COMMAND_TIMEOUT, what, command)
    }
}

impl OutputStream for PipeWireStream {
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
        let id = self.id;
        self.command("the stream to start", |reply| Command::Start { id, reply })
    }

    fn pause(&mut self) -> Result<(), OutputError> {
        let id = self.id;
        self.command("the stream to pause", |reply| Command::Pause { id, reply })
    }

    fn drain(&mut self, timeout: Duration) -> Result<(), OutputError> {
        self.shared.wait_drained(timeout)
    }

    fn stop(&mut self) -> Result<(), OutputError> {
        if std::mem::replace(&mut self.closed, true) {
            return Ok(());
        }
        let id = self.id;
        let result = self
            .link
            .call(COMMAND_TIMEOUT, "the stream to close", |reply| {
                Command::Close { id, reply }
            });
        self.shared.set_state(StreamState::Closed);
        // A backend that has already shut down has closed the stream itself.
        match result {
            Err(OutputError::Closed) => Ok(()),
            other => other,
        }
    }

    fn has_volume(&self) -> bool {
        true
    }

    fn set_volume(&mut self, amplitude: f32) -> Result<(), OutputError> {
        let id = self.id;
        let amplitude = amplitude.clamp(0.0, 1.0);
        self.command("the volume to change", |reply| Command::Volume {
            id,
            amplitude,
            reply,
        })
    }
}

impl Drop for PipeWireStream {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// A stream on the PipeWire thread.
pub(super) struct StreamEntry {
    // The listeners come first: fields drop in order, and a listener must be
    // removed before its stream is destroyed.
    _state_listener: StreamListener<()>,
    _process_listener: StreamListener<()>,
    stream: StreamRc,
    shared: Arc<StreamShared>,
    opening: Rc<Opening>,
    channels: usize,
}

impl StreamEntry {
    /// Creates and connects a stream. The answer to `reply` is sent once the
    /// stream is linked or has failed. On an immediate failure the reply is
    /// handed back with the error.
    pub(super) fn create(
        core: &pw::core::CoreRc,
        request: OpenRequest,
        reply: Reply,
        events: EventSender,
        target_known: &dyn Fn(&str) -> bool,
    ) -> Result<Self, (OutputError, Reply)> {
        let OpenRequest {
            config,
            spec,
            reader,
            shared,
        } = request;
        let channels = usize::from(spec.channel_count());
        let backend_error = |reason: String| OutputError::Backend {
            backend: KIND,
            reason,
        };
        let Some(encoder) = Encoder::new(spec.format, channels) else {
            return Err((
                backend_error(format!("cannot write {}", spec.format)),
                reply,
            ));
        };

        let format = match format_pod(spec) {
            Ok(bytes) => bytes,
            Err(reason) => return Err((backend_error(reason), reply)),
        };
        let Some(pod) = Pod::from_bytes(&format) else {
            return Err((backend_error("bad format pod".into()), reply));
        };
        let props = properties(&config, spec);
        let stream = match StreamRc::new(core.clone(), "Helikodu playback", props) {
            Ok(stream) => stream,
            Err(error) => return Err((backend_error(error.to_string()), reply)),
        };

        // The reply is answered once the stream is linked or has failed; failures
        // below take it back.
        let opening = Rc::new(Opening {
            reply: RefCell::new(Some(reply)),
            paused: Cell::new(false),
            linked: Cell::new(false),
            connected: Cell::new(false),
            volume: config.volume.map(|v| vec![v.clamp(0.0, 1.0); channels]),
        });
        let fail = |error: OutputError| {
            let reply = opening
                .reply
                .borrow_mut()
                .take()
                .unwrap_or_else(|| mpsc::channel().0);
            Err((error, reply))
        };

        // Two listeners, so that the real-time callback and the main-loop callback
        // never share data.
        let mut process = Process {
            reader,
            shared: shared.clone(),
            encoder,
            scratch: vec![0.0; SCRATCH_FRAMES * channels].into_boxed_slice(),
            channels,
            stride: spec.bytes_per_frame(),
            rate: spec.rate.hz(),
        };
        let process_listener = match stream
            .add_local_listener_with_user_data(())
            .process(move |stream, ()| process.run(stream))
            .register()
        {
            Ok(listener) => listener,
            Err(error) => return fail(backend_error(error.to_string())),
        };

        let device = config.device.clone();
        let target_exists = match &device {
            DeviceSelection::Device(id) => target_known(id.as_str()),
            DeviceSelection::SystemDefault => true,
        };
        let watcher = StateWatcher {
            shared: shared.clone(),
            events,
            device,
            target_exists,
            opening: opening.clone(),
        };
        let state_listener = match stream
            .add_local_listener_with_user_data(())
            .state_changed(move |stream, (), _old, new| watcher.changed(stream, new))
            .register()
        {
            Ok(listener) => listener,
            Err(error) => return fail(backend_error(error.to_string())),
        };

        let flags = StreamFlags::AUTOCONNECT
            | StreamFlags::INACTIVE
            | StreamFlags::MAP_BUFFERS
            | StreamFlags::RT_PROCESS;
        if let Err(error) = stream.connect(spa::utils::Direction::Output, None, flags, &mut [pod]) {
            return fail(backend_error(error.to_string()));
        }
        Ok(Self {
            _state_listener: state_listener,
            _process_listener: process_listener,
            stream,
            shared,
            opening,
            channels,
        })
    }

    /// The stream's node id, once the server has created the node.
    pub(super) fn node_id(&self) -> u32 {
        self.stream.node_id()
    }

    /// The session manager linked the stream's node to a device.
    pub(super) fn linked(&self) {
        self.opening.linked.set(true);
        self.opening.try_finish(&self.stream);
    }

    pub(super) fn set_active(&self, active: bool) -> Result<(), OutputError> {
        let state = if active {
            StreamState::Running
        } else {
            StreamState::Paused
        };
        if self.shared.state().is_finished() {
            return Err(OutputError::Closed);
        }
        self.stream
            .set_active(active)
            .map_err(|error| OutputError::Backend {
                backend: KIND,
                reason: error.to_string(),
            })?;
        self.shared.transition(state);
        Ok(())
    }

    pub(super) fn set_volume(&self, amplitude: f32) -> Result<(), OutputError> {
        let volumes = vec![amplitude; self.channels];
        self.stream
            .set_control(spa::sys::SPA_PROP_channelVolumes, &volumes)
            .map_err(|error| OutputError::Backend {
                backend: KIND,
                reason: error.to_string(),
            })
    }

    /// Marks the stream failed, for a lost connection.
    pub(super) fn fail(&self) {
        self.shared.transition(StreamState::Failed);
    }

    pub(super) fn close(self) {
        let _ = self.stream.disconnect();
        if !self.shared.state().is_finished() {
            self.shared.set_state(StreamState::Closed);
        }
    }
}

/// The stream properties: identity, target, rate and exclusivity.
fn properties(config: &StreamConfig, spec: AudioSpec) -> pw::properties::PropertiesBox {
    let mut props = pw::properties::properties! {
        "media.type" => "Audio",
        "media.category" => "Playback",
        "media.role" => "Music",
        "application.name" => "Helikodu",
        "application.id" => "io.github.sorrelfox.Helikodu",
        "node.name" => "helikodu",
        "node.description" => "Helikodu",
    };
    if let DeviceSelection::Device(id) = &config.device {
        props.insert("target.object", id.as_str());
        // Fail rather than move to another device when the device goes away...
        props.insert("node.dont-reconnect", "true");
        // ...or when it is not there: without this the session manager links the
        // stream to the default device instead.
        props.insert("node.dont-fallback", "true");
    }
    let rate = spec.rate.hz();
    let exclusive = config.mode == OutputMode::Exclusive;
    if config.request_rate {
        props.insert("node.rate", format!("1/{rate}"));
        if exclusive {
            // node.rate alone only switches the graph if the rate is allowed;
            // forcing switches it for every stream on the device.
            props.insert("node.force-rate", rate.to_string());
        }
    }
    if exclusive {
        props.insert("node.exclusive", "true");
    }
    props
}

/// An `EnumFormat` pod offering exactly `spec`, with explicit channel positions.
fn format_pod(spec: AudioSpec) -> Result<Vec<u8>, String> {
    use spa::param::audio::{AudioFormat, AudioInfoRaw};
    let mut info = AudioInfoRaw::new();
    info.set_format(match Endianness::NATIVE {
        Endianness::Little => AudioFormat::F32LE,
        Endianness::Big => AudioFormat::F32BE,
    });
    info.set_rate(spec.rate.hz());
    info.set_channels(u32::from(spec.channel_count()));
    let mut position = [0; spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    let positions: Vec<u32> = if spec.layout.is_unspecified() {
        (0..u32::from(spec.channel_count()))
            .map(|i| spa::sys::SPA_AUDIO_CHANNEL_AUX0 + i)
            .collect()
    } else {
        spec.layout.positions().map(spa_channel).collect()
    };
    for (slot, value) in position.iter_mut().zip(positions) {
        *slot = value;
    }
    info.set_position(position);
    let object = spa::pod::Value::Object(spa::pod::Object {
        type_: spa::sys::SPA_TYPE_OBJECT_Format,
        id: spa::sys::SPA_PARAM_EnumFormat,
        properties: info.into(),
    });
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &object)
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|error| format!("cannot build the format: {error:?}"))
}

/// The SPA channel position of a WAVE position.
fn spa_channel(position: ChannelPosition) -> u32 {
    use spa::sys::*;
    match position {
        ChannelPosition::FrontLeft => SPA_AUDIO_CHANNEL_FL,
        ChannelPosition::FrontRight => SPA_AUDIO_CHANNEL_FR,
        ChannelPosition::FrontCenter => SPA_AUDIO_CHANNEL_FC,
        ChannelPosition::LowFrequency => SPA_AUDIO_CHANNEL_LFE,
        ChannelPosition::BackLeft => SPA_AUDIO_CHANNEL_RL,
        ChannelPosition::BackRight => SPA_AUDIO_CHANNEL_RR,
        ChannelPosition::FrontLeftOfCenter => SPA_AUDIO_CHANNEL_FLC,
        ChannelPosition::FrontRightOfCenter => SPA_AUDIO_CHANNEL_FRC,
        ChannelPosition::BackCenter => SPA_AUDIO_CHANNEL_RC,
        ChannelPosition::SideLeft => SPA_AUDIO_CHANNEL_SL,
        ChannelPosition::SideRight => SPA_AUDIO_CHANNEL_SR,
        ChannelPosition::TopCenter => SPA_AUDIO_CHANNEL_TC,
        ChannelPosition::TopFrontLeft => SPA_AUDIO_CHANNEL_TFL,
        ChannelPosition::TopFrontCenter => SPA_AUDIO_CHANNEL_TFC,
        ChannelPosition::TopFrontRight => SPA_AUDIO_CHANNEL_TFR,
        ChannelPosition::TopBackLeft => SPA_AUDIO_CHANNEL_TRL,
        ChannelPosition::TopBackCenter => SPA_AUDIO_CHANNEL_TRC,
        ChannelPosition::TopBackRight => SPA_AUDIO_CHANNEL_TRR,
    }
}

/// The progress of a stream that is being opened. Opening is complete when the
/// stream has reached `Paused` (its node exists and the format is set) and the
/// session manager has linked it to a device; reaching `Paused` alone does not
/// mean that a target was found.
struct Opening {
    reply: RefCell<Option<Reply>>,
    paused: Cell<bool>,
    linked: Cell<bool>,
    /// Whether opening succeeded; later errors are a lost device or a failure.
    connected: Cell<bool>,
    /// The volume to set once connected.
    volume: Option<Vec<f32>>,
}

impl Opening {
    fn try_finish(&self, stream: &pw::stream::Stream) {
        if self.paused.get() && self.linked.get() && !self.connected.replace(true) {
            // The session manager restores a stored volume when the stream
            // connects; the engine's own volume replaces it.
            if let Some(volume) = &self.volume {
                let _ = stream.set_control(spa::sys::SPA_PROP_channelVolumes, volume);
            }
            self.answer(Ok(()));
        }
    }

    fn answer(&self, result: Result<(), OutputError>) {
        if let Some(reply) = self.reply.borrow_mut().take() {
            let _ = reply.send(result);
        }
    }
}

/// Follows a stream's state on the main loop: answers the open request, sets the
/// initial volume, and reports a lost device or a failure.
struct StateWatcher {
    shared: Arc<StreamShared>,
    events: EventSender,
    device: DeviceSelection,
    /// Whether the target was in the device list when the stream was opened, to
    /// tell "not there" from "held exclusively by another stream".
    target_exists: bool,
    opening: Rc<Opening>,
}

impl StateWatcher {
    fn changed(&self, stream: &pw::stream::Stream, new: pw::stream::StreamState) {
        match new {
            pw::stream::StreamState::Paused | pw::stream::StreamState::Streaming => {
                self.opening.paused.set(true);
                self.opening.try_finish(stream);
            }
            pw::stream::StreamState::Error(message) => self.failed(&message),
            pw::stream::StreamState::Unconnected | pw::stream::StreamState::Connecting => {}
        }
    }

    fn failed(&self, message: &str) {
        let target_missing = message.contains("target not found");
        if !self.opening.connected.get() {
            // Opening failed.
            self.shared.set_state(StreamState::Failed);
            let error = match (&self.device, target_missing, self.target_exists) {
                (DeviceSelection::Device(_), true, true) => OutputError::DeviceBusy {
                    backend: KIND,
                    device: self.device.clone(),
                    owner: Some("an exclusive stream".into()),
                },
                (DeviceSelection::Device(_), true, false) => OutputError::DeviceNotFound {
                    backend: KIND,
                    device: self.device.clone(),
                },
                _ => OutputError::Backend {
                    backend: KIND,
                    reason: message.to_owned(),
                },
            };
            self.opening.answer(Err(error));
            return;
        }
        if target_missing {
            if self.shared.transition(StreamState::Lost) {
                let _ = self.events.send(OutputEvent::DeviceLost {
                    backend: KIND,
                    device: self.device.clone(),
                });
            }
        } else if self.shared.transition(StreamState::Failed) {
            let _ = self.events.send(OutputEvent::Error {
                backend: KIND,
                reason: message.to_owned(),
            });
        }
    }
}

/// The real-time side of a stream: runs on PipeWire's data thread. It copies,
/// converts and stores counters; it never allocates, locks or blocks.
struct Process {
    reader: SampleReader,
    shared: Arc<StreamShared>,
    encoder: Encoder,
    scratch: Box<[f32]>,
    channels: usize,
    /// Bytes per frame.
    stride: usize,
    /// The stream's rate in hertz.
    rate: u32,
}

impl Process {
    fn run(&mut self, stream: &pw::stream::Stream) {
        if let Some(mut buffer) = stream.dequeue_buffer() {
            // The graph asks for a quantum; the mapped buffer is much larger, and
            // filling all of it would make the callback run far too rarely.
            let requested = usize::try_from(buffer.requested()).unwrap_or(usize::MAX);
            if let Some(data) = buffer.datas_mut().first_mut() {
                let mut frames = 0;
                if let Some(bytes) = data.data() {
                    frames = self.fill(bytes, requested);
                }
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = i32::try_from(self.stride).unwrap_or(0);
                *chunk.size_mut() = u32::try_from(frames * self.stride).unwrap_or(0);
            }
            // Dropping the buffer queues it.
        }
        self.record_delay(stream);
    }

    /// Fills `bytes` with up to `requested` frames and returns how many it wrote.
    fn fill(&mut self, bytes: &mut [u8], requested: usize) -> usize {
        let scratch_frames = self.scratch.len() / self.channels;
        let room = bytes.len() / self.stride;
        let frames = if requested > 0 {
            requested.min(room)
        } else {
            scratch_frames.min(room)
        };
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(scratch_frames);
            let samples = &mut self.scratch[..n * self.channels];
            self.reader.read_encoded(
                samples,
                &self.encoder,
                &mut bytes[done * self.stride..(done + n) * self.stride],
            );
            done += n;
        }
        frames
    }

    /// Records how many delivered frames have not been heard yet: the graph delay
    /// to the device, the frames still queued in the stream, and the resampler's.
    fn record_delay(&self, stream: &pw::stream::Stream) {
        let Ok(time) = stream.time() else {
            return;
        };
        let clock = time.rate();
        let mut delay = 0u64;
        if clock.denom > 0 {
            // `delay` is in graph ticks of clock.num / clock.denom seconds.
            let ticks = u128::try_from(time.delay()).unwrap_or(0);
            let frames =
                ticks * u128::from(clock.num) * u128::from(self.rate) / u128::from(clock.denom);
            delay = u64::try_from(frames).unwrap_or(u64::MAX);
            if clock.num == 1 {
                self.shared.set_device_rate(clock.denom);
            }
        }
        delay = delay
            .saturating_add(time.queued() / self.stride as u64)
            .saturating_add(time.buffered());
        self.shared.set_delay_frames(delay);
    }
}
