// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! cpal 0.18 probe for the PulseAudio, JACK and PipeWire hosts.
//!
//! ```text
//! cpal_probe hosts
//! cpal_probe run --host pulseaudio|pipewire|jack [--device SUBSTRING] [--rate HZ] [--secs SECS]
//!                [--buffer FRAMES]
//! ```
//!
//! Plays digital silence only. For JACK, run with
//! `PIPEWIRE_PROPS='{ jack.self-connect-mode = ignore-all }'` so that cpal's
//! automatic connection to the system ports (the default device) is ignored;
//! the probe then connects its ports to the null sink itself.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, HostId, StreamConfig};
use spike_output::{CallbackStats, RenderConfig, pull, run, spawn_render};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "hosts".into());
    let mut host = "pulseaudio".to_string();
    let mut device = "helikodu_spike".to_string();
    let mut rate = 48000;
    let mut secs = 3.0;
    let mut buffer: Option<u32> = None;
    while let Some(k) = args.next() {
        let mut val = || args.next().ok_or_else(|| anyhow!("{k} needs a value"));
        match k.as_str() {
            "--host" => host = val()?,
            "--device" => device = val()?,
            "--rate" => rate = val()?.parse()?,
            "--secs" => secs = val()?.parse()?,
            "--buffer" => buffer = Some(val()?.parse()?),
            other => return Err(anyhow!("unknown argument {other}")),
        }
    }
    match cmd.as_str() {
        "hosts" => {
            println!("compiled hosts: {:?}", cpal::ALL_HOSTS);
            println!("available hosts: {:?}", cpal::available_hosts());
            Ok(())
        }
        "run" => run_host(&host, &device, rate, secs, buffer),
        other => Err(anyhow!("unknown command {other}")),
    }
}

