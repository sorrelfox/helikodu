// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! "Verify files": decoding files from start to end to find those that do not
//! decode cleanly. Nothing is changed.
//!
//! A file is checked for:
//!
//! - opening at all (a file that no decoder opens "could not be read");
//! - parts that could not be read or decoded and were skipped, with the place of
//!   the first;
//! - decoding that fails for good before the end, with the place where it stopped;
//! - a length that differs from the one the file states: by more than a twentieth
//!   of a second for lossless formats, by more than a second and 2 % for lossy
//!   ones, whose stated lengths are often estimates;
//! - for FLAC, the MD5 checksum of the audio that the file stores, compared with
//!   that of the decoded audio.
//!
//! Files are checked by a few threads at a lowered priority.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use helikodu_engine::decode::{BlockSamples, DecodeError, DecoderRegistry, PcmBlock};
use helikodu_model::library::MediaSource;

mod md5;

/// The nice value of the checking threads: below interactive work.
const NICE: i32 = 10;

/// The codecs whose stated length is exact.
const LOSSLESS: &[&str] = &[
    "flac",
    "alac",
    "wavpack",
    "ape",
    "tta",
    "tak",
    "shorten",
    "mlp",
    "truehd",
    "dsd",
    "dsd_lsbf",
    "dsd_msbf",
    "dsd_lsbf_planar",
    "dsd_msbf_planar",
    "wmalossless",
];

/// What a file is, as the check saw it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerifiedFormat {
    /// The codec, such as `flac`.
    pub codec: String,
    /// The significant bits of lossless audio, when stated.
    pub bits: Option<u32>,
    /// The sample rate in Hz.
    pub rate: Option<u32>,
}

/// Something wrong with a file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum VerifyProblem {
    /// Parts could not be read or decoded and were skipped: how many, and where
    /// the first was.
    Skipped {
        /// How many.
        count: u64,
        /// Where the first was.
        at: Option<Duration>,
    },
    /// Decoding stopped for good before the end.
    Stopped {
        /// Where.
        at: Duration,
        /// What the decoder said.
        reason: String,
    },
    /// The audio is shorter than the length the file states.
    Shorter {
        /// The length the file states.
        stated: Duration,
        /// The length decoded.
        decoded: Duration,
    },
    /// The audio is longer than the length the file states.
    Longer {
        /// The length the file states.
        stated: Duration,
        /// The length decoded.
        decoded: Duration,
    },
    /// The audio does not match the checksum stored in the file.
    ChecksumMismatch,
}

/// The result of checking one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// The file.
    pub path: PathBuf,
    /// Its format, once it was opened.
    pub format: Option<VerifiedFormat>,
    /// Why it could not be opened, if it could not.
    pub unreadable: Option<String>,
    /// What is wrong with it; empty for a clean file.
    pub problems: Vec<VerifyProblem>,
}

impl VerifyReport {
    /// Whether the file decoded cleanly.
    pub fn is_clean(&self) -> bool {
        self.unreadable.is_none() && self.problems.is_empty()
    }
}

fn frames_to_duration(frames: u64, rate: u32) -> Duration {
    if rate == 0 {
        return Duration::ZERO;
    }
    Duration::from_secs_f64(frames as f64 / f64::from(rate))
}

