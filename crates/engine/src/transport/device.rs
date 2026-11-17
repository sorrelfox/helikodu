// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The output the engine plays to: the connected backend and the open stream.

use helikodu_model::audio::{
    AudioSpec, BackendSelection, DeviceInfo, DeviceSelection, OutputBackendKind, OutputMode,
    OutputSelection,
};
use helikodu_model::player::Volume;
use helikodu_model::settings::VolumeControl;

use helikodu_model::player::{EngineError, EngineEvent, Loudness};
use helikodu_model::settings::DeviceLostPolicy;

use super::source::Source;
use super::worker::Worker;
use crate::output::{
    EventSender, NullBackend, OutputBackend, OutputError, OutputStream, Pacing, SampleWriter,
    SharedReservation, StreamConfig, WavBackend, automatic_backend, connect_backend_with,
};
use crate::pipeline::{LoudnessInfo, PathPlan, PathRequest, VolumePlan, stream_config};

/// The connected backend, kept between streams: connecting to a sound server takes
/// time.
#[derive(Debug)]
pub(super) struct Backends {
    connected: Option<(OutputBackendKind, Box<dyn OutputBackend>)>,
    events: EventSender,
    pacing: Pacing,
    reservation: SharedReservation,
}

impl Backends {
    pub(super) fn new(events: EventSender, pacing: Pacing, reservation: SharedReservation) -> Self {
        Self {
            connected: None,
            events,
            pacing,
            reservation,
        }
    }

    /// The backend `selection` names, connected if it is not yet. The second value
    /// is its device list when it was connected just now.
    pub(super) fn get(
        &mut self,
        selection: &OutputSelection,
    ) -> Result<(&dyn OutputBackend, Option<Vec<DeviceInfo>>), OutputError> {
        let kind = match selection.backend {
            BackendSelection::Automatic => automatic_backend(),
            BackendSelection::Backend(kind) => kind,
        };
        let mut fresh = None;
        let reuse =
            matches!(&self.connected, Some((k, _)) if *k == kind && kind != OutputBackendKind::Wav);
        if !reuse {
            self.connected = None;
            let backend: Box<dyn OutputBackend> = match kind {
                OutputBackendKind::Null => Box::new(NullBackend::with_pacing(self.pacing)),
                OutputBackendKind::Wav => {
                    let DeviceSelection::Device(path) = &selection.device else {
                        return Err(OutputError::BackendUnavailable {
                            backend: kind,
                            reason: "needs a file name as its device".into(),
                        });
                    };
                    Box::new(WavBackend::new(path.as_str()).with_pacing(self.pacing))
                }
                _ => connect_backend_with(kind, &self.events, &self.reservation)?,
            };
            fresh = Some(backend.devices().unwrap_or_default());
            self.connected = Some((kind, backend));
        }
        let Some((_, backend)) = &self.connected else {
            return Err(OutputError::Closed);
        };
        Ok((backend.as_ref(), fresh))
    }

    /// The connected backend, if it is `kind`.
    pub(super) fn connected(&self, kind: OutputBackendKind) -> Option<&dyn OutputBackend> {
        match &self.connected {
            Some((k, backend)) if *k == kind => Some(backend.as_ref()),
            _ => None,
        }
    }
}

/// An open stream and how it is fed.
#[derive(Debug)]
pub(super) struct Output {
    pub(super) stream: Box<dyn OutputStream>,
    pub(super) writer: SampleWriter,
    /// The format the stream was asked for; another item joins the stream when it
    /// would ask for the same.
    pub(super) requested: AudioSpec,
    /// The device and mode the stream plays to.
    pub(super) device: DeviceSelection,
    pub(super) mode: OutputMode,
    /// Where the volume is applied.
    pub(super) volume: VolumePlan,
    /// The sink takes samples as fast as they come.
    pub(super) freewheel: bool,
    /// Started and not paused.
    pub(super) running: bool,
}

impl Output {
    /// Opens a stream with `config` on `backend` and sets its volume.
    pub(super) fn open(
        backend: &dyn OutputBackend,
        mut config: StreamConfig,
        control: &VolumeControl,
        volume: Volume,
        freewheel: bool,
    ) -> Result<Self, OutputError> {
        if *control == VolumeControl::System {
            config.volume = Some(volume.amplitude());
        }
        let opened = backend.open(&config)?;
        // A hardware mixer is not driven yet; the engine scales the samples instead.
        let control = match control {
            VolumeControl::Hardware(_) => &VolumeControl::Software,
            other => other,
        };
        let plan = VolumePlan::new(control, opened.stream.has_volume(), volume);
        let mut output = Self {
            stream: opened.stream,
            writer: opened.writer,
            requested: config.spec,
            device: config.device,
            mode: config.mode,
            volume: plan,
            freewheel,
            running: false,
        };
        output.set_volume(volume);
        Ok(output)
    }

