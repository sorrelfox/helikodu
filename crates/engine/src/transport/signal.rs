// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The signal path of the item being heard, and the playback counters, as events.
//!
//! The path is built from the plan the item is actually processed with, the stream
//! the device was opened with and what the backend reports about the device (the
//! rate a sound server's graph runs at). The engine thread looks at these inputs on
//! every wake-up while playing and sends
//! [`SignalPath`](EngineEvent::SignalPath) when one of them has changed; the
//! comparison does not allocate, so the steady state stays allocation-free.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use helikodu_model::audio::{
    DeviceSelection, OutputBackendKind, OutputMode, SampleFormat, SampleRate,
};
use helikodu_model::player::{
    CompressorActivity, EngineEvent, ItemId, LimiterActivity, PlaybackStats, SignalDevice,
    SignalLamp, SignalPath, SignalSource, SignalStage, StageKind, StageState, TransportState,
    Volume,
};
use helikodu_model::settings::{ChainStage, DspSettings};

use super::worker::Worker;
use crate::pipeline::{LoudnessMode, PathPlan, VolumePlan};

/// How often the playback counters are sent while playing.
const STATS_INTERVAL: Duration = Duration::from_millis(250);
/// The detail of a convolution stage that is switched on but left out because its
/// impulse response cannot be used.
const LEFT_OUT: &str = "left out: the impulse response cannot be used";
/// The shortest time between two reports of the limiter's activity.
const LIMITER_INTERVAL: Duration = Duration::from_millis(200);
/// How many items' plans are remembered: the heard one and those written after it.
const PLANS: usize = 4;

/// What the engine thread remembers between wake-ups.
#[derive(Debug, Default)]
pub(super) struct SignalTracker {
    /// The latest plan of recent items, with a generation that changes with it.
    plans: VecDeque<Remembered>,
    generation: u64,
    /// The inputs of the path sent last; `None` before the first and after `None`
    /// was sent.
    sent: Option<Key>,
    last_stats: Option<Instant>,
    /// The limiter's activity sent last (`Some(None)`: that it is off), and when.
    limiter_sent: Option<Option<LimiterActivity>>,
    last_limiter: Option<Instant>,
    /// The same for the compressor.
    compressor_sent: Option<Option<CompressorActivity>>,
    last_compressor: Option<Instant>,
}

#[derive(Debug)]
struct Remembered {
    item: ItemId,
    plan: PathPlan,
    codec: Option<String>,
    generation: u64,
}

/// Everything the path depends on, compared without allocating.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    item: ItemId,
    plan: u64,
    streams: u64,
    device_rate: Option<SampleRate>,
    volume: VolumePlan,
    level: Volume,
    switches: Switches,
}

/// Which optional stages are switched on in the settings, so that a stage that does
/// nothing can say whether it is off or bypassed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Switches {
    pub(crate) loudness: bool,
    pub(crate) equalizer: bool,
    pub(crate) crossfeed: bool,
    pub(crate) mixer: bool,
    pub(crate) limiter: bool,
    pub(crate) compressor: bool,
    pub(crate) convolution: bool,
    pub(crate) pitch: bool,
}

impl Switches {
    pub(crate) fn of(loudness: LoudnessMode, dsp: &DspSettings) -> Self {
        Self {
            loudness: loudness != LoudnessMode::Off,
            equalizer: dsp.equalizer.enabled,
            crossfeed: dsp.crossfeed.enabled,
            mixer: dsp.channel_mix.enabled,
            limiter: dsp.limiter.enabled,
            compressor: dsp.compressor.enabled,
            convolution: dsp.convolution.enabled,
            pitch: dsp.pitch.enabled,
        }
    }
}

