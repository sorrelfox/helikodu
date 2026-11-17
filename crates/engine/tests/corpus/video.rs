// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Video files play their audio only, and no video decoder is ever opened.
//!
//! Each file is decoded through the registry and played through the transport.
//! The proof that no video decoder opens comes from FFmpeg itself: the bridge
//! gives every file a codec whitelist of audio decoders, and FFmpeg refuses to
//! open a decoder that is not on it, with an error in its log, before the decoder
//! is initialised. A child process decodes the set with FFmpeg's log on and the
//! parent reads it: every video stream must have been refused, and the whitelist
//! must name no video decoder of the FFmpeg build (checked against the `ffmpeg`
//! tool's decoder list when it is installed, as far as the log quotes the list:
//! FFmpeg cuts log lines at 1 KiB). The bridge's own tests prove the
//! same with a probe on `avcodec_open2` itself.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use helikodu_ffmpeg_bridge::{MediaFile, StreamKind};
use helikodu_model::player::{EngineError, EngineEvent, TrackEndReason, TransportState};

use crate::decoding;
use crate::formats::strict;
use crate::manifest::{Corpus, Entry};
use crate::report::Table;
use crate::session::{Session, item, null_config};

/// Set in the child process that decodes the set with FFmpeg's log on.
const CHILD: &str = "HELIKODU_CORPUS_VIDEO_CHILD";
/// Starts each file's part of the child's log.
const MARK: &str = "=== corpus file ";

/// Video files the engine does not play yet, with the reason.
const KNOWN_GAPS: &[(&str, &str)] = &[
    ("video/mpeg2-mp2.mpg", MPEG_PS),
    ("video/mpeg2-ac3.vob", MPEG_PS),
];

const MPEG_PS: &str = "MPEG program stream refused: the ffmpeg-bridge format whitelist \
     names `mpegps`, but FFmpeg calls the demuxer `mpeg`";

/// What FFmpeg's log says about one file.
#[derive(Debug, Default)]
struct Log {
    /// Decoders FFmpeg refused because they are not on the whitelist.
    refused: BTreeSet<String>,
    /// The demuxer was refused.
    format_refused: bool,
}

/// The child: decodes every video file with FFmpeg's errors on standard error.
#[test]
fn ffmpeg_log_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let Some(corpus) = Corpus::load("video (child)") else {
        return;
    };
    helikodu_ffmpeg_bridge::set_log_level(helikodu_ffmpeg_bridge::LogLevel::Error);
    for entry in corpus.set("video") {
        eprintln!("{MARK}{}", entry.path);
        let _ = decoding::decode(&corpus.path(&entry));
    }
    eprintln!("{MARK}end");
}

/// Runs [`ffmpeg_log_child`] and reads its log: what was refused per file, and
/// the codec whitelist FFmpeg quoted.
fn ffmpeg_log() -> (BTreeMap<String, Log>, BTreeSet<String>) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["video::ffmpeg_log_child", "--exact", "--nocapture"])
        .env(CHILD, "1")
        .output()
        .expect("the test program runs again");
    assert!(output.status.success(), "the child failed: {output:?}");
    let text = String::from_utf8_lossy(&output.stderr);
    let mut logs: BTreeMap<String, Log> = BTreeMap::new();
    let mut whitelist = BTreeSet::new();
    let mut current = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix(MARK) {
            current = Some(path.to_owned());
            continue;
        }
        let Some(path) = &current else { continue };
        let log = logs.entry(path.clone()).or_default();
        if line.contains("Format not on whitelist") {
            log.format_refused = true;
        }
        // "[h264 @ 0x...] Codec (h264) not on whitelist 'aac,ac3,...'"
        if let Some((_, rest)) = line.split_once("Codec (")
            && let Some((codec, rest)) = rest.split_once(") not on whitelist '")
        {
            log.refused.insert(codec.to_owned());
            if let Some((list, _)) = rest.split_once('\'') {
                whitelist.extend(list.split(',').map(str::to_owned));
            }
        }
    }
    (logs, whitelist)
}

/// The video decoders of the FFmpeg build, from `ffmpeg -decoders`, if installed.
fn video_decoders() -> Option<BTreeSet<String>> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-decoders"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    // Lines such as " V....D h264    H.264 / AVC ..." after the " ------" rule.
    let list = text
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("---"))
        .skip(1)
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            let flags = words.next()?;
            let name = words.next()?;
            flags.starts_with('V').then(|| name.to_owned())
        })
        .collect();
    Some(list)
}

/// Plays every file through the transport; the end reason of each.
fn play_all(corpus: &Corpus, files: &[Entry]) -> BTreeMap<String, TrackEndReason> {
    let mut session = Session::new(null_config());
    let items: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(i, f)| item(i as u64 + 1, &corpus.path(f)))
        .collect();
    session.play_queue(&items);
    session.wait_state(TransportState::Stopped);
    let mut ends = BTreeMap::new();
    for event in &session.log {
        if let EngineEvent::TrackEnded { item, reason } = event {
            let index = usize::try_from(item.get() - 1).unwrap();
            ends.insert(files[index].path.clone(), *reason);
        }
    }
    // An item that fails before it starts is skipped: only the `Error` names it.
    for event in &session.log {
        if let EngineEvent::Error {
            item: Some(id),
            error: EngineError::Unplayable { .. },
        } = event
        {
            let index = usize::try_from(id.get() - 1).unwrap();
            ends.entry(files[index].path.clone())
                .or_insert(TrackEndReason::Failed);
        }
    }
    ends
}

