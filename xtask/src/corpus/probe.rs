// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checks a made file against its recipe's expectations with `ffprobe` and,
//! for the length, a full decode with `ffmpeg`.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::recipe::Expect;

pub(crate) fn verify(path: &Path, expect: &Expect) -> Result<()> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=format_name:stream=codec_type,codec_name,sample_rate,channels",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .context("could not start `ffprobe`")?;
    if !output.status.success() {
        bail!(
            "ffprobe cannot read it: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let report: Value =
        serde_json::from_slice(&output.stdout).context("ffprobe printed invalid JSON")?;
    let streams = report["streams"].as_array().cloned().unwrap_or_default();
    let of_type = |kind: &str| {
        streams
            .iter()
            .filter(|stream| stream["codec_type"] == kind)
            .collect::<Vec<_>>()
    };
    let audio = of_type("audio");
    let video = of_type("video");

    let mut problems = Vec::new();
    let mut compare = |what: &str, expected: String, actual: String| {
        if expected != actual {
            problems.push(format!("{what} is {actual}, expected {expected}"));
        }
    };
    if let Some(demuxer) = &expect.demuxer {
        let actual = report["format"]["format_name"].as_str().unwrap_or("none");
        compare("demuxer", demuxer.clone(), actual.to_owned());
    }
    if let Some(count) = expect.audio_streams {
        compare(
            "audio stream count",
            count.to_string(),
            audio.len().to_string(),
        );
    }
    if let Some(count) = expect.video_streams {
        compare(
            "video stream count",
            count.to_string(),
            video.len().to_string(),
        );
    }
    let first = audio.first();
    let field = |name: &str| {
        first.map_or_else(
            || String::from("none"),
            |stream| match &stream[name] {
                Value::String(text) => text.clone(),
                Value::Null => String::from("none"),
                other => other.to_string(),
            },
        )
    };
    if let Some(codec) = &expect.codec {
        compare("codec", codec.clone(), field("codec_name"));
    }
    if let Some(rate) = expect.sample_rate {
        compare("sample rate", rate.to_string(), field("sample_rate"));
    }
    if let Some(channels) = expect.channels {
        compare("channel count", channels.to_string(), field("channels"));
    }
    if let Some(samples) = expect.samples {
        let channels: u64 = field("channels").parse().unwrap_or(0);
        let decoded = decoded_frames(path, channels)?;
        compare("decoded length", samples.to_string(), decoded.to_string());
    }
    if !problems.is_empty() {
        bail!("{}", problems.join("; "));
    }
    Ok(())
}

/// The number of sample frames FFmpeg decodes from the first audio stream.
fn decoded_frames(path: &Path, channels: u64) -> Result<u64> {
    if channels == 0 {
        bail!("there is no audio stream to decode");
    }
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-c:a", "pcm_s32le", "-f", "s32le", "-"])
        .stdin(Stdio::null())
        .output()
        .context("could not start `ffmpeg`")?;
    if !output.status.success() {
        bail!(
            "ffmpeg cannot decode it: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout.len() as u64 / (4 * channels))
}