/// What the path needs besides the plan.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PathContext {
    pub(crate) item: ItemId,
    /// The decoder's codec name.
    pub(crate) codec: Option<String>,
    pub(crate) backend: OutputBackendKind,
    pub(crate) device: DeviceSelection,
    pub(crate) description: Option<String>,
    pub(crate) mode: OutputMode,
    /// The rate the device runs at, as the backend reports it.
    pub(crate) device_rate: Option<SampleRate>,
    /// Where the volume is applied now, and the volume.
    pub(crate) volume: VolumePlan,
    pub(crate) level: Volume,
    pub(crate) buffer_ms: u32,
    pub(crate) switches: Switches,
}

impl Worker {
    /// Sends the signal path of the heard item when it has changed, and the playback
    /// counters when they are due. Called on every wake-up.
    pub(super) fn update_signal(&mut self) {
        self.remember_plans();
        let key = match self.state {
            TransportState::Stopped => None,
            // While a paused device is released, or a stream is being opened, the
            // path sent last stands.
            _ => match self.signal_key() {
                Some(key) => Some(key),
                None => return,
            },
        };
        if key != self.signal.sent {
            let path = key.and_then(|key| self.signal_path(key.item));
            // A path that cannot be built yet (no plan for the heard item) is tried
            // again on the next wake-up.
            if key.is_none() || path.is_some() {
                self.signal.sent = key;
                self.emit(EngineEvent::SignalPath(path.map(Box::new)));
            }
        }
        if matches!(
            self.state,
            TransportState::Playing | TransportState::Draining
        ) && self
            .signal
            .last_stats
            .is_none_or(|last| last.elapsed() >= STATS_INTERVAL)
            && let Some(stats) = self.playback_stats()
        {
            self.signal.last_stats = Some(Instant::now());
            self.emit(EngineEvent::PlaybackStats(stats));
        }
    }

    /// Sends the limiter's activity when it has changed since it was last sent,
    /// at most every [`LIMITER_INTERVAL`]. An idle limiter does not change, so
    /// nothing is sent then. The activity is copied out of the chain, which
    /// only this thread touches; nothing waits on a lock.
    pub(super) fn update_limiter(&mut self) {
        let now = self.dsp.as_ref().and_then(|dsp| dsp.limiter_activity());
        if self.signal.limiter_sent == Some(now) {
            return;
        }
        // Turning the limiter on or off is told at once; activity is paced.
        let switched = self
            .signal
            .limiter_sent
            .is_none_or(|sent| sent.is_some() != now.is_some());
        if !switched
            && self
                .signal
                .last_limiter
                .is_some_and(|last| last.elapsed() < LIMITER_INTERVAL)
        {
            return;
        }
        self.signal.limiter_sent = Some(now);
        self.signal.last_limiter = Some(Instant::now());
        self.emit(EngineEvent::Limiter(now));
    }

    /// Sends the compressor's activity as [`update_limiter`](Self::update_limiter)
    /// sends the limiter's.
    pub(super) fn update_compressor(&mut self) {
        let now = self.dsp.as_ref().and_then(|dsp| dsp.compressor_activity());
        if self.signal.compressor_sent == Some(now) {
            return;
        }
        let switched = self
            .signal
            .compressor_sent
            .is_none_or(|sent| sent.is_some() != now.is_some());
        if !switched
            && self
                .signal
                .last_compressor
                .is_some_and(|last| last.elapsed() < LIMITER_INTERVAL)
        {
            return;
        }
        self.signal.compressor_sent = Some(now);
        self.signal.last_compressor = Some(Instant::now());
        self.emit(EngineEvent::Compressor(now));
    }

    /// Starts the limiter's counts anew for the item that is now heard.
    pub(super) fn reset_limiter(&mut self) {
        if let Some(dsp) = &mut self.dsp {
            dsp.reset_limiter_activity();
        }
    }

    /// Sends that nothing is heard, unless that was the last thing sent.
    pub(super) fn clear_signal(&mut self) {
        if self.signal.sent.take().is_some() {
            self.emit(EngineEvent::SignalPath(None));
        }
    }

