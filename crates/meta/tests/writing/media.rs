// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test media, generated at test time: a WAV file written by the tests and
//! encoded by the `ffmpeg` tool, and pictures encoded by the `image` crate.

use std::ffi::OsStr;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// One second of a quiet 440 Hz tone, 44.1 kHz, stereo, 16 bit.
fn wav_bytes() -> Vec<u8> {
    let rate = 44_100u32;
    let frames = rate as usize;
    let mut pcm = Vec::with_capacity(frames * 4);
    for i in 0..frames {
        let t = i as f64 / f64::from(rate);
        let s = ((t * 440.0 * std::f64::consts::TAU).sin() * 3000.0) as i16;
        pcm.extend_from_slice(&s.to_le_bytes());
        pcm.extend_from_slice(&s.to_le_bytes());
    }
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(&pcm);
    out
}

/// The folder all generated media go to.
fn root() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
        let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("meta-tags-writing");
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).expect("create the media folder");
        fs::write(d.join("source.wav"), wav_bytes()).expect("write the source WAV");
        d
    })
}

/// A fresh, empty folder for one test.
pub(crate) fn folder(name: &str) -> PathBuf {
    let dir = root().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the test folder");
    dir
}

/// The names in `dir`, sorted.
pub(crate) fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("list the folder")
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
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

/// One audio format the tests write.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Format {
    /// The file extension.
    pub(crate) extension: &'static str,
    /// The ffmpeg encoder.
    pub(crate) encoder: &'static str,
    /// The ffmpeg output format.
    pub(crate) muxer: &'static str,
}

/// Every format whose tags are written, as ffmpeg can make it.
pub(crate) const WRITABLE: &[Format] = &[
    Format {
        extension: "mp3",
        encoder: "libmp3lame",
        muxer: "mp3",
    },
    Format {
        extension: "aac",
        encoder: "aac",
        muxer: "adts",
    },
    Format {
        extension: "flac",
        encoder: "flac",
        muxer: "flac",
    },
    Format {
        extension: "ogg",
        encoder: "libvorbis",
        muxer: "ogg",
    },
    Format {
        extension: "opus",
        encoder: "libopus",
        muxer: "opus",
    },
    Format {
        extension: "m4a",
        encoder: "aac",
        muxer: "ipod",
    },
    Format {
        extension: "wv",
        encoder: "wavpack",
        muxer: "wv",
    },
    Format {
        extension: "wav",
        encoder: "pcm_s16le",
        muxer: "wav",
    },
    Format {
        extension: "aiff",
        encoder: "pcm_s16be",
        muxer: "aiff",
    },
];

/// Encodes the source WAV into `dir/name` with `format` and the extra ffmpeg
/// `args` (such as `-metadata` pairs). `None`, after printing why, when ffmpeg or
/// the encoder is missing.
pub(crate) fn encode<S: AsRef<OsStr>>(
    dir: &Path,
    name: &str,
    format: Format,
    args: &[S],
) -> Option<PathBuf> {
    if !have_ffmpeg() {
        eprintln!("SKIPPED: ffmpeg is not installed; {name} not tested");
        return None;
    }
    if !have_encoder(format.encoder) {
        eprintln!(
            "SKIPPED: ffmpeg has no {} encoder; {name} not tested",
            format.encoder
        );
        return None;
    }
    let out = dir.join(name);
    let result = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(root().join("source.wav"))
        .args(["-map_metadata", "-1", "-c:a", format.encoder])
        .args(args)
        .args(["-f", format.muxer])
        .arg(&out)
        .output()
        .expect("run ffmpeg");
    assert!(
        result.status.success(),
        "ffmpeg failed for {name}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Some(out)
}

/// `-metadata key=value` arguments for each pair.
pub(crate) fn metadata(pairs: &[(&str, &str)]) -> Vec<String> {
    pairs
        .iter()
        .flat_map(|(k, v)| ["-metadata".to_owned(), format!("{k}={v}")])
        .collect()
}

/// A small PNG picture filled with `rgb`.
pub(crate) fn png(rgb: [u8; 3]) -> Vec<u8> {
    let picture = image::RgbImage::from_pixel(8, 8, image::Rgb(rgb));
    let mut bytes = Cursor::new(Vec::new());
    picture
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode the PNG");
    bytes.into_inner()
}

/// A small JPEG picture filled with `rgb`.
pub(crate) fn jpeg(rgb: [u8; 3]) -> Vec<u8> {
    let picture = image::RgbImage::from_pixel(8, 8, image::Rgb(rgb));
    let mut bytes = Cursor::new(Vec::new());
    picture
        .write_to(&mut bytes, image::ImageFormat::Jpeg)
        .expect("encode the JPEG");
    bytes.into_inner()
}
