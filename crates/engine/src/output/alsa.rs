// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The native ALSA backend.
//!
//! Devices are the playback PCMs of every card, named `hw:CARD=<id>,DEV=<n>` with
//! the card's ID string (never its index, which changes between boots), and the
//! PCMs defined in the ALSA configuration that do not belong to one card (such
//! as `pipewire` or `pulse`). "System default" opens the `default` PCM.
//!
//! In exclusive mode the device is opened without the rate converter of the plug
//! layer, and the format, rate and channel count are negotiated exactly; what
//! the device refuses is reported in the negotiated spec. In shared mode alsa-lib
//! may convert.
//!
//! A sound server usually holds every card, so a `hw:` or `plughw:` device is
//! busy. The backend takes the card through the injected
//! [`DeviceReservation`](super::DeviceReservation) (see
//! [`with_reservation`](AlsaBackend::with_reservation)): in exclusive mode before
//! it opens the device, in shared mode when the device turns out to be busy. The
//! stream keeps the card until it is stopped or dropped, and gives it back after
//! closing the device. Without a reservation, or when the holder refuses, the
//! device fails with [`OutputError::DeviceBusy`], naming the program that holds
//! it.
//!
//! A writer thread per stream takes the place of an audio callback. It does not
//! ask for real-time scheduling yet.

mod devices;
mod reserve;
mod writer;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use ::alsa as alsa_lib;
use alsa_lib::pcm::{Access, Format, HwParams, PCM};
use alsa_lib::{Direction, ValueOr};
use helikodu_model::audio::{
    AudioSpec, ChannelLayout, DeviceInfo, DeviceSelection, Endianness, OutputBackendKind,
    OutputMode, SampleFormat, SampleRate,
};

use self::writer::{AlsaStream, Negotiated};
use super::convert::Encoder;
use super::{
    EventSender, OpenedStream, OutputBackend, OutputError, ReservedCard, SharedReservation,
    StreamConfig,
};

const KIND: OutputBackendKind = OutputBackendKind::Alsa;
/// The hardware buffer the backend asks for, in microseconds. The ring buffer in
/// front of it holds the render-ahead; this only has to bridge scheduling delays.
const HW_BUFFER_US: u32 = 100_000;
/// The number of periods in the hardware buffer.
const PERIODS: u32 = 4;
/// The rates taken as a device's own when no rate is asked for, best first.
const DEFAULT_RATES: [u32; 2] = [48_000, 44_100];

/// The ALSA backend.
///
/// While it exists, a thread looks for sound cards that appear or disappear (every
/// two seconds, from `/proc/asound/cards`) and reports them on the event channel.
pub struct AlsaBackend {
    events: EventSender,
    reservation: SharedReservation,
    watcher: Option<(JoinHandle<()>, Arc<AtomicBool>)>,
}

impl std::fmt::Debug for AlsaBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlsaBackend").finish_non_exhaustive()
    }
}

impl AlsaBackend {
    /// An ALSA backend reporting device changes and stream events on `events`.
    pub fn new(events: EventSender) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let watcher = thread::Builder::new()
            .name("helikodu-alsa-cards".into())
            .spawn({
                let events = events.clone();
                let stop = stop.clone();
                move || devices::watch_cards(&events, &stop)
            })
            .ok()
            .map(|thread| (thread, stop));
        Self {
            events,
            reservation: SharedReservation::default(),
            watcher,
        }
    }

    /// Uses `reservation` to take cards that another program holds. Without it,
    /// such devices are reported busy.
    #[must_use]
    pub fn with_reservation(mut self, reservation: SharedReservation) -> Self {
        self.reservation = reservation;
        self
    }

    /// Opens the PCM `name`, taking `card` first or when busy.
    fn open_pcm(
        &self,
        config: &StreamConfig,
        name: &str,
        card: Option<&ReservedCard>,
    ) -> Result<OpenedStream, OutputError> {
        let target = reserve::Target {
            reservation: &self.reservation,
            card,
            exclusive: config.mode == OutputMode::Exclusive,
            device: &config.device,
            name,
        };
        // Non-blocking, so a busy device fails at once instead of waiting.
        let (pcm, reservation) = reserve::open(
            &target,
            || PCM::new(name, Direction::Playback, true),
            |error| open_error(&config.device, name, error),
        )?;
        let negotiated = negotiate(&pcm, config)?;
        AlsaStream::spawn(pcm, config, negotiated, self.events.clone(), reservation)
    }
}

