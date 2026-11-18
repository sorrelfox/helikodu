// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The output the engine plays to: the connected backend and the open stream.

use helikodu_model::audio::{
    AudioSpec, BackendSelection, DeviceId, DeviceInfo, DeviceKey, DeviceSelection,
    OutputBackendKind, OutputMode, OutputSelection,
};
use helikodu_model::player::Volume;
use helikodu_model::settings::{OutputSettings, VolumeControl};

use helikodu_model::player::{EngineError, EngineEvent, Loudness};
use helikodu_model::settings::DeviceLostPolicy;

use super::source::Source;
use super::stats::{EngineStats, Reported};
use super::worker::Worker;
use crate::dsp::DspChain;
use crate::output::{
    EventSender, NullBackend, OutputBackend, OutputError, OutputStream, Pacing, SampleWriter,
    SharedConnector, SharedReservation, StreamConfig, WavBackend, automatic_backend,
    connect_backend_with,
};
use crate::pipeline::{
    LoudnessInfo, PathPlan, PathRequest, ProcessingSettings, VolumePlan, stream_config,
};

/// The connected backend, kept between streams: connecting to a sound server takes
/// time.
#[derive(Debug)]
pub(super) struct Backends {
    connected: Option<(OutputBackendKind, Box<dyn OutputBackend>)>,
    events: EventSender,
    pacing: Pacing,
    reservation: SharedReservation,
    connector: SharedConnector,
    /// The default device of the connected backend, as last reported.
    pub(super) default_device: Option<DeviceId>,
    /// The connected backend's devices, as last reported (for their descriptions).
    pub(super) devices: Vec<DeviceInfo>,
}

impl Backends {
    pub(super) fn new(
        events: EventSender,
        pacing: Pacing,
        reservation: SharedReservation,
        connector: SharedConnector,
    ) -> Self {
        Self {
            connected: None,
            events,
            pacing,
            reservation,
            connector,
            default_device: None,
            devices: Vec::new(),
        }
    }

    /// The backend `selection` stands for: the connected one for "automatic" once
    /// there is one.
    pub(super) fn kind_of(&self, selection: BackendSelection) -> OutputBackendKind {
        match (selection, &self.connected) {
            (BackendSelection::Backend(kind), _) => kind,
            (BackendSelection::Automatic, Some((kind, _))) => *kind,
            (BackendSelection::Automatic, None) => automatic_backend(),
        }
    }

    /// Notes the default device in a device list of the connected backend.
    pub(super) fn note_devices(&mut self, devices: &[DeviceInfo]) {
        self.default_device = devices
            .iter()
            .find(|device| device.is_default)
            .map(|device| device.id.clone());
        self.devices = devices.to_vec();
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
            self.default_device = None;
            self.devices.clear();
            let custom = self.connector.connect(kind, &self.events).transpose()?;
            let backend: Box<dyn OutputBackend> = match (custom, kind) {
                (Some(custom), _) => custom,
                (None, OutputBackendKind::Null) => Box::new(NullBackend::with_pacing(self.pacing)),
                (None, OutputBackendKind::Wav) => {
                    let DeviceSelection::Device(path) = &selection.device else {
                        return Err(OutputError::BackendUnavailable {
                            backend: kind,
                            reason: "needs a file name as its device".into(),
                        });
                    };
                    Box::new(WavBackend::new(path.as_str()).with_pacing(self.pacing))
                }
                (None, _) => connect_backend_with(kind, &self.events, &self.reservation)?,
            };
            let devices = backend.devices().unwrap_or_default();
            self.note_devices(&devices);
            fresh = Some(devices);
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
    /// The output settings with the device's profile applied.
    pub(super) settings: OutputSettings,
    /// Where the volume is applied.
    pub(super) volume: VolumePlan,
    /// The sink takes samples as fast as they come.
    pub(super) freewheel: bool,
    /// Started and not paused.
    pub(super) running: bool,
    /// Frames of DAC warm-up silence still to write before the first audio.
    pub(super) preroll_left: u64,
    /// The engine's counters, and what this stream has added to them.
    stats: EngineStats,
    reported: Reported,
}

impl Output {
    /// Opens a stream with `config` on `backend` and sets its volume.
    pub(super) fn open(
        backend: &dyn OutputBackend,
        mut config: StreamConfig,
        settings: OutputSettings,
        volume: Volume,
        freewheel: bool,
        stats: EngineStats,
    ) -> Result<Self, OutputError> {
        let control = &settings.volume;
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
            settings,
            volume: plan,
            freewheel,
            running: false,
            preroll_left: 0,
            stats,
            reported: Reported::default(),
        };
        output.stats.stream_opened();
        output.set_volume(volume);
        Ok(output)
    }

