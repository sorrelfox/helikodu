// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! ALSA output probe, built on the `alsa` crate.
//!
//! ```text
//! alsa_probe list                       cards, PCM devices, named PCMs, and the
//!                                       capabilities of every free hw: device
//! alsa_probe caps --device NAME         capabilities of one device
//! alsa_probe open --device NAME [--retry-ms MS]
//!                                       try to open; with --retry-ms keep trying
//! alsa_probe negotiate --device NAME [--rate HZ] [--format S16_LE|S24_LE|S24_3LE|S32_LE]
//!                      [--channels N] [--silence-ms MS]
//! ```
//!
//! Only `negotiate --silence-ms` writes to the device, and only zeros
//! (digital silence), for at most 250 ms.

use std::time::{Duration, Instant};

use alsa::card;
use alsa::ctl::{Ctl, DeviceIter};
use alsa::device_name::HintIter;
use alsa::pcm::{Access, Format, HwParams, PCM, State};
use alsa::{Direction, ValueOr};
use anyhow::{Result, anyhow};

const RATES: [u32; 12] = [
    8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000, 384000,
];

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "list".into());
    let mut device = None;
    let mut rate = 48000;
    let mut format = "S32_LE".to_string();
    let mut channels = 2;
    let mut silence_ms = 0u64;
    let mut retry_ms = 0u64;
    while let Some(k) = args.next() {
        let mut val = || args.next().ok_or_else(|| anyhow!("{k} needs a value"));
        match k.as_str() {
            "--device" => device = Some(val()?),
            "--rate" => rate = val()?.parse()?,
            "--format" => format = val()?,
            "--channels" => channels = val()?.parse()?,
            "--silence-ms" => silence_ms = val()?.parse()?,
            "--retry-ms" => retry_ms = val()?.parse()?,
            other => return Err(anyhow!("unknown argument {other}")),
        }
    }
    match cmd.as_str() {
        "list" => list(),
        "caps" => caps(&device.ok_or_else(|| anyhow!("--device"))?),
        "open" => open(&device.ok_or_else(|| anyhow!("--device"))?, retry_ms),
        "negotiate" => negotiate(
            &device.ok_or_else(|| anyhow!("--device"))?,
            rate,
            &format,
            channels,
            silence_ms.min(250),
        ),
        other => Err(anyhow!("unknown command {other}")),
    }
}

/// Who holds a PCM, from /proc/asound (`owner_pid` in the sub-stream status).
fn owner_of(card: i32, dev: i32) -> String {
    let path = format!("/proc/asound/card{card}/pcm{dev}p/sub0/status");
    let status = std::fs::read_to_string(&path).unwrap_or_default();
    if status.trim() == "closed" {
        return "closed".into();
    }
    let pid = status.lines().find_map(|l| {
        l.trim()
            .strip_prefix("owner_pid")
            .map(|v| v.trim_start_matches([' ', ':']).trim().to_string())
    });
    let state = status
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("state:")
                .map(|v| v.trim().to_string())
        })
        .unwrap_or_default();
    match pid {
        Some(pid) => {
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
            format!("state={state} owner_pid={pid} ({})", comm.trim())
        }
        None => format!("status: {}", status.lines().next().unwrap_or("?")),
    }
}