#[test]
fn video_files_play_their_audio_only() {
    let Some(corpus) = Corpus::load("video") else {
        return;
    };
    let files = corpus.set("video");
    let (logs, whitelist) = ffmpeg_log();
    let ends = play_all(&corpus, &files);
    let mut table = Table::new(
        "video",
        &[
            "file", "streams", "codec", "rate", "ch", "frames", "played", "refused",
        ],
    );
    for entry in &files {
        let expect = entry.expect();
        let audio = expect.audio_streams.unwrap_or(0);
        let video = expect.video_streams.unwrap_or(0);
        let log = logs.get(&entry.path);
        let refused: Vec<&str> = log
            .map(|l| l.refused.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let played = ends.get(&entry.path).copied();
        let mut cells = vec![entry.display(), format!("{audio} a, {video} v")];
        let mut problems = Vec::new();
        // The streams as the bridge sees them; with several audio streams the
        // engine plays the default one, not the first.
        let mut expected_codec = expect.codec.clone();
        if let Ok(file) = MediaFile::open(corpus.path(entry)) {
            let count =
                |kind: StreamKind| file.streams().iter().filter(|s| s.kind == kind).count() as u32;
            if (count(StreamKind::Audio), count(StreamKind::Video)) != (audio, video) {
                problems.push(format!(
                    "the bridge sees {} audio and {} video streams",
                    count(StreamKind::Audio),
                    count(StreamKind::Video)
                ));
            }
            if audio > 1 {
                expected_codec = file
                    .audio_streams()
                    .find(|s| s.disposition.is_default())
                    .map(|s| s.codec.clone());
            }
        }
        match decoding::decode(&corpus.path(entry)) {
            Ok(decoded) => {
                cells.extend([
                    decoded.codec.clone(),
                    decoded.rate.to_string(),
                    decoded.channels.to_string(),
                    decoded.frames().to_string(),
                ]);
                if audio == 0 {
                    problems.push("a file without audio opened".to_owned());
                }
                if expected_codec
                    .as_deref()
                    .is_some_and(|c| c != decoded.codec)
                {
                    problems.push(format!(
                        "decodes {} instead of {:?}",
                        decoded.codec, expected_codec
                    ));
                }
                if expect.sample_rate.is_some_and(|r| r != decoded.rate)
                    || expect
                        .channels
                        .is_some_and(|c| usize::from(c) != decoded.channels)
                {
                    problems.push("rate or channels differ from the manifest".into());
                }
                if let Some(seconds) = corpus.source_seconds(entry) {
                    let want = seconds * f64::from(decoded.rate);
                    if (decoded.frames() as f64 - want).abs() > want * 0.05 + 4096.0 {
                        problems.push(format!(
                            "{} frames, about {want:.0} expected",
                            decoded.frames()
                        ));
                    }
                }
                if played != Some(TrackEndReason::Finished) {
                    problems.push(format!("the transport ended it with {played:?}"));
                }
            }
            Err(error) => {
                cells.extend(["-", "-", "-", "-"].map(String::from));
                if audio > 0 {
                    problems.push(format!("does not open: {error}"));
                } else if !matches!(error.to_engine_error(), EngineError::Unplayable { .. }) {
                    problems.push(format!("not reported unplayable: {error}"));
                }
                if played != Some(TrackEndReason::Failed) {
                    problems.push(format!("the transport ended it with {played:?}"));
                }
            }
        }
        // Every video stream FFmpeg looked at was refused by the whitelist.
        let format_refused = log.is_some_and(|l| l.format_refused);
        if video > 0 && !format_refused && refused.is_empty() {
            problems.push("no refused video decoder in FFmpeg's log".into());
        }
        cells.push(format!("{played:?}").replace("Some(", "").replace(')', ""));
        cells.push(if format_refused {
            "(format)".into()
        } else {
            refused.join(",")
        });
        let gap = KNOWN_GAPS.iter().find(|(p, _)| *p == entry.path);
        match gap {
            Some((_, reason)) if !problems.is_empty() && !strict() => table.gap(cells, reason),
            _ => table.row(cells, (!problems.is_empty()).then(|| problems.join("; "))),
        }
    }
    // The whitelist FFmpeg enforced names no video decoder.
    assert!(
        !whitelist.is_empty(),
        "FFmpeg's log quoted no codec whitelist"
    );
    match video_decoders() {
        Some(video) => {
            let leaked: Vec<_> = whitelist.intersection(&video).collect();
            assert!(
                leaked.is_empty(),
                "video decoders on the whitelist: {leaked:?}"
            );
            println!(
                "codec whitelist: {} decoders, none of the build's {} video decoders",
                whitelist.len(),
                video.len()
            );
        }
        None => eprintln!("SKIPPED: no `ffmpeg` tool; the whitelist was not checked against it"),
    }
    table.finish();
}