    /// The negotiated format.
    pub(super) fn spec(&self) -> AudioSpec {
        self.stream.spec()
    }

    /// The frames of silence the DAC warm-up (`preroll_ms`) asks for before the
    /// first audio of the stream.
    pub(super) fn preroll_frames(&self) -> u64 {
        u64::from(self.settings.preroll_ms) * u64::from(self.spec().rate.hz()) / 1000
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

    /// Adds what the stream has done since the last call to the engine's counters.
    pub(super) fn publish_stats(&mut self) {
        let timing = self.stream.timing();
        self.reported.publish(&timing, &self.stats);
    }

    /// Stops the stream and closes the device.
    pub(super) fn close(mut self) {
        self.publish_stats();
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
        let channels = usize::from(device.channel_count());
        // The chain lives as long as the stream format: a gapless join keeps it.
        if !self
            .dsp
            .as_ref()
            .is_some_and(|dsp| dsp.fits(device.rate.hz(), channels))
        {
            self.dsp = Some(DspChain::new(&self.config.dsp, device.rate.hz(), channels));
        }
        let dsp = self.dsp.as_ref().map(DspChain::plan).unwrap_or_default();
        let mut plan = plan_for(&self.config.processing, output, source).with_dsp(dsp);
        if source.decoder_info().dop && !super::formats::keeps_dop(&plan) {
            // DoP needs every bit to reach the DAC; this path would change them.
            source.fall_back_to_pcm()?;
            plan = plan_for(&self.config.processing, output, source).with_dsp(dsp);
        }
        source.attach(plan)?;
        // The loop goes with the item, whichever source plays it.
        let range = self
            .ab_loop
            .filter(|(id, _)| *id == source.item().id)
            .map(|(_, range)| range);
        source.set_loop(range);
        let needed = source.max_output_frames().max(4096) * channels;
        if self.staging.len() < needed {
            self.staging.resize(needed, 0.0);
        }
        Ok(())
    }

    /// The output settings for playing to `device` of the chosen backend, in the
    /// chosen mode: the general ones with the device's profile applied.
    pub(super) fn output_settings_for(&self, device: &DeviceSelection) -> OutputSettings {
        let kind = self.backends.kind_of(self.selection.backend);
        let mut general = self.config.output.clone();
        general.backend = self.selection.backend;
        general.device = device.clone();
        general.mode = self.selection.mode;
        general.for_device(&DeviceKey::of_selection(kind, device))
    }

    /// The stream an item would ask for.
    pub(super) fn stream_config(&self, source: &Source) -> StreamConfig {
        let settings = self.output_settings_for(&self.selection.device);
        let mut config = stream_config(&settings, &source.format());
        config.device = self.selection.device.clone();
        config.mode = settings.mode;
        config.tap = Some(self.config.tap.clone());
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
                // The default device's profile chooses the mode and the buffer; the
                // format stays as the item asked for it.
                let settings = self.output_settings_for(&DeviceSelection::SystemDefault);
                let config = StreamConfig {
                    device: DeviceSelection::SystemDefault,
                    mode: settings.mode,
                    buffer: std::time::Duration::from_millis(u64::from(settings.buffer_ms)),
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
        let settings = self.output_settings_for(&config.device);
        let (backend, fresh) = self.backends.get(&selection)?;
        let kind = backend.kind();
        let freewheel = pacing == crate::output::Pacing::Freewheel && kind.is_for_testing();
        let output = Output::open(
            backend,
            config.clone(),
            settings,
            self.volume,
            freewheel,
            self.stats.clone(),
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

/// The processing plan of `source` on `output` with `settings`.
pub(super) fn plan_for(
    settings: &ProcessingSettings,
    output: &Output,
    source: &Source,
) -> PathPlan {
    let item = source.item();
    let request = PathRequest {
        source: source.format(),
        loudness: loudness_info(&item.loudness),
        album_context: item.album_context,
        track_volume_db: item.track_volume_db,
        device: output.spec(),
        volume: output.volume,
        dsp: false,
        crossfade: false,
    };
    PathPlan::new(&request, settings)
}

/// The loudness values of an item as the path plan takes them. Measured values
/// carry true peaks, tags sample peaks; the item says which.
fn loudness_info(loudness: &Loudness) -> LoudnessInfo {
    LoudnessInfo {
        track_gain_db: loudness.track_gain_db,
        track_peak: loudness.track_peak,
        album_gain_db: loudness.album_gain_db,
        album_peak: loudness.album_peak,
        true_peak: loudness.true_peak,
    }
}
