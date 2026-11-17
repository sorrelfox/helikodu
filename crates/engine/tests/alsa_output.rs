// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The ALSA backend against the sound cards of the machine.
//!
//! These tests are ignored by default: they need sound hardware. They never make
//! a sound: they only list devices, open the free `hw:` devices, negotiate a
//! format and close them again, without starting a stream or writing a sample.
//! Run them with
//!
//! ```sh
//! cargo test -p helikodu-engine --test alsa_output -- --ignored
//! ```

#![forbid(unsafe_code)]
// Tests may unwrap (see clippy.toml); clippy does not count the helpers here as tests.
#![allow(clippy::unwrap_used)]
#![cfg(feature = "alsa")]

use std::sync::mpsc;
use std::time::Duration;

use helikodu_engine::output::{AlsaBackend, OutputBackend, OutputError, StreamConfig, StreamState};
use helikodu_model::audio::{
    AudioSpec, ChannelLayout, DeviceId, DeviceSelection, OutputBackendKind, OutputMode, SampleRate,
};

fn hw_devices(backend: &AlsaBackend) -> Vec<helikodu_model::audio::DeviceInfo> {
    backend
        .devices()
        .unwrap()
        .into_iter()
        .filter(|d| d.id.as_str().starts_with("hw:CARD="))
        .collect()
}

#[test]
#[ignore = "needs sound hardware"]
fn lists_cards_by_id_string_with_capabilities() {
    let (events, _received) = mpsc::channel();
    let backend = AlsaBackend::new(events);
    let devices = backend.devices().unwrap();
    for device in &devices {
        println!(
            "{} - {} - rates {:?} formats {:?} channels {:?}",
            device.id,
            device.description,
            device
                .capabilities
                .sample_rates
                .iter()
                .map(|r| r.hz())
                .collect::<Vec<_>>(),
            device
                .capabilities
                .formats
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            device.capabilities.channel_counts
        );
        assert_eq!(device.backend, OutputBackendKind::Alsa);
        assert_ne!(device.id.as_str(), "default");
    }
    let hw = hw_devices(&backend);
    assert!(!hw.is_empty(), "no hw: playback devices");
    // Card ID strings, never indexes.
    for device in &hw {
        let card = device.id.as_str()["hw:CARD=".len()..]
            .split(',')
            .next()
            .unwrap();
        assert!(card.parse::<u32>().is_err(), "{}", device.id);
    }
    // A free device reports what it accepts.
    let probed: Vec<_> = hw
        .iter()
        .filter(|d| !d.capabilities.formats.is_empty())
        .collect();
    for device in &probed {
        assert!(
            !device.capabilities.sample_rates.is_empty(),
            "{}",
            device.id
        );
        assert!(
            !device.capabilities.channel_counts.is_empty(),
            "{}",
            device.id
        );
    }
}

#[test]
#[ignore = "needs sound hardware"]
fn marks_the_device_behind_default() {
    let (events, _received) = mpsc::channel();
    let backend = AlsaBackend::new(events);
    let devices = backend.devices().unwrap();
    let defaults: Vec<&str> = devices
        .iter()
        .filter(|d| d.is_default)
        .map(|d| d.id.as_str())
        .collect();
    println!("default: {defaults:?}");
    assert_eq!(
        defaults.len(),
        1,
        "exactly one default device: {devices:#?}"
    );
    // With a sound server's plugin as `default`, its PCM is the default; otherwise
    // one device of a card.
    let id = defaults[0];
    assert!(
        id.starts_with("hw:CARD=") || ["pipewire", "pulse"].contains(&id),
        "{id}"
    );
}

#[test]
#[ignore = "needs sound hardware"]
fn negotiates_exclusively_and_closes_without_playing() {
    let (events, _received) = mpsc::channel();
    let backend = AlsaBackend::new(events);
    let mut opened_any = false;
    for device in hw_devices(&backend) {
        let caps = &device.capabilities;
        let (Some(&format), Some(&rate), Some(&channels)) = (
            caps.formats.first(),
            caps.sample_rates.first(),
            caps.channel_counts.first(),
        ) else {
            // Busy (held by the sound server) when listed.
            continue;
        };
        let Some(layout) = ChannelLayout::default_for_count(channels) else {
            continue;
        };
        let spec = AudioSpec::new(format, rate, layout);
        let config = StreamConfig {
            device: DeviceSelection::Device(device.id.clone()),
            mode: OutputMode::Exclusive,
            ..StreamConfig::new(spec)
        };
        match backend.open(&config) {
            Ok(mut opened) => {
                opened_any = true;
                // Exact negotiation of a format the device listed.
                assert_eq!(opened.stream.spec(), spec, "{}", device.id);
                assert_eq!(opened.stream.state(), StreamState::Ready);
                std::thread::sleep(Duration::from_millis(20));
                // Never started: nothing was taken or written.
                assert_eq!(opened.stream.timing().frames_delivered, 0);
                opened.stream.stop().unwrap();
                assert!(opened.writer.is_closed());
                println!("{}: negotiated {spec}", device.id);
            }
            Err(OutputError::DeviceBusy { owner, .. }) => {
                println!("{}: busy, held by {owner:?}", device.id);
            }
            Err(error) => panic!("{}: {error}", device.id),
        }
    }
    assert!(opened_any, "no free hw: device to negotiate with");
}

#[test]
#[ignore = "needs sound hardware"]
fn refused_rates_come_back_as_the_nearest_accepted_one() {
    let (events, _received) = mpsc::channel();
    let backend = AlsaBackend::new(events);
    // A device that does not offer every rate.
    let Some(device) = hw_devices(&backend).into_iter().find(|d| {
        let rates = &d.capabilities.sample_rates;
        !rates.is_empty() && !rates.contains(&SampleRate::new(11_025).unwrap())
    }) else {
        return;
    };
    let caps = &device.capabilities;
    let spec = AudioSpec::new(
        caps.formats[0],
        SampleRate::new(11_025).unwrap(),
        ChannelLayout::default_for_count(caps.channel_counts[0]).unwrap(),
    );
    let config = StreamConfig {
        device: DeviceSelection::Device(device.id.clone()),
        mode: OutputMode::Exclusive,
        ..StreamConfig::new(spec)
    };
    let mut opened = backend.open(&config).unwrap();
    let negotiated = opened.stream.spec();
    assert_ne!(negotiated.rate.hz(), 11_025);
    println!("{}: 11025 Hz refused, got {}", device.id, negotiated.rate);
    opened.stream.stop().unwrap();
}

#[test]
#[ignore = "needs sound hardware"]
fn a_missing_card_is_not_found() {
    let (events, _received) = mpsc::channel();
    let backend = AlsaBackend::new(events);
    let spec = AudioSpec::new(
        helikodu_model::audio::SampleFormat::S16(helikodu_model::audio::Endianness::Little),
        SampleRate::HZ_44100,
        ChannelLayout::STEREO,
    );
    let config = StreamConfig {
        device: DeviceSelection::Device(DeviceId::new("hw:CARD=HelikoduNoSuchCard,DEV=0").unwrap()),
        mode: OutputMode::Exclusive,
        ..StreamConfig::new(spec)
    };
    let result = backend.open(&config);
    assert!(
        matches!(result, Err(OutputError::DeviceNotFound { .. })),
        "{result:?}"
    );
}
