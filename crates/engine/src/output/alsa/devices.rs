// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! ALSA device enumeration, capabilities, owners of busy devices, and the card
//! watcher.

use std::collections::BTreeMap;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use ::alsa as alsa_lib;
use alsa_lib::Direction;
use alsa_lib::ctl::{Ctl, DeviceIter};
use alsa_lib::device_name::HintIter;
use alsa_lib::pcm::{HwParams, PCM};
use helikodu_model::audio::{DeviceCapabilities, DeviceId, DeviceInfo, SampleFormat, SampleRate};

use super::super::{EventSender, OutputError, OutputEvent};
use super::{KIND, alsa_format, libc_errno};

/// How often the card watcher looks at the list of cards.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);
/// Where the kernel lists the sound cards.
const CARDS_FILE: &str = "/proc/asound/cards";
/// The rates a device is asked about.
const RATES: [u32; 15] = [
    8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176_400, 192_000, 352_800,
    384_000, 705_600, 768_000,
];
/// At most this many channel counts are listed for one device.
const MAX_CHANNEL_COUNTS: u32 = 32;

/// The playback devices of every card, then the PCMs of the ALSA configuration
/// that belong to no card. With `probe`, each free `hw:` device is opened to read
/// what it accepts; a busy one is listed without capabilities.
pub(super) fn list(probe: bool) -> Result<Vec<DeviceInfo>, OutputError> {
    let mut devices = Vec::new();
    for card in alsa_lib::card::Iter::new() {
        let Ok(card) = card else { continue };
        let Ok(ctl) = Ctl::from_card(&card, false) else {
            continue;
        };
        let Ok(info) = ctl.card_info() else { continue };
        let (Ok(card_id), Ok(card_name)) = (info.get_id(), info.get_name()) else {
            continue;
        };
        for device in DeviceIter::new(&ctl) {
            let Ok(index) = u32::try_from(device) else {
                continue;
            };
            let Ok(pcm) = ctl.pcm_info(index, 0, Direction::Playback) else {
                continue;
            };
            let name = format!("hw:CARD={card_id},DEV={index}");
            let Ok(id) = DeviceId::new(name.as_str()) else {
                continue;
            };
            let pcm_name = pcm.get_name().unwrap_or_default();
            let description = if pcm_name.is_empty() {
                card_name.to_owned()
            } else {
                format!("{card_name}, {pcm_name}")
            };
            devices.push(DeviceInfo {
                backend: KIND,
                id,
                description,
                is_default: false,
                capabilities: if probe {
                    capabilities(&name)
                } else {
                    DeviceCapabilities::default()
                },
            });
        }
    }
    devices.extend(configured_pcms());
    if let Some(default) = pick_default(&default_pcm(), &devices) {
        for device in &mut devices {
            device.is_default = device.id == default;
        }
    }
    Ok(devices)
}

/// What the `default` PCM ("System default") plays to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DefaultPcm {
    /// A device of a card: directly, or through the plug, dmix or another layer.
    Card { id: String, device: u32 },
    /// A PCM that belongs to no card, such as a sound server's plugin. The text is
    /// alsa-lib's description of the PCM (its dump), for example
    /// `ALSA <-> PipeWire PCM I/O Plugin`.
    Plugin(String),
    /// The `default` PCM could not be opened and does not tell.
    Unknown,
}

/// Opens the `default` PCM, without starting it or writing a sample, and finds out
/// what it plays to. A `default` that is busy is a card held by another program:
/// then it is the first card, alsa-lib's default card.
fn default_pcm() -> DefaultPcm {
    match PCM::new("default", Direction::Playback, true) {
        Ok(pcm) => {
            if let Ok(info) = pcm.info()
                && let Some(id) = card_id(info.get_card())
            {
                return DefaultPcm::Card {
                    id,
                    device: info.get_device(),
                };
            }
            let Ok(mut out) = alsa_lib::Output::buffer_open() else {
                return DefaultPcm::Unknown;
            };
            if pcm.dump(&mut out).is_err() {
                return DefaultPcm::Unknown;
            }
            DefaultPcm::Plugin(out.buffer_string(|text| String::from_utf8_lossy(text).into_owned()))
        }
        Err(error) if error.errno() == libc_errno::EBUSY => alsa_lib::card::Iter::new()
            .flatten()
            .next()
            .and_then(|card| card_id(card.get_index()))
            .map_or(DefaultPcm::Unknown, |id| DefaultPcm::Card { id, device: 0 }),
        Err(_) => DefaultPcm::Unknown,
    }
}

/// The ID string of the card with index `index`.
fn card_id(index: i32) -> Option<String> {
    if index < 0 {
        return None;
    }
    let ctl = Ctl::from_card(&alsa_lib::card::Card::new(index), false).ok()?;
    let info = ctl.card_info().ok()?;
    info.get_id().ok().map(str::to_owned)
}