    /// Notes the plan of the item being rendered, if it is new or changed.
    fn remember_plans(&mut self) {
        let Some(source) = &self.current else {
            return;
        };
        let Some(plan) = source.plan() else {
            return;
        };
        let item = source.item().id;
        let tracker = &mut self.signal;
        if let Some(entry) = tracker.plans.iter_mut().find(|r| r.item == item) {
            if entry.plan != *plan {
                tracker.generation += 1;
                entry.plan = plan.clone();
                entry.generation = tracker.generation;
            }
            return;
        }
        tracker.generation += 1;
        if tracker.plans.len() == PLANS {
            tracker.plans.pop_front();
        }
        tracker.plans.push_back(Remembered {
            item,
            plan: plan.clone(),
            codec: Some(source.codec().0),
            generation: tracker.generation,
        });
    }

    /// The inputs of the path now, or `None` without a stream or a heard item.
    fn signal_key(&self) -> Option<Key> {
        let output = self.output.as_ref()?;
        let item = self.timeline.heard()?.item.id;
        let plan = self
            .signal
            .plans
            .iter()
            .find(|r| r.item == item)
            .map_or(0, |r| r.generation);
        Some(Key {
            item,
            plan,
            streams: self.stats.snapshot().streams_opened,
            device_rate: output.stream.timing().device_rate,
            volume: output.volume,
            level: self.volume,
            switches: Switches::of(self.config.processing.loudness.mode, &self.config.dsp),
        })
    }

    /// Builds the path of `item` on the open stream.
    fn signal_path(&self, item: ItemId) -> Option<SignalPath> {
        let output = self.output.as_ref()?;
        let remembered = self.signal.plans.iter().find(|r| r.item == item)?;
        let backend = output.stream.backend();
        // The list as last reported: listing again could probe the hardware.
        let description = self
            .backends
            .devices
            .iter()
            .find(|device| {
                device.backend == backend
                    && match &output.device {
                        DeviceSelection::Device(id) => device.id == *id,
                        DeviceSelection::SystemDefault => device.is_default,
                    }
            })
            .map(|device| device.description.clone());
        let context = PathContext {
            item,
            codec: remembered.codec.clone(),
            backend,
            device: output.device.clone(),
            description,
            mode: output.mode,
            device_rate: output.stream.timing().device_rate,
            volume: output.volume,
            level: self.volume,
            buffer_ms: output.settings.buffer_ms,
            switches: Switches::of(self.config.processing.loudness.mode, &self.config.dsp),
        };
        Some(signal_path(&remembered.plan, &context))
    }

    /// The counters of the open stream.
    fn playback_stats(&self) -> Option<PlaybackStats> {
        let output = self.output.as_ref()?;
        let timing = output.stream.timing();
        let rate = u64::from(timing.rate.hz().max(1));
        let ms = |frames: u64| u32::try_from(frames * 1000 / rate).unwrap_or(u32::MAX);
        Some(PlaybackStats {
            buffer_ms: ms(output.writer.capacity_frames() as u64),
            buffered_ms: ms(timing.queued_frames()),
            underruns: timing.underruns,
            frames_delivered: timing.frames_delivered,
            silent_frames: timing.silent_frames,
            total_underruns: self.stats.snapshot().underruns,
        })
    }
}

