// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Plays one file to a chosen backend and device, and prints the engine's events.
//!
//! ```sh
//! cargo run -p helikodu-engine --example play -- FILE [--backend NAME] [--device NAME]
//!     [--exclusive] [--start SECONDS] [--volume 0..1]
//! cargo run -p helikodu-engine --example play -- --list-devices [--backend NAME]
//! ```
//!
//! Backends: `auto` (the default), `pipewire`, `alsa`, `null`, or `wav` with the file
//! to write as the device. While it plays, lines on standard input control it:
//! `pause`, `play`, `seek SECONDS`, `volume 0..1`, `output BACKEND [DEVICE]`, `stop`.

#![forbid(unsafe_code)]

use std::error::Error;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use helikodu_engine::output::{automatic_backend, connect_backend};
use helikodu_engine::transport::{Engine, EngineConfig, EngineSender};
use helikodu_model::audio::{
    BackendSelection, DeviceSelection, MediaTime, OutputBackendKind, OutputMode, OutputSelection,
};
use helikodu_model::library::MediaSource;
use helikodu_model::player::{
    EngineCommand, EngineEvent, ItemId, PlaybackItem, TransportState, Volume,
};

#[derive(Debug, Default)]
struct Args {
    file: Option<PathBuf>,
    backend: BackendSelection,
    device: DeviceSelection,
    mode: OutputMode,
    start: Duration,
    volume: Option<Volume>,
    list_devices: bool,
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = Args::default();
    let mut words = std::env::args().skip(1);
    while let Some(word) = words.next() {
        let mut value = || words.next().ok_or(format!("{word} needs a value"));
        match word.as_str() {
            "--backend" => args.backend = value()?.parse()?,
            "--device" => args.device = value()?.parse()?,
            "--exclusive" => args.mode = OutputMode::Exclusive,
            "--start" => args.start = Duration::try_from_secs_f64(value()?.parse()?)?,
            "--volume" => {
                args.volume = Some(Volume::new(value()?.parse()?).ok_or("volume is 0 to 1")?);
            }
            "--list-devices" => args.list_devices = true,
            "-h" | "--help" => {
                println!(
                    "play FILE [--backend NAME] [--device NAME] [--exclusive] \
                     [--start SECONDS] [--volume 0..1]\nplay --list-devices [--backend NAME]"
                );
                std::process::exit(0);
            }
            _ if args.file.is_none() && !word.starts_with("--") => {
                args.file = Some(PathBuf::from(word));
            }
            _ => return Err(format!("unknown argument {word}").into()),
        }
    }
    Ok(args)
}

fn list_devices(backend: BackendSelection) -> Result<(), Box<dyn Error>> {
    let kind = match backend {
        BackendSelection::Automatic => automatic_backend(),
        BackendSelection::Backend(kind) => kind,
    };
    let (events, _) = mpsc::channel();
    let backend = connect_backend(kind, &events)?;
    println!("{kind}:");
    for device in backend.devices()? {
        let default = if device.is_default { " (default)" } else { "" };
        println!("  {}  {}{default}", device.id, device.description);
    }
    Ok(())
}

/// Reads commands from standard input until it closes.
fn read_commands(engine: EngineSender, mode: OutputMode) {
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            return;
        };
        let words: Vec<&str> = line.split_whitespace().collect();
        let command = match words.as_slice() {
            ["pause"] => Some(EngineCommand::Pause),
            ["play"] => Some(EngineCommand::Play),
            ["stop"] => Some(EngineCommand::Stop),
            ["seek", seconds] => seconds
                .parse::<f64>()
                .ok()
                .and_then(|s| Duration::try_from_secs_f64(s).ok())
                .map(|d| EngineCommand::Seek(MediaTime::from_duration(d))),
            ["volume", position] => position
                .parse()
                .ok()
                .and_then(Volume::new)
                .map(EngineCommand::SetVolume),
            ["output", backend, device @ ..] => {
                let backend = backend.parse().ok();
                let device = match device {
                    [] => Some(DeviceSelection::SystemDefault),
                    [name] => name.parse().ok(),
                    _ => None,
                };
                backend.zip(device).map(|(backend, device)| {
                    EngineCommand::SetOutput(OutputSelection {
                        backend,
                        device,
                        mode,
                    })
                })
            }
            _ => None,
        };
        match command {
            Some(command) => {
                if engine.send(command).is_err() {
                    return;
                }
            }
            None => eprintln!("commands: pause, play, seek S, volume V, output B [D], stop"),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    if args.list_devices {
        return list_devices(args.backend);
    }
    let file = args.file.ok_or("no file given; see --help")?;
    if args.backend == BackendSelection::Backend(OutputBackendKind::Wav)
        && args.device == DeviceSelection::SystemDefault
    {
        return Err("the wav backend needs --device FILE".into());
    }

    let mut config = EngineConfig::default();
    config.output.backend = args.backend;
    config.output.device = args.device;
    config.output.mode = args.mode;
    if let Some(volume) = args.volume {
        config.volume = volume;
    }
    let (engine, events) = Engine::spawn(config)?;
    let sender = engine.sender();
    let mode = args.mode;
    std::thread::spawn(move || read_commands(sender, mode));

    let item = PlaybackItem::new(ItemId::new(1), MediaSource::file(&file));
    engine.send(EngineCommand::Load {
        item,
        start: MediaTime::from_duration(args.start),
        paused: false,
    })?;
    let mut loaded = false;
    for event in events.iter() {
        match &event {
            EngineEvent::Position { position, .. } => {
                println!("position {:.3} s", position.as_secs_f64());
            }
            EngineEvent::DevicesChanged { backend, devices } => {
                println!("devices of {backend}: {}", devices.len());
            }
            other => println!("{other:?}"),
        }
        match event {
            EngineEvent::NearEnd { .. } => engine.send(EngineCommand::SetNext(None))?,
            EngineEvent::StateChanged(TransportState::Loading) => loaded = true,
            EngineEvent::StateChanged(TransportState::Stopped) if loaded => break,
            _ => {}
        }
    }
    Ok(())
}