/// The listed device that `default` plays to: the card's device, or the PCM of the
/// sound server whose plugin `default` is (`pipewire`, `pulse`, `jack`).
fn pick_default(default: &DefaultPcm, devices: &[DeviceInfo]) -> Option<DeviceId> {
    let wanted = match default {
        DefaultPcm::Card { id, device } => format!("hw:CARD={id},DEV={device}"),
        DefaultPcm::Plugin(text) => {
            let text = text.to_ascii_lowercase();
            let server = [
                ("pipewire", "pipewire"),
                ("pulse", "pulse"),
                ("jack", "jack"),
            ]
            .into_iter()
            .find(|(mark, _)| text.contains(mark))?;
            server.1.to_owned()
        }
        DefaultPcm::Unknown => return None,
    };
    devices
        .iter()
        .find(|device| device.id.as_str() == wanted)
        .map(|device| device.id.clone())
}

/// PCMs from the ALSA configuration that belong to no card, such as `pipewire`.
/// `default` (and `sysdefault`, the first card's default) is "System default" and
/// `null` discards audio; none of them is listed.
fn configured_pcms() -> Vec<DeviceInfo> {
    let Ok(hints) = HintIter::new_str(None, "pcm") else {
        return Vec::new();
    };
    hints
        .filter(|hint| !matches!(hint.direction, Some(Direction::Capture)))
        .filter_map(|hint| {
            let name = hint.name?;
            if name.contains(':') || ["default", "sysdefault", "null"].contains(&name.as_str()) {
                return None;
            }
            let description = hint
                .desc
                .as_deref()
                .and_then(|d| d.lines().next())
                .unwrap_or(&name)
                .to_owned();
            Some(DeviceInfo {
                backend: KIND,
                id: DeviceId::new(name).ok()?,
                description,
                is_default: false,
                capabilities: DeviceCapabilities::default(),
            })
        })
        .collect()
}

/// What a device accepts. Opens it non-blocking; a busy or missing device gives
/// empty lists.
pub(super) fn capabilities(name: &str) -> DeviceCapabilities {
    let Ok(pcm) = PCM::new(name, Direction::Playback, true) else {
        return DeviceCapabilities::default();
    };
    let Ok(hw) = HwParams::any(&pcm) else {
        return DeviceCapabilities::default();
    };
    // Only real formats are tested: alsa-lib aborts on Format::Unknown.
    let formats = SampleFormat::ALL
        .into_iter()
        .filter(|format| hw.test_format(alsa_format(*format)).is_ok())
        .collect();
    let sample_rates = RATES
        .into_iter()
        .filter(|rate| hw.test_rate(*rate).is_ok())
        .filter_map(SampleRate::new)
        .collect();
    let channel_counts = match (hw.get_channels_min(), hw.get_channels_max()) {
        (Ok(min), Ok(max)) => (min.max(1)..=max.min(min.saturating_add(MAX_CHANNEL_COUNTS - 1)))
            .filter(|n| hw.test_channels(*n).is_ok())
            .filter_map(|n| u16::try_from(n).ok())
            .collect(),
        _ => Vec::new(),
    };
    DeviceCapabilities {
        sample_rates,
        formats,
        channel_counts,
    }
}

/// The program that holds a busy `hw:` or `plughw:` device, from the kernel's
/// status of its first playback sub-device, for example `pipewire (pid 4121)`.
pub(super) fn owner(name: &str) -> Option<String> {
    let (card, device) = parse_hw_name(name)?;
    let card = card_index(&card)?;
    let status =
        fs::read_to_string(format!("/proc/asound/card{card}/pcm{device}p/sub0/status")).ok()?;
    let pid = owner_pid(&status)?;
    let program = fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    let program = program.trim();
    Some(if program.is_empty() {
        format!("pid {pid}")
    } else {
        format!("{program} (pid {pid})")
    })
}

/// The card (ID string or index) and device number of `hw:CARD=x,DEV=n`,
/// `plughw:CARD=x,DEV=n`, `hw:x,n` or `hw:x`.
pub(super) fn parse_hw_name(name: &str) -> Option<(String, u32)> {
    let args = name
        .strip_prefix("hw:")
        .or_else(|| name.strip_prefix("plughw:"))?;
    let mut card = None;
    let mut device = 0;
    for (i, part) in args.split(',').enumerate() {
        let (key, value) = match part.split_once('=') {
            Some((key, value)) => (key, value),
            None if i == 0 => ("CARD", part),
            None => ("DEV", part),
        };
        match key {
            "CARD" => card = Some(value.to_owned()),
            "DEV" => device = value.parse().ok()?,
            _ => {}
        }
    }
    Some((card?, device))
}

