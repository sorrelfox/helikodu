// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The one-second silent file every track is copied from, made once per run
//! by FFmpeg or, where FFmpeg lacks the encoder, by `flac` or `opusenc`.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use super::Format;
use crate::corpus::tools;

/// Makes the template in `scratch` (which is created and removed) and returns
/// its bytes and a description of the encoder that made it.
pub(crate) fn make(format: Format, scratch: &Path) -> Result<(Vec<u8>, String)> {
    fs::create_dir_all(scratch)
        .with_context(|| format!("could not create {}", scratch.display()))?;
    let result = make_in(format, scratch);
    let _ = fs::remove_dir_all(scratch);
    result
}

fn make_in(format: Format, scratch: &Path) -> Result<(Vec<u8>, String)> {
    let rate = match format {
        Format::Flac => 44_100,
        Format::Opus => 48_000,
    };
    let wav = scratch.join("silence.wav");
    fs::write(&wav, silent_wav(rate, 2, rate))
        .with_context(|| format!("could not write {}", wav.display()))?;
    let output = scratch.join(format!("template.{}", format.extension()));

    let has_ffmpeg = tools::find_program("ffmpeg").is_some();
    let codec = match format {
        Format::Flac => "flac",
        Format::Opus => "libopus",
    };
    let (program, args): (&str, Vec<String>) = if has_ffmpeg && ffmpeg_has_encoder(codec) {
        let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        args.push(wav.display().to_string());
        args.extend(
            [
                "-map_metadata",
                "-1",
                "-fflags",
                "+bitexact",
                "-flags",
                "+bitexact",
                "-c:a",
                codec,
            ]
            .iter()
            .map(|s| (*s).to_owned()),
        );
        if format == Format::Opus {
            args.extend(["-b:a", "32k"].iter().map(|s| (*s).to_owned()));
        }
        args.push(output.display().to_string());
        ("ffmpeg", args)
    } else {
        match format {
            Format::Flac => (
                "flac",
                vec![
                    String::from("--silent"),
                    String::from("--no-padding"),
                    String::from("-o"),
                    output.display().to_string(),
                    wav.display().to_string(),
                ],
            ),
            Format::Opus => (
                "opusenc",
                vec![
                    String::from("--quiet"),
                    String::from("--bitrate"),
                    String::from("32"),
                    wav.display().to_string(),
                    output.display().to_string(),
                ],
            ),
        }
    };
    if tools::find_program(program).is_none() {
        bail!(
            "making the {} template needs FFmpeg with the `{codec}` encoder or `{program}`",
            format.extension()
        );
    }
    let result = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("could not start `{program}`"))?;
    if !result.status.success() {
        bail!(
            "`{program}` failed ({}): {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    let bytes =
        fs::read(&output).with_context(|| format!("could not read {}", output.display()))?;
    let encoder = if program == "ffmpeg" {
        format!("ffmpeg {codec}")
    } else {
        program.to_owned()
    };
    Ok((bytes, encoder))
}

fn ffmpeg_has_encoder(name: &str) -> bool {
    let Ok(output) = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some(name))
}

/// A 16-bit PCM WAV file of `frames` silent frames.
fn silent_wav(rate: u32, channels: u16, frames: u32) -> Vec<u8> {
    let block = u32::from(channels) * 2;
    let data = frames * block;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block).to_le_bytes());
    out.extend_from_slice(&(block as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    out.resize(44 + data as usize, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header() {
        let wav = silent_wav(48_000, 2, 10);
        assert_eq!(wav.len(), 44 + 40);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]),
            192_000
        );
    }
}