impl Drop for AlsaBackend {
    fn drop(&mut self) {
        if let Some((thread, stop)) = self.watcher.take() {
            stop.store(true, Ordering::Release);
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

impl OutputBackend for AlsaBackend {
    fn kind(&self) -> OutputBackendKind {
        KIND
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError> {
        devices::list(true)
    }

    fn open(&self, config: &StreamConfig) -> Result<OpenedStream, OutputError> {
        let name = match &config.device {
            DeviceSelection::SystemDefault => "default",
            DeviceSelection::Device(id) => id.as_str(),
        };
        let card = match &config.device {
            DeviceSelection::SystemDefault => None,
            DeviceSelection::Device(_) => reserve::card_of(name),
        };
        self.open_pcm(config, name, card.as_ref())
    }
}

/// Why opening `name` failed, in the backend's terms.
fn open_error(device: &DeviceSelection, name: &str, error: &alsa_lib::Error) -> OutputError {
    match error.errno() {
        libc_errno::EBUSY => OutputError::DeviceBusy {
            backend: KIND,
            device: device.clone(),
            owner: devices::owner(name),
        },
        libc_errno::ENOENT | libc_errno::ENODEV | libc_errno::ENXIO => {
            OutputError::DeviceNotFound {
                backend: KIND,
                device: device.clone(),
            }
        }
        _ => OutputError::Backend {
            backend: KIND,
            reason: format!("cannot open {name}: {error}"),
        },
    }
}

/// The errno values the backend tells apart.
mod libc_errno {
    pub(super) const ENOENT: i32 = 2;
    pub(super) const ENXIO: i32 = 6;
    pub(super) const EAGAIN: i32 = 11;
    pub(super) const EBUSY: i32 = 16;
    pub(super) const ENODEV: i32 = 19;
    pub(super) const EPIPE: i32 = 32;
    pub(super) const ESTRPIPE: i32 = 86;
    pub(super) const EBADFD: i32 = 77;
}

/// The ALSA format of a sample format.
fn alsa_format(format: SampleFormat) -> Format {
    use Endianness::{Big, Little};
    match format {
        SampleFormat::U8 => Format::U8,
        SampleFormat::S16(Little) => Format::S16LE,
        SampleFormat::S16(Big) => Format::S16BE,
        SampleFormat::S24(Little) => Format::S24LE,
        SampleFormat::S24(Big) => Format::S24BE,
        SampleFormat::S24Packed(Little) => Format::S243LE,
        SampleFormat::S24Packed(Big) => Format::S243BE,
        SampleFormat::S32(Little) => Format::S32LE,
        SampleFormat::S32(Big) => Format::S32BE,
        SampleFormat::F32(Little) => Format::FloatLE,
        SampleFormat::F32(Big) => Format::FloatBE,
        SampleFormat::F64(Little) => Format::Float64LE,
        SampleFormat::F64(Big) => Format::Float64BE,
        SampleFormat::DsdU8 => Format::DSDU8,
        SampleFormat::DsdU16(Little) => Format::DSDU16LE,
        SampleFormat::DsdU16(Big) => Format::DSDU16BE,
        SampleFormat::DsdU32(Little) => Format::DSDU32LE,
        SampleFormat::DsdU32(Big) => Format::DSDU32BE,
    }
}

/// The formats to try, best first: the requested one, then the widest integer
/// formats, then float.
fn format_candidates(requested: SampleFormat) -> Vec<SampleFormat> {
    let native = Endianness::NATIVE;
    let mut candidates = vec![requested];
    for format in [
        SampleFormat::S32(native),
        SampleFormat::S24(native),
        SampleFormat::S24Packed(native),
        SampleFormat::S16(native),
        SampleFormat::F32(native),
        SampleFormat::U8,
    ] {
        if !candidates.contains(&format) {
            candidates.push(format);
        }
    }
    // The writer converts from float, so DSD cannot be written yet.
    candidates.retain(|format| !format.is_dsd());
    candidates
}

/// Sets the hardware and software parameters and reads back what the device
/// accepted.
fn negotiate(pcm: &PCM, config: &StreamConfig) -> Result<Negotiated, OutputError> {
    let refused = |reason: String| OutputError::FormatUnsupported {
        backend: KIND,
        device: config.device.clone(),
        reason,
    };
    let failed = |what: &str, error: alsa_lib::Error| OutputError::Backend {
        backend: KIND,
        reason: format!("{what}: {error}"),
    };
    let requested = config.spec;
    {
        let hw = HwParams::any(pcm).map_err(|e| failed("hardware parameters", e))?;
        if config.mode == OutputMode::Exclusive {
            // No converter inside alsa-lib: the rate must be the device's own.
            hw.set_rate_resample(false)
                .map_err(|e| failed("disable resampling", e))?;
        }
        hw.set_access(Access::RWInterleaved)
            .map_err(|e| refused(format!("interleaved access: {e}")))?;
        let format = format_candidates(requested.format)
            .into_iter()
            .find(|format| hw.test_format(alsa_format(*format)).is_ok())
            .ok_or_else(|| refused("no supported sample format".into()))?;
        hw.set_format(alsa_format(format))
            .map_err(|e| refused(format!("format {format}: {e}")))?;
        hw.set_channels_near(u32::from(requested.channel_count()))
            .map_err(|e| refused(format!("{} channels: {e}", requested.channel_count())))?;
        // "Match the track" and a fixed rate ask for the rate; when the device does
        // not have it, the nearest one it has is taken and the engine resamples.
        // "Device default" takes the device's usual rate.
        let rate = if config.request_rate {
            requested.rate.hz()
        } else {
            DEFAULT_RATES
                .into_iter()
                .find(|rate| hw.test_rate(*rate).is_ok())
                .unwrap_or(requested.rate.hz())
        };
        hw.set_rate_near(rate, ValueOr::Nearest)
            .map_err(|e| refused(format!("{}: {e}", requested.rate)))?;
        hw.set_buffer_time_near(HW_BUFFER_US, ValueOr::Nearest)
            .map_err(|e| failed("buffer size", e))?;
        hw.set_periods_near(PERIODS, ValueOr::Nearest)
            .map_err(|e| failed("period count", e))?;
        pcm.hw_params(&hw).map_err(|e| refused(e.to_string()))?;
    }

    let hw = pcm
        .hw_params_current()
        .map_err(|e| failed("read hardware parameters", e))?;
    let format = format_candidates(requested.format)
        .into_iter()
        .find(|f| hw.get_format().is_ok_and(|g| g == alsa_format(*f)))
        .ok_or_else(|| refused("unexpected sample format".into()))?;
    let channels = hw.get_channels().map_err(|e| failed("channels", e))?;
    let rate = hw.get_rate().map_err(|e| failed("rate", e))?;
    let period = hw.get_period_size().map_err(|e| failed("period size", e))?;
    let buffer = hw.get_buffer_size().map_err(|e| failed("buffer size", e))?;
    let can_pause = hw.can_pause();
    drop(hw);

    let channels = u16::try_from(channels).map_err(|_| refused("too many channels".into()))?;
    let layout = if channels == requested.channel_count() {
        requested.layout
    } else {
        ChannelLayout::default_for_count(channels).ok_or_else(|| refused("no channels".into()))?
    };
    let rate = SampleRate::new(rate).ok_or_else(|| refused("rate 0".into()))?;
    let spec = AudioSpec::new(format, rate, layout);
    let period = usize::try_from(period).map_err(|_| refused("period size".into()))?;
    let buffer = usize::try_from(buffer).map_err(|_| refused("buffer size".into()))?;

    {
        let sw = pcm
            .sw_params_current()
            .map_err(|e| failed("software parameters", e))?;
        // Start when half the hardware buffer is filled; wake the writer for every
        // free period.
        let start = alsa_lib::pcm::Frames::try_from(buffer / 2)
            .unwrap_or(1)
            .max(1);
        let period_frames = alsa_lib::pcm::Frames::try_from(period).unwrap_or(1);
        sw.set_start_threshold(start)
            .and_then(|()| sw.set_avail_min(period_frames))
            .and_then(|()| pcm.sw_params(&sw))
            .map_err(|e| failed("software parameters", e))?;
    }

    let mut encoder = Encoder::new(format, usize::from(channels))
        .ok_or_else(|| refused(format!("{format} cannot be written")))?;
    if let Some(map) = writer::channel_map(pcm, layout) {
        encoder = encoder.with_channel_map(map);
    }
    Ok(Negotiated {
        spec,
        encoder,
        period_frames: period.max(1),
        can_pause,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::reserve::tests::{FakeReservation, card};
    use super::*;

    /// Whether alsa-lib has its `null` PCM, which discards everything. The tests
    /// below open it as if it were a device of a card that has to be taken first.
    fn null_pcm_exists() -> bool {
        let exists = PCM::new("null", Direction::Playback, true).is_ok();
        if !exists {
            eprintln!("skipped: alsa-lib cannot open its null PCM");
        }
        exists
    }

    fn exclusive_config() -> StreamConfig {
        let spec = AudioSpec::new(
            SampleFormat::S16(Endianness::Little),
            SampleRate::new(48_000).unwrap(),
            ChannelLayout::default_for_count(2).unwrap(),
        );
        let mut config = StreamConfig::new(spec);
        config.mode = OutputMode::Exclusive;
        config
    }

    fn backend(fake: &FakeReservation) -> AlsaBackend {
        let (events, _received) = mpsc::channel();
        AlsaBackend::new(events).with_reservation(SharedReservation::new(fake.clone()))
    }

    #[test]
    fn the_stream_keeps_the_card_until_it_is_stopped_or_dropped() {
        if !null_pcm_exists() {
            return;
        }
        let fake = FakeReservation::granting();
        let backend = backend(&fake);
        let config = exclusive_config();

        let opened = backend.open_pcm(&config, "null", Some(&card())).unwrap();
        assert_eq!(fake.entries(), ["acquire 3"]);
        let mut stream = opened.stream;
        stream.stop().unwrap();
        assert_eq!(fake.entries(), ["acquire 3", "release 3"]);
        drop(stream);
        assert_eq!(fake.entries(), ["acquire 3", "release 3"]);

        let opened = backend.open_pcm(&config, "null", Some(&card())).unwrap();
        fake.note("opened");
        drop(opened);
        assert_eq!(
            fake.entries(),
            ["acquire 3", "release 3", "acquire 3", "opened", "release 3"]
        );
    }

    #[test]
    fn a_refused_card_is_busy_and_the_device_is_not_opened() {
        let fake = FakeReservation::refusing(super::super::ReservationError::Refused {
            holder: Some("JACK".into()),
        });
        let error = backend(&fake)
            .open_pcm(&exclusive_config(), "null", Some(&card()))
            .unwrap_err();
        assert!(
            matches!(&error, OutputError::DeviceBusy { owner: Some(o), .. } if o == "JACK"),
            "{error}"
        );
        assert_eq!(fake.entries(), ["acquire 3"]);
    }

    #[test]
    fn requested_format_comes_first_and_dsd_is_dropped() {
        let candidates = format_candidates(SampleFormat::S16(Endianness::Big));
        assert_eq!(candidates[0], SampleFormat::S16(Endianness::Big));
        assert_eq!(candidates.len(), 7);
        let candidates = format_candidates(SampleFormat::DsdU32(Endianness::Big));
        assert!(candidates.iter().all(|f| !f.is_dsd()));
        assert_eq!(candidates[0], SampleFormat::S32(Endianness::NATIVE));
    }

    #[test]
    fn every_sample_format_has_a_real_alsa_format() {
        // alsa-lib aborts the process on Format::Unknown, so it must never appear.
        for format in SampleFormat::ALL {
            assert_ne!(alsa_format(format), Format::Unknown, "{format}");
        }
    }
}
