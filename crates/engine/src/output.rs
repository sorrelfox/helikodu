// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Output: the backends that send rendered audio to a device.
//!
//! An [`OutputBackend`] lists its devices and opens an [`OutputStream`] on one of
//! them. Opening negotiates the format the device takes and returns, besides the
//! stream, a [`SampleWriter`]: the producer side of a lock-free ring buffer of
//! interleaved 32-bit float samples in canonical (WAVE) channel order.
//!
//! The render thread fills the ring buffer ahead of playback. The backend's
//! real-time side (an audio callback, or a writer thread for ALSA and the
//! software sinks) copies out of the ring buffer, applies software volume,
//! anti-click fades and dither (the device-side stage, controlled with
//! [`DeviceAction`]s and [`SampleWriter::volume`]) and converts to the device
//! format. It never allocates, locks or blocks; when the ring buffer runs dry it
//! plays silence and counts an underrun in an atomic counter.
//!
//! Device additions and removals, a stream's device going away and other
//! failures arrive as [`OutputEvent`]s on the channel given to the backend.
//!
//! Exclusive access to a sound card that another program holds goes through an
//! injected [`DeviceReservation`] (see the [`reservation`](DeviceReservation)
//! types); the engine itself does not speak D-Bus.
//!
//! Backends:
//!
//! | Backend | Type | Cargo feature |
//! |---|---|---|
//! | PipeWire | [`PipeWireBackend`] | `pipewire` |
//! | ALSA | [`AlsaBackend`] | `alsa` |
//! | Null (discards audio) | [`NullBackend`] | always built |
//! | WAV file | [`WavBackend`] | always built |

#[cfg(feature = "alsa")]
mod alsa;
mod convert;
mod device_stage;
mod error;
mod event;
mod null;
#[cfg(feature = "pipewire")]
mod pipewire;
mod reservation;
mod ring;
mod software_sink;
mod stream_status;
mod wav;

use std::fmt;
use std::time::Duration;

use helikodu_model::audio::{
    AudioSpec, DeviceInfo, DeviceSelection, OutputBackendKind, OutputMode,
};

#[cfg(feature = "alsa")]
pub use self::alsa::AlsaBackend;
pub use self::device_stage::DeviceAction;
pub use self::error::OutputError;
pub use self::event::{EventSender, OutputEvent};
pub use self::null::NullBackend;
#[cfg(feature = "pipewire")]
pub use self::pipewire::PipeWireBackend;
pub use self::reservation::{
    DeviceReservation, NoReservation, ReservationError, ReservationGuard, ReservedCard,
    SharedReservation,
};
pub use self::ring::SampleWriter;
pub use self::software_sink::Pacing;
pub use self::stream_status::{StreamState, StreamTiming};
pub use self::wav::WavBackend;

/// A way of sending audio to devices: lists the devices and opens streams on them.
pub trait OutputBackend: Send + fmt::Debug {
    /// Which backend this is.
    fn kind(&self) -> OutputBackendKind;

    /// The devices the backend offers now, each with its stable name.
    ///
    /// "System default" is not in the list; it is
    /// [`DeviceSelection::SystemDefault`].
    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError>;

    /// Opens a stream on a device and negotiates its format.
    ///
    /// The stream does not play until [`OutputStream::start`]. Fill the returned
    /// [`SampleWriter`] before starting, so that playback does not begin with an
    /// underrun.
    fn open(&self, config: &StreamConfig) -> Result<OpenedStream, OutputError>;
}

/// An open stream on one device.
///
/// Dropping a stream stops it.
pub trait OutputStream: Send + fmt::Debug {
    /// The backend the stream belongs to.
    fn backend(&self) -> OutputBackendKind;

    /// The negotiated format: the rate at which samples are taken from the ring
    /// buffer, their channel count, and the sample format the device receives.
    ///
    /// It can differ from the requested [`StreamConfig::spec`]; the engine then
    /// converts (resamples or remixes) before writing.
    fn spec(&self) -> AudioSpec;

    /// What the stream is doing.
    fn state(&self) -> StreamState;

    /// Counters for position and diagnostics. Lock-free; cheap enough to call
    /// several times per second.
    fn timing(&self) -> StreamTiming;

    /// Starts or resumes playback.
    fn start(&mut self) -> Result<(), OutputError>;

    /// Pauses playback. Samples still in the ring buffer stay there.
    fn pause(&mut self) -> Result<(), OutputError>;

    /// Waits until everything written so far has been played, or `timeout` has
    /// passed. Silence after the last written sample does not count as an
    /// underrun while draining.
    fn drain(&mut self, timeout: Duration) -> Result<(), OutputError>;

