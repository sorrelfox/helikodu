// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test media: a reference signal and the files encoded from it.

use std::collections::HashMap;
use std::f64::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use helikodu_ffmpeg_bridge::{AudioDecoder, MediaFile, PcmSamples, StreamChoice};
use helikodu_model::audio::AudioSpec;

/// Frame counts that are not multiples of any codec frame size.
pub(crate) const FRAMES_44K: usize = 220_757;
pub(crate) const FRAMES_48K: usize = 240_263;

/// A reference signal: stereo, a rising chirp on the left and a falling one on the
/// right, plus a little noise. Never periodic, so a cross-correlation has one peak.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    S44k16,
    S48k16,
    S48k24,
}

impl Source {
    pub(crate) fn rate(self) -> u32 {
        match self {
            Self::S44k16 => 44_100,
            _ => 48_000,
        }
    }

    pub(crate) fn frames(self) -> usize {
        match self {
            Self::S44k16 => FRAMES_44K,
            _ => FRAMES_48K,
        }
    }

    pub(crate) fn bits(self) -> u32 {
        match self {
            Self::S48k24 => 24,
            _ => 16,
        }
    }

    fn wav_name(self) -> &'static str {
        match self {
            Self::S44k16 => "src_44k_16.wav",
            Self::S48k16 => "src_48k_16.wav",
            Self::S48k24 => "src_48k_24.wav",
        }
    }
}

/// The reference signal as integers of the source's bit depth, interleaved stereo.
pub(crate) fn reference_ints(src: Source) -> Vec<i32> {
    let rate = f64::from(src.rate());
    let n = src.frames();
    let dur = n as f64 / rate;
    let scale = f64::from(1u32 << (src.bits() - 1));
    let mut lcg: u32 = 0x1234_5678;
    let mut noise = move || {
        lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(lcg >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0
    };
    let chirp = |t: f64, f0: f64, f1: f64| (TAU * (f0 * t + (f1 - f0) * t * t / (2.0 * dur))).sin();
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / rate;
        let l = 0.25 * chirp(t, 100.0, 8000.0) + 0.02 * noise();
        let r = 0.20 * chirp(t, 6000.0, 150.0) + 0.02 * noise();
        for x in [l, r] {
            out.push((x * scale).round().clamp(-scale, scale - 1.0) as i32);
        }
    }
    out
}

/// The reference signal normalised to [-1, 1).
pub(crate) fn reference(src: Source) -> Vec<f64> {
    let scale = f64::from(1u32 << (src.bits() - 1));
    reference_ints(src)
        .into_iter()
        .map(|v| f64::from(v) / scale)
        .collect()
}

fn wav_bytes(src: Source) -> Vec<u8> {
    let ints = reference_ints(src);
    let bytes_per = (src.bits() / 8) as usize;
    let data_len = ints.len() * bytes_per;
    let block_align = 2 * bytes_per as u16;
    let mut b = Vec::with_capacity(44 + data_len);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&src.rate().to_le_bytes());
    b.extend_from_slice(&(src.rate() * u32::from(block_align)).to_le_bytes());
    b.extend_from_slice(&block_align.to_le_bytes());
    b.extend_from_slice(&(src.bits() as u16).to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data_len as u32).to_le_bytes());
    for v in ints {
        b.extend_from_slice(&v.to_le_bytes()[..bytes_per]);
    }
    b
}

/// The folder all generated media go to.
pub(crate) fn dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        // The refused codec opens and attacks make FFmpeg log a lot of errors.
        helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
        let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ffmpeg-bridge-media");
        fs::create_dir_all(&d).expect("create the media folder");
        d
    })
}

/// Serialises generation, and remembers what failed.
static MADE: Mutex<Option<HashMap<String, Option<PathBuf>>>> = Mutex::new(None);

/// Runs `make` once per name; `None` (with a message) when it could not.
fn once(name: &str, make: impl FnOnce(&Path) -> Result<(), String>) -> Option<PathBuf> {
    let mut made = MADE.lock().unwrap_or_else(|p| p.into_inner());
    let made = made.get_or_insert_with(HashMap::new);
    if let Some(done) = made.get(name) {
        return done.clone();
    }
    let path = dir().join(name);
    let result = if path.exists() {
        Some(path)
    } else {
        let tmp = dir().join(format!("tmp-{name}"));
        let _ = fs::remove_file(&tmp);
        match make(&tmp).and_then(|()| fs::rename(&tmp, &path).map_err(|e| e.to_string())) {
            Ok(()) => Some(path),
            Err(e) => {
                eprintln!("SKIPPED: cannot create {name}: {e}");
                None
            }
        }
    };
    made.insert(name.to_owned(), result.clone());
    result
}

/// The reference signal as a WAV file (written by the tests, no tool needed).
pub(crate) fn source_wav(src: Source) -> PathBuf {
    once(src.wav_name(), |p| {
        fs::write(p, wav_bytes(src)).map_err(|e| e.to_string())
    })
    .expect("writing a WAV file never needs a tool")
}