/// The signal path of `plan` played as `context` says.
pub(crate) fn signal_path(plan: &PathPlan, context: &PathContext) -> SignalPath {
    let source = plan.source;
    let device = plan.device;
    let pipewire = context.backend == OutputBackendKind::PipeWire;
    let source_int = source.spec.format.is_integer();
    let source_bits = if source_int {
        source.significant_bits()
    } else {
        24
    };
    let volume_touches = match context.volume {
        VolumePlan::Software(_) => context.level != Volume::FULL,
        _ => false,
    };
    let mut stages = Vec::with_capacity(15);
    let mut stage = |kind, state, detail: String| {
        stages.push(SignalStage {
            kind,
            state,
            detail,
            delay_ms: 0.0,
        });
    };
    // The delays of the DSP stages that add one, set once every stage is listed.
    let mut delays: Vec<(StageKind, f64)> = Vec::new();

    let codec = context.codec.clone().unwrap_or_default();
    let lossy = is_lossy_codec(&codec);
    stage(
        StageKind::Decoder,
        if lossy {
            StageState::Active
        } else {
            StageState::Bypassed
        },
        codec,
    );
    match plan.channel_mix {
        Some(mix) => stage(StageKind::ChannelMix, StageState::Active, mix.to_string()),
        None => stage(
            StageKind::ChannelMix,
            StageState::NotNeeded,
            source.spec.layout.to_string(),
        ),
    }
    match plan.gain {
        Some(gain) => stage(StageKind::Loudness, StageState::Active, gain.to_string()),
        None if context.switches.loudness => {
            stage(StageKind::Loudness, StageState::Bypassed, "0 dB".into());
        }
        None => stage(StageKind::Loudness, StageState::Off, String::new()),
    }
    // The playback speed shows with the resampler, which carries it out at the
    // tape kind.
    match (plan.resample, plan.speed) {
        (Some(resample), Some(speed)) => stage(
            StageKind::Resampler,
            StageState::Active,
            format!("{speed}, {resample}"),
        ),
        (None, Some(speed)) => stage(StageKind::Resampler, StageState::Active, speed.to_string()),
        (Some(resample), None) => stage(
            StageKind::Resampler,
            StageState::Active,
            resample.to_string(),
        ),
        (None, None) => stage(
            StageKind::Resampler,
            StageState::NotNeeded,
            kilohertz(device.rate),
        ),
    }
    if plan.crossfade {
        stage(StageKind::Crossfade, StageState::Active, "crossfade".into());
    } else {
        stage(StageKind::Crossfade, StageState::NotNeeded, String::new());
    }
    let dsp = &plan.dsp_stages;
    let switches = context.switches;
    let mut dsp_stage = |kind, active: Option<String>, on: bool, left_out: bool| match active {
        Some(detail) => stage(kind, StageState::Active, detail),
        None if on && left_out => stage(kind, StageState::Bypassed, LEFT_OUT.into()),
        None if on => stage(kind, StageState::Bypassed, String::new()),
        None => stage(kind, StageState::Off, String::new()),
    };
    for chain_stage in dsp.order {
        let (kind, on) = match chain_stage {
            ChainStage::Pitch => (StageKind::Pitch, switches.pitch),
            ChainStage::Equalizer => (StageKind::Equalizer, switches.equalizer),
            ChainStage::Convolution => (StageKind::Convolution, switches.convolution),
            ChainStage::Crossfeed => (StageKind::Crossfeed, switches.crossfeed),
            ChainStage::ChannelMix => (StageKind::ChannelMixer, switches.mixer),
            ChainStage::Compressor => (StageKind::Compressor, switches.compressor),
            ChainStage::Limiter => (StageKind::Limiter, switches.limiter),
        };
        let planned = dsp.stage(chain_stage);
        if let Some(planned) = planned
            && planned.delay_ms() > 0.0
        {
            delays.push((kind, planned.delay_ms()));
        }
        // Convolution that is switched on but cannot use its impulse response
        // says so; the front ends name the problem.
        let left_out = chain_stage == ChainStage::Convolution && dsp.convolution_left_out;
        dsp_stage(kind, planned.map(|s| s.to_string()), on, left_out);
    }
    let percent = |volume: Volume| format!("{:.0} %", volume.position() * 100.0);
    match context.volume {
        VolumePlan::Software(_) if volume_touches => stage(
            StageKind::Volume,
            StageState::Active,
            format!("volume {}", percent(context.level)),
        ),
        VolumePlan::Software(_) => stage(StageKind::Volume, StageState::Bypassed, "100 %".into()),
        VolumePlan::Stream => stage(
            StageKind::Volume,
            StageState::External,
            format!("system mixer {}", percent(context.level)),
        ),
        VolumePlan::Hardware => stage(
            StageKind::Volume,
            StageState::External,
            "hardware mixer".into(),
        ),
        VolumePlan::Fixed => stage(
            StageKind::Volume,
            StageState::External,
            "fixed 100 %".into(),
        ),
    }
    let processing = plan.gain.is_some()
        || plan.channel_mix.is_some()
        || plan.resample.is_some()
        || plan.dsp
        || plan.crossfade
        || volume_touches;
    match plan.dither {
        Some(bits) if processing || plan.always_dither => stage(
            StageKind::Dither,
            StageState::Active,
            format!("TPDF dither {bits}-bit"),
        ),
        _ => stage(StageKind::Dither, StageState::NotNeeded, String::new()),
    }

    // What the device format does to the samples, and what the backend does after
    // the engine.
    let device_bits = match device.format {
        SampleFormat::F64(_) => 53,
        format if format.is_float() => 24,
        format => format.bits(),
    };
    let carried_bits = if pipewire {
        device_bits.min(24)
    } else {
        device_bits
    };
    let same_format = source.spec.format == device.format;
    let exact = if source_int && device.format.is_integer() {
        source_bits <= device_bits
    } else if source_int {
        // Integers of up to 24 bits are exact floats; PipeWire turns them back into
        // the device's integers, a file or float device keeps them as floats.
        source_bits <= carried_bits && pipewire
    } else {
        same_format
    };
    let server_rate = context.device_rate.filter(|rate| *rate != device.rate);
    let mut conversion = format!("{} → {}", source.spec.format, device.format);
    if let Some(rate) = server_rate {
        conversion.push_str(&format!(
            " · resampled by the server {} → {}",
            kilohertz(device.rate),
            kilohertz(rate)
        ));
    }
    let conversion_state = if same_format && server_rate.is_none() {
        StageState::NotNeeded
    } else if exact && server_rate.is_none() {
        StageState::Bypassed
    } else {
        StageState::Active
    };
    stage(StageKind::Conversion, conversion_state, conversion);

    // Whether the device gets the stream untouched by anything after the engine.
    let direct = match context.backend {
        OutputBackendKind::Alsa => matches!(
            &context.device,
            DeviceSelection::Device(id) if id.as_str().starts_with("hw:")
        ),
        OutputBackendKind::PipeWire => context.mode == OutputMode::Exclusive,
        OutputBackendKind::Null | OutputBackendKind::Wav => true,
        _ => false,
    };
    let server_volume =
        pipewire && context.volume == VolumePlan::Stream && context.level != Volume::FULL;
    let lamp = if source_bits > carried_bits {
        SignalLamp::Lossy
    } else if plan.resample.is_some() || server_rate.is_some() {
        SignalLamp::Resampled
    } else if processing
        || plan.always_dither && !exact
        || conversion_state == StageState::Active
        || !direct
        || server_volume
    {
        SignalLamp::LosslessProcessed
    } else {
        SignalLamp::BitPerfect
    };

    for (kind, delay_ms) in delays {
        if let Some(stage) = stages.iter_mut().find(|s| s.kind == kind) {
            stage.delay_ms = delay_ms;
        }
    }
    SignalPath {
        item: context.item,
        source: SignalSource {
            spec: source.spec,
            bits: if source_int {
                source_bits
            } else {
                source.spec.format.bits()
            },
            codec: context.codec.clone().filter(|codec| !codec.is_empty()),
        },
        stages,
        device: SignalDevice {
            backend: context.backend,
            device: context.device.clone(),
            description: context.description.clone(),
            mode: context.mode,
            format: device,
            device_rate: server_rate,
            exact_bits: (carried_bits < device_bits || pipewire).then_some(carried_bits),
            buffer_ms: context.buffer_ms,
        },
        lamp,
    }
}