/// Checks the file at `path`. `None` when `cancelled` said so before the end.
pub fn verify_file(
    registry: &DecoderRegistry,
    path: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Option<VerifyReport> {
    let mut report = VerifyReport {
        path: path.to_path_buf(),
        format: None,
        unreadable: None,
        problems: Vec::new(),
    };
    let mut decoder = match registry.open(&MediaSource::file(path)) {
        Ok(decoder) => decoder,
        Err(error) => {
            report.unreadable = Some(error.to_string());
            return Some(report);
        }
    };
    let spec = decoder.spec();
    let rate = spec.rate.hz();
    let info = decoder.info().clone();
    report.format = Some(VerifiedFormat {
        codec: info.codec.clone(),
        bits: info.bits_per_sample,
        rate: Some(rate),
    });
    let stated = decoder.duration();
    let flac = info.codec == "flac" && info.container.contains("flac");
    let stored_md5 = if flac { stored_flac_md5(path) } else { None };
    let mut hasher = stored_md5.map(|(_, bits)| (md5::Md5::new(), bits));
    let mut block = PcmBlock::new();
    let mut frames: u64 = 0;
    loop {
        if cancelled() {
            return None;
        }
        match decoder.read(&mut block) {
            Ok(true) => {
                frames += block.frames() as u64;
                if let Some((md5, bits)) = hasher.as_mut()
                    && !feed(md5, block.samples(), *bits)
                {
                    // Not samples the checksum can be compared with.
                    hasher = None;
                }
            }
            Ok(false) => break,
            Err(error) => {
                report.problems.push(VerifyProblem::Stopped {
                    at: frames_to_duration(frames, rate),
                    reason: decode_reason(&error),
                });
                hasher = None;
                break;
            }
        }
    }
    let (skipped, first) = decoder.skipped_errors();
    if skipped > 0 {
        report.problems.insert(
            0,
            VerifyProblem::Skipped {
                count: skipped,
                at: first.map(|f| frames_to_duration(f, rate)),
            },
        );
    }
    let stopped = report
        .problems
        .iter()
        .any(|p| matches!(p, VerifyProblem::Stopped { .. }));
    if let Some(stated) = stated
        && !stopped
    {
        let stated = stated.as_duration();
        let decoded = frames_to_duration(frames, rate);
        let lossless = LOSSLESS.contains(&info.codec.as_str())
            || info.codec.starts_with("pcm_")
            || info.codec.starts_with("dsd");
        let tolerance = if lossless {
            Duration::from_millis(50)
        } else {
            Duration::from_secs(1).max(stated / 50)
        };
        if decoded + tolerance < stated {
            report
                .problems
                .push(VerifyProblem::Shorter { stated, decoded });
        } else if decoded > stated + tolerance {
            report
                .problems
                .push(VerifyProblem::Longer { stated, decoded });
        }
    }
    if let (Some((stored, _)), Some((hasher, _))) = (stored_md5, hasher)
        && skipped == 0
        && !stopped
        && hasher.finish() != stored
    {
        report.problems.push(VerifyProblem::ChecksumMismatch);
    }
    Some(report)
}

fn decode_reason(error: &DecodeError) -> String {
    let text = error.to_string();
    if text.is_empty() {
        "decoding failed".to_owned()
    } else {
        text
    }
}

/// Adds the samples of a block to the checksum as FLAC computes it: each sample
/// in as many little-endian bytes as its significant bits need. Returns `false`
/// for samples that are not integers.
fn feed(hasher: &mut md5::Md5, samples: &BlockSamples, bits: u32) -> bool {
    let bytes = bits.div_ceil(8) as usize;
    let mut buffer = Vec::with_capacity(samples.len() * bytes);
    match samples {
        BlockSamples::S16(v) => {
            let shift = 16_u32.saturating_sub(bits);
            for &s in v {
                let value = i32::from(s) >> shift;
                buffer.extend_from_slice(&value.to_le_bytes()[..bytes]);
            }
        }
        BlockSamples::S32(v) => {
            let shift = 32_u32.saturating_sub(bits);
            for &s in v {
                let value = s >> shift;
                buffer.extend_from_slice(&value.to_le_bytes()[..bytes]);
            }
        }
        BlockSamples::U8(v) => {
            for &s in v {
                let value = i32::from(s) - 128;
                buffer.extend_from_slice(&value.to_le_bytes()[..bytes]);
            }
        }
        BlockSamples::F32(_) => return false,
    }
    hasher.update(&buffer);
    true
}

/// The MD5 of the audio stored in a FLAC file's STREAMINFO, with the bits per
/// sample; `None` when the file has none (all zeros) or cannot be read.
fn stored_flac_md5(path: &Path) -> Option<([u8; 16], u32)> {
    let mut file = File::open(path).ok()?;
    let mut head = [0_u8; 10];
    file.read_exact(&mut head).ok()?;
    let mut skip = 0_u64;
    // An ID3v2 tag before the stream.
    if &head[..3] == b"ID3" {
        let size = head[6..10]
            .iter()
            .fold(0_u64, |acc, b| (acc << 7) | u64::from(b & 0x7f));
        let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
        skip = 10 + size + footer;
    }
    let mut file = File::open(path).ok()?;
    std::io::copy(&mut (&mut file).take(skip), &mut std::io::sink()).ok()?;
    let mut block = [0_u8; 4 + 4 + 34];
    file.read_exact(&mut block).ok()?;
    if &block[..4] != b"fLaC" || block[4] & 0x7f != 0 {
        return None;
    }
    let info = &block[8..];
    let bits = (u32::from(info[12] & 0x01) << 4 | u32::from(info[13] >> 4)) + 1;
    let mut md5 = [0_u8; 16];
    md5.copy_from_slice(&info[18..34]);
    (md5 != [0; 16]).then_some((md5, bits))
}

/// Checks `paths` with up to `threads` threads (0: half the processors), handing
/// each report to `found` as it is ready, with its index. Stops early when
/// `cancelled` says so.
pub fn verify_files(
    paths: &[PathBuf],
    threads: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
    found: &(dyn Fn(usize, VerifyReport) + Sync),
) {
    let registry = DecoderRegistry::with_default_backends();
    let threads = match threads {
        0 => thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1)),
        n => n,
    }
    .min(paths.len().max(1));
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        for index in 0..threads {
            let (registry, next) = (&registry, &next);
            let _ = thread::Builder::new()
                .name(format!("helikodu-verify-{index}"))
                .spawn_scoped(scope, move || {
                    let thread = rustix::thread::gettid();
                    let _ = rustix::process::setpriority_process(Some(thread), NICE);
                    loop {
                        if cancelled() {
                            break;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = paths.get(i) else { break };
                        if let Some(report) = verify_file(registry, path, cancelled) {
                            found(i, report);
                        }
                    }
                });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_could_not_be_read() {
        let registry = DecoderRegistry::with_default_backends();
        let report = verify_file(
            &registry,
            Path::new("/nonexistent/helikodu/none.flac"),
            &|| false,
        )
        .expect("a report");
        assert!(report.unreadable.is_some());
        assert!(!report.is_clean());
    }

    #[test]
    fn checksum_bytes_follow_the_significant_bits() {
        let mut a = md5::Md5::new();
        assert!(feed(
            &mut a,
            &BlockSamples::S32(vec![0x0012_3400, -256]),
            24
        ));
        let mut b = md5::Md5::new();
        b.update(&[0x34, 0x12, 0x00, 0xff, 0xff, 0xff]);
        assert_eq!(a.finish(), b.finish());
        assert!(!feed(
            &mut md5::Md5::new(),
            &BlockSamples::F32(vec![0.0]),
            16
        ));
    }
}

#[cfg(test)]
mod files {
    #![allow(clippy::unwrap_used)]

    use std::process::Command;

    use super::*;

    /// A FLAC file of two seconds made by the `ffmpeg` tool, or `None` without it.
    fn flac(name: &str) -> Option<PathBuf> {
        let dir = std::env::temp_dir().join(format!("helikodu-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let made = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg("sine=frequency=440:duration=2:sample_rate=44100")
            .args(["-ac", "2", "-c:a", "flac", "-f", "flac"])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        if !made {
            eprintln!("SKIPPED: ffmpeg is not installed");
        }
        made.then_some(path)
    }

    #[test]
    fn a_clean_file_has_no_problems() {
        let Some(path) = flac("clean.flac") else {
            return;
        };
        let registry = DecoderRegistry::with_default_backends();
        let report = verify_file(&registry, &path, &|| false).unwrap();
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(report.format.unwrap().codec, "flac");
    }

    #[test]
    fn changed_bytes_in_the_audio_are_found() {
        let Some(path) = flac("damaged.flac") else {
            return;
        };
        let mut bytes = std::fs::read(&path).unwrap();
        let middle = bytes.len() / 2;
        for b in &mut bytes[middle..middle + 64] {
            *b ^= 0x5a;
        }
        std::fs::write(&path, &bytes).unwrap();
        let registry = DecoderRegistry::with_default_backends();
        let report = verify_file(&registry, &path, &|| false).unwrap();
        assert!(!report.is_clean(), "{report:?}");
    }

    #[test]
    fn a_cut_file_is_shorter_than_it_says() {
        let Some(path) = flac("cut.flac") else { return };
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        let registry = DecoderRegistry::with_default_backends();
        let report = verify_file(&registry, &path, &|| false).unwrap();
        assert!(
            report.problems.iter().any(|p| matches!(
                p,
                VerifyProblem::Shorter { .. } | VerifyProblem::Stopped { .. }
            )),
            "{report:?}"
        );
    }

    #[test]
    fn many_files_are_checked_in_parallel() {
        let (Some(a), Some(b)) = (flac("p1.flac"), flac("p2.flac")) else {
            return;
        };
        let found = std::sync::Mutex::new(Vec::new());
        verify_files(&[a, b], 2, &|| false, &|i, r| {
            found.lock().unwrap().push((i, r.is_clean()))
        });
        let mut found = found.into_inner().unwrap();
        found.sort();
        assert_eq!(found, [(0, true), (1, true)]);
    }
}
