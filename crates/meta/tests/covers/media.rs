// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test pictures and media, generated at test time.

use std::ffi::OsStr;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbImage};

/// A `width` × `height` picture: `color` with a diagonal gradient, so that it
/// is not trivially compressible and scaling has something to do.
pub(crate) fn rgb(width: u32, height: u32, color: [u8; 3]) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        let shade = ((x + y) % 32) as u8;
        image::Rgb([
            color[0].saturating_sub(shade),
            color[1].saturating_sub(shade),
            color[2].saturating_sub(shade),
        ])
    })
}

pub(crate) fn jpeg(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&rgb(width, height, color))
        .expect("encode a JPEG");
    out
}

pub(crate) fn png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(
            rgb(width, height, color).as_raw(),
            width,
            height,
            ExtendedColorType::Rgb8,
        )
        .expect("encode a PNG");
    out
}

pub(crate) fn webp(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    WebPEncoder::new_lossless(Cursor::new(&mut out))
        .write_image(
            rgb(width, height, color).as_raw(),
            width,
            height,
            ExtendedColorType::Rgb8,
        )
        .expect("encode a WebP");
    out
}

/// A 1 × 1 GIF.
pub(crate) const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff\x00\x00\x00!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A well-formed PNG header claiming `width` × `height` 8-bit RGBA pixels,
/// followed by a tiny `IDAT` (one stored, empty zlib block) and `IEND`.
pub(crate) fn png_claiming(width: u32, height: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(
        &mut out,
        b"IDAT",
        &[0x78, 0x01, 0x01, 0x00, 0x00, 0xFF, 0xFF, 0, 0, 0, 1],
    );
    png_chunk(&mut out, b"IEND", &[]);
    out
}

/// A real decompression bomb: a `side` × `side` 8-bit grey PNG of zeros, a few
/// kilobytes encoded.
pub(crate) fn png_bomb(side: u32) -> Vec<u8> {
    let pixels = vec![0u8; (side * side) as usize];
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&pixels, side, side, ExtendedColorType::L8)
        .expect("encode a PNG");
    out
}

/// A fresh, empty folder under the test temporary directory.
pub(crate) fn fresh_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let d = root().join(format!("{name}-{n}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).expect("create a test folder");
    d
}

fn root() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Quiet);
        let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("meta-covers");
        fs::create_dir_all(&d).expect("create the media folder");
        fs::write(d.join("source.wav"), wav_bytes()).expect("write the source WAV");
        d
    })
}

/// A quarter second of silence, 44.1 kHz, stereo, 16 bit.
fn wav_bytes() -> Vec<u8> {
    let pcm = vec![0u8; 44_100 / 4 * 4];
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&44_100u32.to_le_bytes());
    out.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(&pcm);
    out
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
    list.as_deref()
        .is_some_and(|l| l.lines().any(|l| l.split_whitespace().nth(1) == Some(name)))
}

/// Runs ffmpeg on the source WAV, writing `out` in `dir`: `inputs` are extra
/// `-i` inputs, `args` everything after them. `None`, after printing why, when
/// ffmpeg or `encoder` is missing.
pub(crate) fn ffmpeg<S: AsRef<OsStr>>(
    dir: &Path,
    out: &str,
    encoder: &str,
    inputs: &[&Path],
    args: &[S],
) -> Option<PathBuf> {
    if !have_encoder(encoder) {
        eprintln!("SKIPPED: ffmpeg or its {encoder} encoder is missing; {out} not tested");
        return None;
    }
    let path = dir.join(out);
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(root().join("source.wav"));
    for input in inputs {
        cmd.arg("-i").arg(input);
    }
    let result = cmd
        .args(["-map_metadata", "-1", "-c:a", encoder])
        .args(args)
        .arg(&path)
        .output()
        .expect("run ffmpeg");
    assert!(
        result.status.success(),
        "ffmpeg failed for {out}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Some(path)
}

/// Encodes the source WAV into `out` with `encoder` and no pictures.
pub(crate) fn plain(dir: &Path, out: &str, encoder: &str) -> Option<PathBuf> {
    ffmpeg::<&str>(dir, out, encoder, &[], &[])
}

/// Encodes the source WAV into `out` with `encoder`, with `picture` as an
/// attached picture whose `comment` (the ID3v2/FLAC picture type) is `kind`.
pub(crate) fn with_attached_picture(
    dir: &Path,
    out: &str,
    encoder: &str,
    picture: &Path,
    kind: Option<&str>,
) -> Option<PathBuf> {
    let mut args = vec![
        "-map".to_owned(),
        "0:a".to_owned(),
        "-map".to_owned(),
        "1:v".to_owned(),
        "-c:v".to_owned(),
        "copy".to_owned(),
        "-disposition:v".to_owned(),
        "attached_pic".to_owned(),
    ];
    if let Some(kind) = kind {
        args.push("-metadata:s:v".to_owned());
        args.push(format!("comment={kind}"));
    }
    ffmpeg(dir, out, encoder, &[picture], &args)
}
