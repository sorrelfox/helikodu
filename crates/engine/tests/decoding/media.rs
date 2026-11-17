// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test media: a reference signal, the files encoded from it, and decoding helpers.

use std::collections::HashMap;
use std::f64::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use helikodu_engine::decode::{BlockSamples, Decoder, DecoderRegistry, PcmBlock};
use helikodu_model::audio::AudioSpec;
use helikodu_model::library::MediaSource;

/// Frame counts that are no multiple of any codec frame size.
pub(crate) const FRAMES_44K: usize = 220_757;
pub(crate) const FRAMES_48K: usize = 240_263;

/// The reference signal: 16-bit stereo, a rising chirp on the left and a falling
/// one on the right, plus a little noise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    S44k,
    S48k,
}

impl Source {
    pub(crate) fn rate(self) -> u32 {
        match self {
            Self::S44k => 44_100,
            Self::S48k => 48_000,
        }
    }

    pub(crate) fn frames(self) -> usize {
        match self {
            Self::S44k => FRAMES_44K,
            Self::S48k => FRAMES_48K,
        }
    }

    fn wav_name(self) -> &'static str {
        match self {
            Self::S44k => "src_44k.wav",
            Self::S48k => "src_48k.wav",
        }
    }
}

/// The reference signal, interleaved stereo 16-bit values.
pub(crate) fn reference(src: Source) -> Vec<i16> {
    let rate = f64::from(src.rate());
    let n = src.frames();
    let dur = n as f64 / rate;
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
            out.push((x * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16);
        }
    }
    out
}

fn wav_bytes(src: Source) -> Vec<u8> {
    let samples = reference(src);
    let data_len = samples.len() * 2;
    let mut b = Vec::with_capacity(44 + data_len);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&src.rate().to_le_bytes());
    b.extend_from_slice(&(src.rate() * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data_len as u32).to_le_bytes());
    for v in samples {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

/// The folder all generated media go to.
pub(crate) fn dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        // FFmpeg logs its refusals of non-media files; nothing to see in tests.
        helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
        let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine-decode-media");
        fs::create_dir_all(&d).expect("create the media folder");
        d
    })
}

/// Serialises generation, and remembers what failed.
static MADE: Mutex<Option<HashMap<String, Option<PathBuf>>>> = Mutex::new(None);

/// Runs `make` once per name; `None` (with a message) when it could not.
pub(crate) fn once(name: &str, make: impl FnOnce(&Path) -> Result<(), String>) -> Option<PathBuf> {
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

fn have_ffmpeg() -> bool {
    static HAVE: OnceLock<bool> = OnceLock::new();
    *HAVE.get_or_init(|| {
        Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

fn have_encoder(name: &str) -> bool {
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

/// Encodes the reference signal with the `ffmpeg` tool: `ffmpeg -i <wav> <args>
/// -f <format> <out>`. `None`, after printing why, when the tool or `encoder` is
/// missing.
pub(crate) fn ffmpeg(
    name: &str,
    src: Source,
    encoder: &str,
    args: &[&str],
    format: &str,
) -> Option<PathBuf> {
    if !have_ffmpeg() {
        eprintln!("SKIPPED: ffmpeg is not installed; {name} not tested");
        return None;
    }
    if !have_encoder(encoder) {
        eprintln!("SKIPPED: ffmpeg has no {encoder} encoder; {name} not tested");
        return None;
    }
    let input = source_wav(src);
    once(name, |out| {
        let output = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(&input)
            .args(args)
            .args(["-f", format])
            .arg(out)
            .output()
            .map_err(|e| format!("ffmpeg: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "ffmpeg failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    })
}

/// Writes `bytes` to a file of the media folder.
pub(crate) fn write(name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir().join(name);
    fs::write(&path, bytes).unwrap();
    path
}

/// A whole decoded source.
#[derive(Debug)]
pub(crate) struct Decoded {
    pub(crate) spec: AudioSpec,
    /// As `f32`, the render side's format.
    pub(crate) floats: Vec<f32>,
    /// The exact values of 16-bit integer output.
    pub(crate) ints: Vec<i16>,
    pub(crate) decoder: Box<dyn Decoder>,
}

impl Decoded {
    pub(crate) fn frames(&self) -> usize {
        self.floats.len() / usize::from(self.spec.channel_count())
    }
}

/// Reads the rest of a decoder, checking that blocks are contiguous.
pub(crate) fn decode_rest(mut decoder: Box<dyn Decoder>) -> Decoded {
    let mut block = PcmBlock::new();
    let mut floats = Vec::new();
    let mut ints = Vec::new();
    let mut expected = decoder.position();
    while decoder.read(&mut block).expect("decoding") {
        assert_eq!(block.position(), expected, "blocks are contiguous");
        assert!(!block.is_empty());
        expected += block.frames() as u64;
        assert_eq!(decoder.position(), expected);
        block.append_f32(&mut floats);
        if let BlockSamples::S16(v) = block.samples() {
            ints.extend_from_slice(v);
        }
    }
    Decoded {
        spec: decoder.spec(),
        floats,
        ints,
        decoder,
    }
}

/// Opens `source` through the default registry and decodes all of it.
pub(crate) fn decode(source: &MediaSource) -> Decoded {
    let decoder = DecoderRegistry::default()
        .open(source)
        .unwrap_or_else(|e| panic!("open {}: {e}", source.path().display()));
    decode_rest(decoder)
}

/// Opens and decodes a whole file.
pub(crate) fn decode_file(path: &Path) -> Decoded {
    decode(&MediaSource::file(path))
}