fn list() -> Result<()> {
    let t = Instant::now();
    let mut hw_devices = Vec::new();
    for c in card::Iter::new() {
        let c = c?;
        let ctl = Ctl::from_card(&c, false)?;
        let info = ctl.card_info()?;
        println!(
            "card {} id={} driver={} name={:?} longname={:?}",
            c.get_index(),
            info.get_id()?,
            info.get_driver()?,
            info.get_name()?,
            info.get_longname()?
        );
        for dev in DeviceIter::new(&ctl) {
            let Ok(pi) = ctl.pcm_info(dev as u32, 0, Direction::Playback) else {
                continue;
            };
            let name = format!("hw:CARD={},DEV={dev}", info.get_id()?);
            println!(
                "  playback dev {dev}: {name} id={:?} name={:?} subdevices {}/{} available; {}",
                pi.get_id()?,
                pi.get_name()?,
                pi.get_subdevices_avail(),
                pi.get_subdevices_count(),
                owner_of(c.get_index(), dev)
            );
            hw_devices.push(name);
        }
    }
    println!("enumeration took {:?}", t.elapsed());

    println!("named PCMs (hints):");
    for h in HintIter::new_str(None, "pcm")? {
        if matches!(h.direction, Some(Direction::Capture)) {
            continue;
        }
        let name = h.name.unwrap_or_default();
        let desc = h.desc.unwrap_or_default().replace('\n', " / ");
        println!("  {name}  -- {desc}");
    }

    for d in hw_devices {
        caps(&d)?;
    }
    Ok(())
}

fn fmt_err(e: &alsa::Error) -> String {
    format!("{} (errno {})", e, e.errno())
}

/// Opens a device non-blocking and prints what its hardware parameters allow.
fn caps(device: &str) -> Result<()> {
    let t = Instant::now();
    let pcm = match PCM::new(device, Direction::Playback, true) {
        Ok(p) => p,
        Err(e) => {
            println!(
                "{device}: open failed after {:?}: {}",
                t.elapsed(),
                fmt_err(&e)
            );
            return Ok(());
        }
    };
    let opened = t.elapsed();
    let hw = HwParams::any(&pcm)?;
    let formats: Vec<String> = Format::all()
        .iter()
        // Format::all() contains Unknown (-1). Passing it to alsa-lib trips an
        // assertion in snd_mask_leave and aborts the process.
        .filter(|f| (**f as i32) >= 0 && (**f as i32) <= alsa::pcm::Format::DSDU32BE as i32)
        .filter(|f| hw.test_format(**f).is_ok())
        .map(|f| f.to_string())
        .collect();
    let rates: Vec<String> = RATES
        .iter()
        .filter(|r| hw.test_rate(**r).is_ok())
        .map(|r| r.to_string())
        .collect();
    let access: Vec<&str> = [
        (Access::RWInterleaved, "RW_INTERLEAVED"),
        (Access::MMapInterleaved, "MMAP_INTERLEAVED"),
        (Access::RWNonInterleaved, "RW_NONINTERLEAVED"),
    ]
    .iter()
    .filter(|(a, _)| hw.test_access(*a).is_ok())
    .map(|(_, n)| *n)
    .collect();
    println!(
        "{device}: opened in {opened:?}\n  formats: {}\n  rates: {}..{} Hz; standard rates accepted: {}\n  channels: {}..{}\n  period: {}..{} frames; periods {}..{}; buffer: {}..{} frames\n  access: {}",
        formats.join(" "),
        hw.get_rate_min()?,
        hw.get_rate_max()?,
        rates.join(" "),
        hw.get_channels_min()?,
        hw.get_channels_max()?,
        hw.get_period_size_min()?,
        hw.get_period_size_max()?,
        hw.get_periods_min()?,
        hw.get_periods_max()?,
        hw.get_buffer_size_min()?,
        hw.get_buffer_size_max()?,
        access.join(" "),
    );
    let t_close = Instant::now();
    drop(hw);
    drop(pcm);
    println!("  closed in {:?}", t_close.elapsed());
    Ok(())
}