fn run_host(
    host_name: &str,
    wanted: &str,
    rate: u32,
    secs: f64,
    buffer: Option<u32>,
) -> Result<()> {
    let id = match host_name {
        "pulseaudio" => HostId::PulseAudio,
        "pipewire" => HostId::PipeWire,
        "jack" => {
            let props = std::env::var("PIPEWIRE_PROPS").unwrap_or_default();
            if !props.contains("ignore-all") {
                return Err(anyhow!(
                    "refusing to run the JACK host without PIPEWIRE_PROPS='{{ jack.self-connect-mode = ignore-all }}': cpal would connect to the default device"
                ));
            }
            HostId::Jack
        }
        other => return Err(anyhow!("unknown host {other}")),
    };
    let t = Instant::now();
    let host = cpal::host_from_id(id)?;
    println!("[{host_name}] host created in {:?}", t.elapsed());

    let t = Instant::now();
    let devices: Vec<_> = host.output_devices()?.collect();
    println!(
        "[{host_name}] {} output devices enumerated in {:?}",
        devices.len(),
        t.elapsed()
    );
    for d in &devices {
        let id = d
            .id()
            .map(|i| i.to_string())
            .unwrap_or_else(|e| format!("<{e}>"));
        let desc = d.description().map(|d| d.to_string()).unwrap_or_default();
        println!("[{host_name}]   id={id}  desc={desc}");
    }
    let device = if id == HostId::Jack {
        host.default_output_device()
    } else {
        devices.into_iter().find(|d| {
            d.id()
                .map(|i| i.to_string().contains(wanted))
                .unwrap_or(false)
        })
    }
    .ok_or_else(|| anyhow!("no output device matching {wanted}"))?;
    println!(
        "[{host_name}] using {}; default config {:?}",
        device.id()?,
        device.default_output_config().ok()
    );
    if let Ok(cfgs) = device.supported_output_configs() {
        for c in cfgs.take(6) {
            println!("[{host_name}]   supports {c:?}");
        }
    }

    let channels = 2usize;
    let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(rate as usize / 5 * channels);
    let stop = Arc::new(AtomicBool::new(false));
    let render = spawn_render(producer, RenderConfig::default(), stop.clone());
    std::thread::sleep(Duration::from_millis(20));

    let stats = CallbackStats::new();
    let cb_stats = stats.clone();
    let config = StreamConfig {
        channels: channels as u16,
        sample_rate: rate,
        buffer_size: buffer.map(BufferSize::Fixed).unwrap_or(BufferSize::Default),
    };
    let t = Instant::now();
    let stream = device.build_output_stream::<f32, _, _>(
        config,
        move |out: &mut [f32], _info| {
            let start = cb_stats.begin((out.len() / channels) as u32);
            let missing = pull(&mut consumer, out);
            cb_stats.end(
                start,
                ((out.len() - missing) / channels) as u64,
                (missing / channels) as u64,
            );
        },
        |e| eprintln!("stream error: {e}"),
        None,
    );
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            println!(
                "[{host_name}] build_output_stream failed after {:?}: {e}",
                t.elapsed()
            );
            stop.store(true, Ordering::Relaxed);
            let _ = render.join();
            return Ok(());
        }
    };
    println!("[{host_name}] stream built in {:?}", t.elapsed());

    if id == HostId::Jack {
        connect_jack_to_null_sink()?;
    }
    let t = Instant::now();
    stream.play()?;
    println!("[{host_name}] play() returned in {:?}", t.elapsed());
    std::thread::sleep(Duration::from_secs_f64(secs * 0.7));

    // Where did the stream go, and at which rate does the graph run?
    if id != HostId::Jack {
        for line in run("pactl", &["list", "sink-inputs"]).lines() {
            let l = line.trim();
            if l.starts_with("Sink Input #")
                || l.starts_with("Sink:")
                || l.starts_with("Sample Specification")
                || l.starts_with("Volume:")
                || l.starts_with("application.name")
                || l.starts_with("node.name")
                || l.starts_with("target.object")
                || l.starts_with("node.rate")
                || l.starts_with("node.latency")
            {
                println!("[{host_name}] pactl: {l}");
            }
        }
    } else {
        // Print each of our output ports with every link under it.
        let links = run("pw-link", &["-l"]);
        let mut in_ours = false;
        for line in links.lines() {
            if !line.starts_with(' ') {
                in_ours = line.starts_with("cpal_client_");
            }
            if in_ours {
                println!("[{host_name}] pw-link: {}", line.trim());
            }
        }
    }
    std::thread::sleep(Duration::from_secs_f64(secs * 0.3));
    drop(stream);
    stop.store(true, Ordering::Relaxed);
    let _ = render.join();
    println!("{}", stats.report(host_name, rate));
    Ok(())
}

/// Connects the cpal JACK client's output ports to the null sink's ports
/// through a second JACK client (cpal does not expose its own client).
fn connect_jack_to_null_sink() -> Result<()> {
    let (client, _status) =
        jack::Client::new("helikodu-spike-patch", jack::ClientOptions::NO_START_SERVER)
            .map_err(|e| anyhow!("jack client: {e}"))?;
    println!(
        "[jack] server: rate {} Hz, buffer {} frames",
        client.sample_rate(),
        client.buffer_size()
    );
    let ours = client.ports(
        Some("cpal_client_.*:out_.*"),
        None,
        jack::PortFlags::IS_OUTPUT,
    );
    let sink = client.ports(
        Some("Helikodu-spike-null-sink:playback_.*"),
        None,
        jack::PortFlags::IS_INPUT,
    );
    println!("[jack] our ports {ours:?}; null sink ports {sink:?}");
    for (o, i) in ours.iter().zip(sink.iter()) {
        match client.connect_ports_by_name(o, i) {
            Ok(()) => println!("[jack] connected {o} -> {i}"),
            Err(e) => println!("[jack] connect {o} -> {i} failed: {e}"),
        }
    }
    // Keep the patch client alive only for the connection; links persist.
    drop(client);
    Ok(())
}
