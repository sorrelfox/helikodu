// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test media, generated at test time: a WAV file written by the tests, encoded by
//! the `ffmpeg` tool with `-metadata`, then edited with lofty where a test needs
//! frames FFmpeg cannot write.

use std::ffi::OsStr;
use std::fs;
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
pub(crate) fn dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
        let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("meta-tags-media");
        fs::create_dir_all(&d).expect("create the media folder");
        fs::write(d.join("source.wav"), wav_bytes()).expect("write the source WAV");
        d
    })
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

/// Encodes the source WAV into `name` with `encoder`, the extra ffmpeg `args`
/// (such as `-metadata` pairs) and the output `format`. Each call writes a fresh
/// file. `None`, after printing why, when ffmpeg or the encoder is missing.
pub(crate) fn encode<S: AsRef<OsStr>>(
    name: &str,
    encoder: &str,
    args: &[S],
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
    let out = dir().join(name);
    let input = dir().join("source.wav");
    let result = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&input)
        .args(["-map_metadata", "-1", "-c:a", encoder])
        .args(args)
        .args(["-f", format])
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

/// Text as a Latin-1 reader shows the bytes of `text` encoded in `label`.
pub(crate) fn as_latin1(text: &str, label: &str) -> String {
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).expect("known label");
    let (bytes, _, errors) = encoding.encode(text);
    assert!(!errors);
    bytes.iter().map(|&b| char::from(b)).collect()
}