fn open(device: &str, retry_ms: u64) -> Result<()> {
    let t = Instant::now();
    let deadline = Duration::from_millis(retry_ms);
    let mut attempts = 0;
    loop {
        attempts += 1;
        match PCM::new(device, Direction::Playback, true) {
            Ok(pcm) => {
                println!(
                    "{device}: opened after {:?} ({attempts} attempts); closing",
                    t.elapsed()
                );
                drop(pcm);
                return Ok(());
            }
            Err(e) => {
                if t.elapsed() >= deadline {
                    println!(
                        "{device}: open failed after {:?} ({attempts} attempts): {}",
                        t.elapsed(),
                        fmt_err(&e)
                    );
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn parse_format(s: &str) -> Result<Format> {
    Ok(match s {
        "S16_LE" => Format::S16LE,
        "S24_LE" => Format::S24LE,
        "S24_3LE" => Format::S243LE,
        "S32_LE" => Format::S32LE,
        other => return Err(anyhow!("unsupported format {other}")),
    })
}

/// Exact negotiation as the exclusive mode would do it: no plug layer, no
/// resampling; refuse instead of converting.
fn negotiate(device: &str, rate: u32, format: &str, channels: u32, silence_ms: u64) -> Result<()> {
    let fmt = parse_format(format)?;
    let t = Instant::now();
    let pcm = match PCM::new(device, Direction::Playback, false) {
        Ok(p) => p,
        Err(e) => {
            println!("{device}: open failed: {}", fmt_err(&e));
            return Ok(());
        }
    };
    let t_open = t.elapsed();
    {
        let hw = HwParams::any(&pcm)?;
        // No resampling inside alsa-lib: the rate must be exact.
        hw.set_rate_resample(false)?;
        hw.set_access(Access::RWInterleaved)?;
        if let Err(e) = hw.set_format(fmt) {
            println!("{device}: format {format} refused: {}", fmt_err(&e));
            return Ok(());
        }
        if let Err(e) = hw.set_channels(channels) {
            println!("{device}: {channels} channels refused: {}", fmt_err(&e));
            return Ok(());
        }
        if let Err(e) = hw.set_rate(rate, ValueOr::Nearest) {
            println!("{device}: rate {rate} refused: {}", fmt_err(&e));
            return Ok(());
        }
        // About 200 ms of buffer in four periods, as the engine plans.
        hw.set_buffer_time_near(200_000, ValueOr::Nearest)?;
        hw.set_periods_near(4, ValueOr::Nearest)?;
        pcm.hw_params(&hw)?;
    }
    let t_params = t.elapsed();
    let hw = pcm.hw_params_current()?;
    let (buffer, period) = (hw.get_buffer_size()?, hw.get_period_size()?);
    println!(
        "{device}: negotiated {} {} Hz {} ch; buffer {buffer} frames ({:.1} ms), period {period} frames ({:.1} ms); open {t_open:?}, hw_params {:?}",
        hw.get_format()?,
        hw.get_rate()?,
        hw.get_channels()?,
        buffer as f64 * 1000.0 / rate as f64,
        period as f64 * 1000.0 / rate as f64,
        t_params - t_open
    );
    drop(hw);
    {
        let sw = pcm.sw_params_current()?;
        // Start once one period is queued, so a short write really runs.
        sw.set_start_threshold(period)?;
        sw.set_avail_min(period)?;
        pcm.sw_params(&sw)?;
    }
    pcm.prepare()?;

    if silence_ms > 0 {
        let frames_total = rate as u64 * silence_ms / 1000;
        let frame_bytes = pcm.frames_to_bytes(1) as usize;
        // Zero bytes are digital silence for every signed integer format.
        let zeros = vec![0u8; period as usize * frame_bytes];
        let io = pcm.io_bytes();
        let mut written = 0u64;
        let mut xruns = 0;
        let t_write = Instant::now();
        let mut first_running = None;
        while written < frames_total {
            match io.writei(&zeros) {
                Ok(n) => written += n as u64,
                Err(e) => {
                    xruns += 1;
                    pcm.try_recover(e, true)?;
                }
            }
            if first_running.is_none() && pcm.state() == State::Running {
                first_running = Some(t_write.elapsed());
            }
        }
        let delay = pcm.delay().unwrap_or(-1);
        println!(
            "{device}: wrote {written} frames of silence in {:?}; running after {:?}; delay {delay} frames; xruns {xruns}; state {:?}",
            t_write.elapsed(),
            first_running,
            pcm.state()
        );
        pcm.drop()?;
    }
    drop(pcm);
    println!("{device}: closed; total {:?}", t.elapsed());
    Ok(())
}
