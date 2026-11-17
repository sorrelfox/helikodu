// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test signals, the files made from them, and reading the WAV sink's output.

use std::f64::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// A test signal: interleaved 16-bit stereo.
#[derive(Debug, Clone)]
pub(crate) struct Signal {
    pub(crate) rate: u32,
    pub(crate) samples: Vec<i16>,
}

impl Signal {
    pub(crate) fn frames(&self) -> usize {
        self.samples.len() / 2
    }

    /// The frames `from .. to`.
    pub(crate) fn slice(&self, from: usize, to: usize) -> Self {
        Self {
            rate: self.rate,
            samples: self.samples[from * 2..to * 2].to_vec(),
        }
    }
}

/// A chirp on each channel with a little noise: no two stretches are alike, so a
/// misplaced sample shows.
pub(crate) fn chirp(rate: u32, frames: usize) -> Signal {
    let r = f64::from(rate);
    let dur = frames as f64 / r;
    let mut lcg: u32 = 0x1234_5678;
    let mut noise = move || {
        lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(lcg >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0
    };
    let sweep = |t: f64, f0: f64, f1: f64| (TAU * (f0 * t + (f1 - f0) * t * t / (2.0 * dur))).sin();
    let mut samples = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f64 / r;
        let l = 0.25 * sweep(t, 100.0, 8000.0) + 0.02 * noise();
        let r = 0.20 * sweep(t, 6000.0, 150.0) + 0.02 * noise();
        samples.push(to_i16(l));
        samples.push(to_i16(r));
    }
    Signal { rate, samples }
}

/// A slow, loud sine on both channels (the right one shifted a little): its natural
/// steps are small, so a click stands out.
pub(crate) fn sine(rate: u32, frames: usize) -> Signal {
    let r = f64::from(rate);
    let mut samples = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f64 / r;
        samples.push(to_i16(SINE_AMPLITUDE * (TAU * SINE_HZ * t).sin()));
        samples.push(to_i16(SINE_AMPLITUDE * (TAU * SINE_HZ * t + 1.0).sin()));
    }
    Signal { rate, samples }
}

pub(crate) const SINE_HZ: f64 = 220.0;
pub(crate) const SINE_AMPLITUDE: f64 = 0.5;

/// The largest step from one sample to the next of the sine, in 16-bit steps.
pub(crate) fn sine_max_step(rate: u32) -> f64 {
    SINE_AMPLITUDE * TAU * SINE_HZ / f64::from(rate) * 32_768.0
}

fn to_i16(x: f64) -> i16 {
    (x * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
}

/// The folder for this test's files.
pub(crate) fn dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
        let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine-transport");
        fs::create_dir_all(&d).expect("create the test folder");
        d
    })
}

/// A path in the test folder.
pub(crate) fn path(name: &str) -> PathBuf {
    dir().join(name)
}

/// Writes `signal` as a 16-bit WAV file.
pub(crate) fn write_wav(name: &str, signal: &Signal) -> PathBuf {
    let data_len = signal.samples.len() * 2;
    let mut b = Vec::with_capacity(44 + data_len);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&signal.rate.to_le_bytes());
    b.extend_from_slice(&(signal.rate * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data_len as u32).to_le_bytes());
    for v in &signal.samples {
        b.extend_from_slice(&v.to_le_bytes());
    }
    let path = path(name);
    fs::write(&path, b).unwrap();
    path
}

fn have_encoder(name: &str) -> bool {
    static LIST: OnceLock<Option<String>> = OnceLock::new();
    let list = LIST.get_or_init(|| {
        Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    });
    list.as_ref().is_some_and(|list| {
        list.lines()
            .any(|l| l.split_whitespace().nth(1) == Some(name))
    })
}

/// Encodes the WAV file `input` with the `ffmpeg` tool. `None`, after printing why,
/// when the tool or the encoder is missing.
pub(crate) fn encode(input: &Path, name: &str, encoder: &str, args: &[&str]) -> Option<PathBuf> {
    if !have_encoder(encoder) {
        eprintln!("SKIPPED: no ffmpeg tool with the {encoder} encoder; {name} not tested");
        return None;
    }
    let out = path(name);
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(args)
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(out)
}

/// What the WAV sink wrote.
#[derive(Debug)]
pub(crate) struct Written {
    pub(crate) rate: u32,
    pub(crate) channels: usize,
    /// 16-bit output, as written.
    pub(crate) ints: Vec<i16>,
    /// 32-bit float output, as written.
    pub(crate) floats: Vec<f32>,
}

impl Written {
    pub(crate) fn frames(&self) -> usize {
        (self.ints.len() + self.floats.len()) / self.channels
    }
}

/// Reads a file the WAV sink wrote (`WAVE_FORMAT_EXTENSIBLE`, 68-byte header).
pub(crate) fn read_output(path: &Path) -> Written {
    let b = fs::read(path).unwrap();
    assert_eq!(&b[0..4], b"RIFF");
    let channels = usize::from(u16::from_le_bytes([b[22], b[23]]));
    let rate = u32::from_le_bytes(b[24..28].try_into().unwrap());
    let bits = u16::from_le_bytes([b[34], b[35]]);
    let float = b[44] == 3;
    let len = u32::from_le_bytes(b[64..68].try_into().unwrap()) as usize;
    let data = &b[68..68 + len];
    let mut written = Written {
        rate,
        channels,
        ints: Vec::new(),
        floats: Vec::new(),
    };
    match (float, bits) {
        (false, 16) => {
            written.ints = data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes(*c))
                .collect();
        }
        (true, 32) => {
            written.floats = data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect();
        }
        other => panic!("unexpected output format {other:?}"),
    }
    written
}

/// The level of the anti-click fade in after `k` frames of a fade `length` frames
/// long, as the pipeline's fader computes it.
pub(crate) fn fade_in_level(k: usize, length: usize) -> f64 {
    if k + 1 >= length {
        return 1.0;
    }
    let t = (k + 1) as f64 / length as f64;
    0.5 - 0.5 * (std::f64::consts::PI * t).cos()
}

/// The length in frames of a fade of `millis` at `rate`.
pub(crate) fn fade_frames(millis: u32, rate: u32) -> usize {
    (f64::from(millis) * f64::from(rate) / 1000.0).round() as usize
}

/// The largest step between consecutive frames of either channel.
pub(crate) fn max_step(samples: &[i16]) -> f64 {
    samples
        .as_chunks::<2>()
        .0
        .windows(2)
        .flat_map(|w| [(w[1][0], w[0][0]), (w[1][1], w[0][1])])
        .map(|(a, b)| (f64::from(a) - f64::from(b)).abs())
        .fold(0.0, f64::max)
}