/// The index of the card with this ID string (or index).
pub(super) fn card_index(card: &str) -> Option<i32> {
    if let Ok(index) = card.parse() {
        return Some(index);
    }
    alsa_lib::card::Iter::new().flatten().find_map(|c| {
        let ctl = Ctl::from_card(&c, false).ok()?;
        let info = ctl.card_info().ok()?;
        (info.get_id().ok()? == card).then(|| c.get_index())
    })
}

fn owner_pid(status: &str) -> Option<u32> {
    status.lines().find_map(|line| {
        let value = line.trim().strip_prefix("owner_pid")?;
        value
            .trim_start_matches([' ', '\t', ':'])
            .trim()
            .parse()
            .ok()
    })
}

/// Reports cards that appear or disappear until `stop` is set. Polls the
/// kernel's card list; a udev monitor would avoid the periodic wake-up.
pub(super) fn watch_cards(events: &EventSender, stop: &AtomicBool) {
    let ids = |devices: Vec<DeviceInfo>| -> BTreeMap<DeviceId, DeviceInfo> {
        devices.into_iter().map(|d| (d.id.clone(), d)).collect()
    };
    let mut cards = fs::read_to_string(CARDS_FILE).unwrap_or_default();
    let mut known = ids(list(false).unwrap_or_default());
    loop {
        thread::park_timeout(WATCH_INTERVAL);
        if stop.load(Ordering::Acquire) {
            return;
        }
        let now = fs::read_to_string(CARDS_FILE).unwrap_or_default();
        if now == cards {
            continue;
        }
        cards = now;
        let current = ids(list(false).unwrap_or_default());
        for id in known.keys().filter(|id| !current.contains_key(*id)) {
            let _ = events.send(OutputEvent::DeviceRemoved {
                backend: KIND,
                id: id.clone(),
            });
        }
        for (id, device) in current.iter().filter(|(id, _)| !known.contains_key(*id)) {
            let _ = events.send(OutputEvent::DeviceAdded(DeviceInfo {
                capabilities: capabilities(id.as_str()),
                ..device.clone()
            }));
        }
        known = current;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hw_names() {
        assert_eq!(
            parse_hw_name("hw:CARD=D10s,DEV=1"),
            Some(("D10s".into(), 1))
        );
        assert_eq!(parse_hw_name("plughw:CARD=PCH"), Some(("PCH".into(), 0)));
        assert_eq!(parse_hw_name("hw:2,3"), Some(("2".into(), 3)));
        assert_eq!(parse_hw_name("default"), None);
        assert_eq!(parse_hw_name("hw:CARD=x,DEV=y"), None);
    }

    #[test]
    fn reads_the_owner_pid() {
        let status = "state: RUNNING\nowner_pid   : 4121\ntrigger_time: 1.0\n";
        assert_eq!(owner_pid(status), Some(4121));
        assert_eq!(owner_pid("closed\n"), None);
    }

    fn device(name: &str) -> DeviceInfo {
        DeviceInfo {
            backend: KIND,
            id: DeviceId::new(name).unwrap(),
            description: name.to_owned(),
            is_default: false,
            capabilities: DeviceCapabilities::default(),
        }
    }

    #[test]
    fn the_default_is_the_card_or_the_sound_server_behind_default() {
        let devices = [
            device("hw:CARD=PCH,DEV=0"),
            device("hw:CARD=PCH,DEV=3"),
            device("hw:CARD=D10s,DEV=0"),
            device("pipewire"),
            device("pulse"),
        ];
        let pick = |default: DefaultPcm| pick_default(&default, &devices).map(|id| id.to_string());
        let card = |id: &str, device| DefaultPcm::Card {
            id: id.into(),
            device,
        };
        assert_eq!(pick(card("PCH", 3)).as_deref(), Some("hw:CARD=PCH,DEV=3"));
        assert_eq!(pick(card("D10s", 0)).as_deref(), Some("hw:CARD=D10s,DEV=0"));
        // A card that is not listed (no playback device of that number).
        assert_eq!(pick(card("PCH", 7)), None);
        let plugin = |text: &str| DefaultPcm::Plugin(text.into());
        assert_eq!(
            pick(plugin("ALSA <-> PipeWire PCM I/O Plugin\nIts setup is:\n")).as_deref(),
            Some("pipewire")
        );
        assert_eq!(
            pick(plugin("ALSA <-> PulseAudio PCM I/O Plugin\n")).as_deref(),
            Some("pulse")
        );
        // JACK is not listed here.
        assert_eq!(pick(plugin("ALSA <-> JACK PCM I/O Plugin\n")), None);
        assert_eq!(pick(plugin("Null PCM\n")), None);
        assert_eq!(pick(DefaultPcm::Unknown), None);
    }
}