/// Whether a codec (by FFmpeg's or the DSD backend's name) reconstructs an
/// approximation rather than the original samples.
fn is_lossy_codec(codec: &str) -> bool {
    const LOSSY: [&str; 22] = [
        "mp1", "mp2", "mp3", "aac", "vorbis", "opus", "wmav", "wmapro", "wmavoice", "ac3", "eac3",
        "dca", "dts", "atrac", "mpc", "musepack", "speex", "amr", "gsm", "adpcm", "cook", "dsd",
    ];
    LOSSY.iter().any(|prefix| codec.starts_with(prefix))
}

fn kilohertz(rate: SampleRate) -> String {
    let text = format!("{:.3}", f64::from(rate.hz()) / 1000.0);
    format!("{} kHz", text.trim_end_matches('0').trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
    use helikodu_model::audio::{AudioSpec, ChannelLayout, DeviceId, Endianness};

    use super::*;
    use crate::pipeline::{
        LoudnessInfo, LoudnessSettings, PathRequest, ProcessingSettings, SourceFormat,
    };

    const LE: Endianness = Endianness::Little;

    fn spec(format: SampleFormat, hz: u32, layout: ChannelLayout) -> AudioSpec {
        AudioSpec::new(format, SampleRate::new(hz).unwrap(), layout)
    }

    fn plan(
        source: SourceFormat,
        device: AudioSpec,
        edit: impl FnOnce(&mut PathRequest),
    ) -> PathPlan {
        let mut request = PathRequest {
            source,
            loudness: LoudnessInfo::default(),
            album_context: false,
            track_volume_db: 0.0,
            device,
            volume: VolumePlan::Fixed,
            dsp: false,
            crossfade: false,
        };
        edit(&mut request);
        let settings = ProcessingSettings {
            loudness: LoudnessSettings {
                mode: LoudnessMode::Track,
                ..LoudnessSettings::default()
            },
            ..ProcessingSettings::default()
        };
        PathPlan::new(&request, &settings)
    }

    fn context(backend: OutputBackendKind, device: &str, mode: OutputMode) -> PathContext {
        PathContext {
            item: ItemId::new(1),
            codec: Some("flac".into()),
            backend,
            device: DeviceSelection::Device(DeviceId::new(device).unwrap()),
            description: Some("DAC".into()),
            mode,
            device_rate: None,
            volume: VolumePlan::Fixed,
            level: Volume::FULL,
            buffer_ms: 200,
            switches: Switches::default(),
        }
    }

    fn state(path: &SignalPath, kind: StageKind) -> StageState {
        path.stage(kind).unwrap().state
    }

    #[test]
    fn untouched_32_bit_audio_on_alsa_hw_is_bit_perfect() {
        let s32 = spec(SampleFormat::S32(LE), 96_000, ChannelLayout::STEREO);
        let plan = plan(SourceFormat::new(s32), s32, |_| {});
        let path = signal_path(
            &plan,
            &context(
                OutputBackendKind::Alsa,
                "hw:CARD=DAC,DEV=0",
                OutputMode::Exclusive,
            ),
        );
        assert_eq!(path.lamp, SignalLamp::BitPerfect);
        assert_eq!(path.active_stages().count(), 0);
        assert_eq!(state(&path, StageKind::Conversion), StageState::NotNeeded);
        assert_eq!(state(&path, StageKind::Volume), StageState::External);
        assert_eq!(state(&path, StageKind::Loudness), StageState::Off);
        assert_eq!(state(&path, StageKind::Decoder), StageState::Bypassed);
        assert_eq!(path.stages.len(), 15);
        assert_eq!(path.source.bits, 32);
        assert_eq!(path.device.exact_bits, None);
        assert_eq!(
            path.to_string(),
            "flac 96 kHz/32-bit stereo → bit-perfect → alsa \"DAC\" s32le 96 kHz stereo \
             (exclusive) [bit-perfect]"
        );
        // Through `default` the plug layer or a server may change it.
        let path = signal_path(
            &plan,
            &context(OutputBackendKind::Alsa, "pipewire", OutputMode::Shared),
        );
        assert_eq!(path.lamp, SignalLamp::LosslessProcessed);
    }

    #[test]
    fn pipewire_is_exact_to_24_bits_and_only_in_exclusive_mode() {
        let s24 = SourceFormat::new(spec(SampleFormat::S32(LE), 48_000, ChannelLayout::STEREO))
            .with_bits(24);
        let f32 = spec(SampleFormat::F32(LE), 48_000, ChannelLayout::STEREO);
        let plan = plan(s24, f32, |r| r.volume = VolumePlan::Stream);
        let mut context = context(
            OutputBackendKind::PipeWire,
            "alsa_output.dac",
            OutputMode::Exclusive,
        );
        context.volume = VolumePlan::Stream;
        let path = signal_path(&plan, &context);
        assert_eq!(path.lamp, SignalLamp::BitPerfect);
        assert_eq!(path.device.exact_bits, Some(24));
        assert_eq!(state(&path, StageKind::Conversion), StageState::Bypassed);
        // The stream volume below 100 % is applied by the server.
        context.level = Volume::new(0.5).unwrap();
        assert_eq!(
            signal_path(&plan, &context).lamp,
            SignalLamp::LosslessProcessed
        );
        context.level = Volume::FULL;
        // Shared: the server may mix.
        context.mode = OutputMode::Shared;
        assert_eq!(
            signal_path(&plan, &context).lamp,
            SignalLamp::LosslessProcessed
        );
        // The graph runs at another rate: the server resamples.
        context.device_rate = Some(SampleRate::HZ_44100);
        let path = signal_path(&plan, &context);
        assert_eq!(path.lamp, SignalLamp::Resampled);
        assert_eq!(path.device.device_rate, Some(SampleRate::HZ_44100));
        assert_eq!(state(&path, StageKind::Conversion), StageState::Active);
        // 32-bit sources lose their low bits in the server's float path.
        let s32 = SourceFormat::new(spec(SampleFormat::S32(LE), 48_000, ChannelLayout::STEREO));
        context.device_rate = None;
        context.mode = OutputMode::Exclusive;
        assert_eq!(
            signal_path(&plan_of(s32, f32), &context).lamp,
            SignalLamp::Lossy
        );
    }

    fn plan_of(source: SourceFormat, device: AudioSpec) -> PathPlan {
        plan(source, device, |_| {})
    }

    #[test]
    fn processing_stages_and_lamps() {
        let hi = SourceFormat::new(spec(
            SampleFormat::S32(LE),
            96_000,
            ChannelLayout::SURROUND_5_1,
        ))
        .with_bits(24);
        let cd = spec(SampleFormat::S16(LE), 48_000, ChannelLayout::STEREO);
        let plan = plan(hi, cd, |r| {
            r.loudness.track_gain_db = Some(-6.3);
        });
        let mut context = context(
            OutputBackendKind::Alsa,
            "hw:CARD=DAC,DEV=0",
            OutputMode::Exclusive,
        );
        context.codec = Some("mp3".into());
        context.volume = VolumePlan::Software(Volume::new(0.85).unwrap());
        context.level = Volume::new(0.85).unwrap();
        context.switches.equalizer = true;
        let path = signal_path(&plan, &context);
        // 24 bits into a 16-bit device: bits are lost, the worst of all.
        assert_eq!(path.lamp, SignalLamp::Lossy);
        assert_eq!(state(&path, StageKind::Decoder), StageState::Active);
        assert_eq!(state(&path, StageKind::ChannelMix), StageState::Active);
        assert_eq!(state(&path, StageKind::Loudness), StageState::Active);
        assert_eq!(state(&path, StageKind::Resampler), StageState::Active);
        assert_eq!(state(&path, StageKind::Equalizer), StageState::Bypassed);
        assert_eq!(state(&path, StageKind::Limiter), StageState::Off);
        assert_eq!(state(&path, StageKind::Volume), StageState::Active);
        assert_eq!(state(&path, StageKind::Dither), StageState::Active);
        assert!(!path.dsp_active());
        assert_eq!(
            path.to_string(),
            "mp3 96 kHz/24-bit 5.1 → downmix 5.1 → stereo → ReplayGain -6.30 dB (track) → \
             resample 96 → 48 kHz (high) → volume 85 % → TPDF dither 16-bit → s32le → s16le → \
             alsa \"DAC\" s16le 48 kHz stereo (exclusive) [lossy]"
        );
        // The same at the source's rate and width: processed, nothing lost.
        let wide = spec(SampleFormat::S32(LE), 96_000, ChannelLayout::SURROUND_5_1);
        let plan = plan_of(hi, wide);
        assert_eq!(
            signal_path(&plan, &context).lamp,
            SignalLamp::LosslessProcessed
        );
        // Resampled only.
        let plan = plan_of(
            hi,
            spec(SampleFormat::S32(LE), 48_000, ChannelLayout::SURROUND_5_1),
        );
        context.volume = VolumePlan::Fixed;
        context.level = Volume::FULL;
        assert_eq!(signal_path(&plan, &context).lamp, SignalLamp::Resampled);
    }

    #[test]
    fn dsp_stages_give_their_delay_and_a_convolution_left_out() {
        use crate::dsp::{LimiterPlan, PitchPlan};
        let s32 = spec(SampleFormat::S32(LE), 48_000, ChannelLayout::STEREO);
        let mut plan = plan(SourceFormat::new(s32), s32, |r| r.dsp = true);
        plan.dsp_stages.limiter = Some(LimiterPlan {
            lookahead_ms: 2.0,
            ceiling_db: -1.0,
        });
        plan.dsp_stages.pitch = Some(PitchPlan {
            semitones: 2.0,
            latency_ms: 33.0,
        });
        plan.dsp_stages.convolution_left_out = true;
        let mut context = context(
            OutputBackendKind::Alsa,
            "hw:CARD=DAC,DEV=0",
            OutputMode::Exclusive,
        );
        context.switches.limiter = true;
        context.switches.pitch = true;
        context.switches.convolution = true;
        let path = signal_path(&plan, &context);
        assert_eq!(path.stage(StageKind::Limiter).unwrap().delay_ms, 2.0);
        assert_eq!(path.stage(StageKind::Pitch).unwrap().delay_ms, 33.0);
        let convolution = path.stage(StageKind::Convolution).unwrap();
        assert_eq!(convolution.state, StageState::Bypassed);
        assert!(convolution.detail.starts_with("left out"));
        assert_eq!(convolution.delay_ms, 0.0);
    }

    #[test]
    fn float_sources_and_files() {
        let float = spec(SampleFormat::F32(LE), 44_100, ChannelLayout::STEREO);
        let context = context(OutputBackendKind::Wav, "out.wav", OutputMode::Shared);
        let path = signal_path(&plan_of(SourceFormat::new(float), float), &context);
        assert_eq!(path.lamp, SignalLamp::BitPerfect);
        assert_eq!(path.source.bits, 32);
        // 16-bit audio written as floats is exact in value but converted.
        let cd = spec(SampleFormat::S16(LE), 44_100, ChannelLayout::STEREO);
        let path = signal_path(&plan_of(SourceFormat::new(cd), float), &context);
        assert_eq!(path.lamp, SignalLamp::LosslessProcessed);
        assert_eq!(state(&path, StageKind::Conversion), StageState::Active);
        // 16 bits in a 24-bit container are untouched.
        let wide = spec(SampleFormat::S24(LE), 44_100, ChannelLayout::STEREO);
        let path = signal_path(&plan_of(SourceFormat::new(cd), wide), &context);
        assert_eq!(path.lamp, SignalLamp::BitPerfect);
        assert_eq!(state(&path, StageKind::Conversion), StageState::Bypassed);
    }

    #[test]
    fn lossy_codecs() {
        assert!(is_lossy_codec("mp3float"));
        assert!(is_lossy_codec("aac"));
        assert!(is_lossy_codec("opus"));
        assert!(!is_lossy_codec("flac"));
        assert!(!is_lossy_codec("wmalossless"));
        assert!(!is_lossy_codec("pcm_s24le"));
        assert!(!is_lossy_codec("alac"));
    }
}