    /// The negotiated format.
    pub(super) fn spec(&self) -> AudioSpec {
        self.stream.spec()
    }

    /// Applies `volume` where the plan says.
    pub(super) fn set_volume(&mut self, volume: Volume) {
        match &mut self.volume {
            VolumePlan::Stream => {
                // A failure leaves the old volume; the stream reports its own errors.
                let _ = self.stream.set_volume(volume.amplitude());
            }
            VolumePlan::Software(current) => {
                *current = volume;
                self.writer.volume().set_volume(volume);
            }
            VolumePlan::Hardware | VolumePlan::Fixed => {}
        }
    }

    pub(super) fn start(&mut self) -> Result<(), OutputError> {
        self.stream.start()?;
        self.running = true;
        Ok(())
    }

    pub(super) fn pause(&mut self) {
        // A stream that cannot pause has ended; its state says so.
        let _ = self.stream.pause();
        self.running = false;
    }

    /// Stops the stream and closes the device.
    pub(super) fn close(mut self) {
        // Errors of a closing stream (a WAV file that could not be finished) were
        // reported as events when they happened, or are of no use now.
        let _ = self.stream.stop();
    }
}

impl Worker {
    /// Builds the current item's processing for the open stream.
    pub(super) fn attach_current(&mut self) -> Result<(), EngineError> {
        let (Some(output), Some(source)) = (&self.output, &mut self.current) else {
            return Err(EngineError::NothingLoaded);
        };
        let device = output.spec();
        let item = source.item();
        let request = PathRequest {
            source: source.format(),
            loudness: loudness_info(&item.loudness),
            album_context: item.album_context,
            device,
            volume: output.volume,
            dsp: false,
            crossfade: false,
        };
        source.attach(PathPlan::new(&request, &self.config.processing))?;
        let channels = usize::from(device.channel_count());
        let needed = source.max_output_frames().max(4096) * channels;
        if self.staging.len() < needed {
            self.staging.resize(needed, 0.0);
        }
        Ok(())
    }

    /// The stream an item would ask for.
    pub(super) fn stream_config(&self, source: &Source) -> StreamConfig {
        let mut config = stream_config(&self.config.output, &source.format());
        config.device = self.selection.device.clone();
        config.mode = self.selection.mode;
        config
    }

    /// Opens a stream; a missing device falls back to the default one if the
    /// settings say so.
    pub(super) fn open_output(&mut self, config: StreamConfig) -> Result<Output, EngineError> {
        let first = self.try_open(&config);
        let result = match first {
            Err(OutputError::DeviceNotFound { .. })
                if self.config.output.on_device_lost == DeviceLostPolicy::FallbackDefault
                    && config.device != DeviceSelection::SystemDefault =>
            {
                let config = StreamConfig {
                    device: DeviceSelection::SystemDefault,
                    ..config
                };
                self.try_open(&config)
            }
            other => other,
        };
        result.map_err(|error| error.to_engine_error())
    }

    fn try_open(&mut self, config: &StreamConfig) -> Result<Output, OutputError> {
        let selection = helikodu_model::audio::OutputSelection {
            backend: self.selection.backend,
            device: config.device.clone(),
            mode: config.mode,
        };
        let pacing = self.config.sink_pacing;
        let (backend, fresh) = self.backends.get(&selection)?;
        let kind = backend.kind();
        let freewheel = pacing == crate::output::Pacing::Freewheel && kind.is_for_testing();
        let output = Output::open(
            backend,
            config.clone(),
            &self.config.output.volume,
            self.volume,
            freewheel,
        );
        if let Some(devices) = fresh {
            self.emit(EngineEvent::DevicesChanged {
                backend: kind,
                devices,
            });
        }
        output
    }

    pub(super) fn close_output(&mut self) {
        if let Some(output) = self.output.take() {
            output.close();
        }
        self.staged = 0..0;
    }
}

/// The loudness values of an item as the path plan takes them. Tags carry sample
/// peaks, not true peaks.
fn loudness_info(loudness: &Loudness) -> LoudnessInfo {
    LoudnessInfo {
        track_gain_db: loudness.track_gain_db,
        track_peak: loudness.track_peak,
        album_gain_db: loudness.album_gain_db,
        album_peak: loudness.album_peak,
        true_peak: false,
    }
}