/// Whether `tool` can be run.
pub(crate) fn have_tool(tool: &str) -> bool {
    Command::new(tool)
        .arg(if tool == "ffmpeg" {
            "-version"
        } else {
            "--version"
        })
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Whether the `ffmpeg` tool has the encoder `name`.
pub(crate) fn have_encoder(name: &str) -> bool {
    static LIST: OnceLock<String> = OnceLock::new();
    let list = LIST.get_or_init(|| {
        Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    });
    list.lines()
        .any(|l| l.split_whitespace().nth(1) == Some(name))
}

fn run(tool: &str, args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new(tool);
    if tool == "ffmpeg" {
        cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    }
    let out = cmd
        .args(args)
        .output()
        .map_err(|e| format!("{tool}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{tool} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// A small PNG for cover art.
pub(crate) fn cover_png() -> Option<PathBuf> {
    if !have_tool("ffmpeg") {
        eprintln!("SKIPPED: ffmpeg is not installed");
        return None;
    }
    once("cover.png", |p| {
        run(
            "ffmpeg",
            &strings(&[
                "-f",
                "lavfi",
                "-i",
                "color=c=0x3080c0:s=64x64",
                "-frames:v",
                "1",
                "-f",
                "image2",
                "-c:v",
                "png",
                &s(p),
            ]),
        )
    })
}

/// Encodes a file with `tool`. In `args`, `{in}` is the source WAV, `{out}` the
/// output and `{cover}` a PNG. `encoders` are ffmpeg encoders the command needs.
/// `None`, after printing why, when the tool or an encoder is missing.
pub(crate) fn encode(
    name: &str,
    src: Source,
    tool: &str,
    encoders: &[&str],
    args: &[&str],
) -> Option<PathBuf> {
    if !have_tool(tool) {
        eprintln!("SKIPPED: {tool} is not installed; {name} not tested");
        return None;
    }
    if !have_tool("ffmpeg") {
        eprintln!("SKIPPED: ffmpeg is not installed; {name} not tested");
        return None;
    }
    for e in encoders {
        if !have_encoder(e) {
            eprintln!("SKIPPED: ffmpeg has no {e} encoder; {name} not tested");
            return None;
        }
    }
    let input = source_wav(src);
    let cover = if args.contains(&"{cover}") {
        cover_png()?
    } else {
        PathBuf::new()
    };
    once(name, |out| {
        let args: Vec<String> = args
            .iter()
            .map(|a| {
                a.replace("{in}", &s(&input))
                    .replace("{out}", &s(out))
                    .replace("{cover}", &s(&cover))
            })
            .collect();
        run(tool, &args)
    })
}

/// Shorthand for an ffmpeg encode; the output format comes from `format`.
pub(crate) fn ffmpeg(
    name: &str,
    src: Source,
    encoders: &[&str],
    args: &[&str],
    format: &str,
) -> Option<PathBuf> {
    let mut all = vec!["-i", "{in}"];
    all.extend_from_slice(args);
    all.extend_from_slice(&["-f", format, "{out}"]);
    encode(name, src, "ffmpeg", encoders, &all)
}

pub(crate) fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn strings(a: &[&str]) -> Vec<String> {
    a.iter().map(|x| (*x).to_owned()).collect()
}

/// A whole decoded stream.
#[derive(Debug)]
pub(crate) struct Decoded {
    pub(crate) spec: AudioSpec,
    /// Normalised to [-1, 1).
    pub(crate) samples: Vec<f64>,
    /// The exact integer values, for integer output.
    pub(crate) ints: Vec<i64>,
    pub(crate) decoder: AudioDecoder,
}

impl Decoded {
    pub(crate) fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.spec.channel_count())
    }
}

/// Appends a block's samples.
pub(crate) fn append(samples: &PcmSamples, floats: &mut Vec<f64>, ints: &mut Vec<i64>) {
    match samples {
        PcmSamples::U8(v) => {
            floats.extend(v.iter().map(|&x| (f64::from(x) - 128.0) / 128.0));
            ints.extend(v.iter().map(|&x| i64::from(x)));
        }
        PcmSamples::S16(v) => {
            floats.extend(v.iter().map(|&x| f64::from(x) / 32_768.0));
            ints.extend(v.iter().map(|&x| i64::from(x)));
        }
        PcmSamples::S32(v) => {
            floats.extend(v.iter().map(|&x| f64::from(x) / 2_147_483_648.0));
            ints.extend(v.iter().map(|&x| i64::from(x)));
        }
        PcmSamples::F32(v) => floats.extend(v.iter().map(|&x| f64::from(x))),
    }
}

/// Decodes the rest of a decoder's stream.
pub(crate) fn decode_rest(mut decoder: AudioDecoder) -> Decoded {
    let mut samples = Vec::new();
    let mut ints = Vec::new();
    let mut expected = decoder.position();
    while let Some(block) = decoder.read().expect("decoding") {
        assert_eq!(block.position(), expected, "blocks are contiguous");
        expected += block.frames() as u64;
        assert_eq!(
            block.samples().len(),
            block.frames() * usize::from(block.spec().channel_count())
        );
        append(block.samples(), &mut samples, &mut ints);
    }
    Decoded {
        spec: decoder.spec(),
        samples,
        ints,
        decoder,
    }
}

/// Opens and decodes a whole file.
pub(crate) fn decode(path: &Path) -> Decoded {
    let decoder = MediaFile::open(path)
        .and_then(|m| m.into_decoder(StreamChoice::Auto))
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    decode_rest(decoder)
}

/// The lag, in frames, at which channel 0 of `decoded` best matches `reference`,
/// searched over ±`range` around a window in the middle. 0 means aligned.
pub(crate) fn lag(decoded: &[f64], reference: &[f64], channels: usize, range: i64) -> i64 {
    let ch0 = |v: &[f64], i: usize| v.get(i * channels).copied().unwrap_or(0.0);
    let frames = reference.len() / channels;
    let start = frames / 2;
    let window = 4096;
    let mut best = (f64::MIN, 0);
    for lag in -range..=range {
        let mut sum = 0.0;
        for i in start..start + window {
            let j = i as i64 + lag;
            if j >= 0 {
                sum += ch0(reference, i) * ch0(decoded, j as usize);
            }
        }
        if sum > best.0 {
            best = (sum, lag);
        }
    }
    best.1
}