    /// Stops playback and closes the device. Samples still in the ring buffer are
    /// discarded. A stopped stream cannot be started again.
    fn stop(&mut self) -> Result<(), OutputError>;

    /// Whether the backend applies the volume itself ([`set_volume`](Self::set_volume)).
    fn has_volume(&self) -> bool {
        false
    }

    /// Sets the stream volume as a linear amplitude (0.0 to 1.0) on every channel.
    ///
    /// Only backends whose volume the desktop mixer shows support this; the others
    /// return [`OutputError::Unsupported`] and the engine applies software volume.
    fn set_volume(&mut self, amplitude: f32) -> Result<(), OutputError> {
        let _ = amplitude;
        Err(OutputError::Unsupported("stream volume"))
    }
}

/// What to open: the audio the engine will write, where it goes, and how.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamConfig {
    /// The format the engine renders. Backends try to open the device with it and
    /// report what they got in [`OutputStream::spec`].
    pub spec: AudioSpec,
    /// The device.
    pub device: DeviceSelection,
    /// Shared or exclusive use of the device.
    pub mode: OutputMode,
    /// Whether to ask the device (or the sound server's graph) to run at
    /// `spec.rate`. When false, the backend takes the device's current rate where
    /// it can choose.
    pub request_rate: bool,
    /// The length of the ring buffer: how far the render thread may work ahead.
    pub buffer: Duration,
    /// The stream volume to set once the stream is connected, for backends that
    /// have one. PipeWire's session manager restores a stored volume per
    /// application; this value replaces it.
    pub volume: Option<f32>,
}

impl StreamConfig {
    /// The default ring buffer length.
    pub const DEFAULT_BUFFER: Duration = Duration::from_millis(200);

    /// A shared stream on the system default device, requesting the spec's rate,
    /// with the default buffer and no volume change.
    pub fn new(spec: AudioSpec) -> Self {
        Self {
            spec,
            device: DeviceSelection::SystemDefault,
            mode: OutputMode::Shared,
            request_rate: true,
            buffer: Self::DEFAULT_BUFFER,
            volume: None,
        }
    }
}

/// A newly opened stream and the writer that feeds it.
#[derive(Debug)]
pub struct OpenedStream {
    /// The stream.
    pub stream: Box<dyn OutputStream>,
    /// The producer side of the stream's ring buffer, for the render thread.
    pub writer: SampleWriter,
}

/// The backend to use when the settings say "automatic": PipeWire if its socket
/// is there, otherwise ALSA.
///
/// PulseAudio and JACK are not implemented yet and are never chosen.
pub fn automatic_backend() -> OutputBackendKind {
    #[cfg(feature = "pipewire")]
    if self::pipewire::socket_exists() {
        return OutputBackendKind::PipeWire;
    }
    if cfg!(feature = "alsa") {
        OutputBackendKind::Alsa
    } else {
        OutputBackendKind::Null
    }
}

/// Connects to a backend by kind.
///
/// The WAV backend needs a file name and is created with [`WavBackend::new`]
/// instead; asking for it here, or for a backend that was not built, is an
/// [`OutputError::BackendUnavailable`].
pub fn connect_backend(
    kind: OutputBackendKind,
    events: &EventSender,
) -> Result<Box<dyn OutputBackend>, OutputError> {
    connect_backend_with(kind, events, &SharedReservation::default())
}

/// Connects to a backend by kind, like [`connect_backend`], with the device
/// reservation the ALSA backend uses to take cards that another program holds.
pub fn connect_backend_with(
    kind: OutputBackendKind,
    events: &EventSender,
    reservation: &SharedReservation,
) -> Result<Box<dyn OutputBackend>, OutputError> {
    let unavailable = |reason: &str| OutputError::BackendUnavailable {
        backend: kind,
        reason: reason.to_owned(),
    };
    // Without the native features the event channel and reservation are not used.
    let _ = (events, reservation);
    match kind {
        #[cfg(feature = "pipewire")]
        OutputBackendKind::PipeWire => Ok(Box::new(PipeWireBackend::connect(events.clone())?)),
        #[cfg(feature = "alsa")]
        OutputBackendKind::Alsa => Ok(Box::new(
            AlsaBackend::new(events.clone()).with_reservation(reservation.clone()),
        )),
        OutputBackendKind::Null => Ok(Box::new(NullBackend::new())),
        OutputBackendKind::Wav => Err(unavailable("needs a file name; use WavBackend::new")),
        OutputBackendKind::PulseAudio | OutputBackendKind::Jack => {
            Err(unavailable("not implemented yet"))
        }
        #[allow(unreachable_patterns)]
        _ => Err(unavailable("not built into this program")),
    }
}
