// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Seeks across the formats set: lossless formats land on the exact frame (the
//! samples after the seek are those of a straight decode at that frame); lossy ones
//! land aligned (once the decoder has settled, the samples after the seek match a
//! straight decode best at that frame, not at a neighbouring one).
//!
//! Where the FFmpeg bridge has no exact anchor and the container's time stamps are
//! coarser than a frame (Vorbis in WebM: milliseconds), it states how far a seek
//! may land from its target (`AudioDecoder::seek_precision`); such a file is
//! checked against that guarantee instead. Lossless files must always be exact.
//! Files whose decoder output depends on its running state beyond any warm-up
//! (Codec 2) are checked for position and length only (`NOT_COMPARABLE`).
//!
//! Lossy decoders that overlap frames or adapt a predictor start from a different
//! state after a seek than in a straight decode, so their first frames differ. The
//! table reports this warm-up: the frames until the output stays within -20 dB
//! of the straight decode.

use std::path::Path;

use helikodu_engine::decode::{Decoder, PcmBlock};
use helikodu_ffmpeg_bridge::{MediaFile, StreamChoice};
use helikodu_model::audio::{MediaTime, SampleRate};

use crate::decoding::{self, Decoded};
use crate::formats::{known_gap, samples, strict, unsupported_by_design};
use crate::manifest::{Corpus, is_lossless};
use crate::report::Table;

/// Seeks known to miss in the FFmpeg bridge, with the reason. They are reported
/// as gaps, not failures, unless `HELIKODU_CORPUS_STRICT` is set.
const SEEK_GAPS: &[(&str, &str)] = &[];

/// Lossy files whose decode after a seek cannot be compared with the straight
/// decode sample by sample, with the reason. Their seeks are checked for the
/// reported position and the length of what follows only.
const NOT_COMPARABLE: &[(&str, &str)] = &[(
    "formats/speech/codec2.c2",
    "Codec 2 synthesises phases from the decoder's running state (random for \
     unvoiced speech), so the samples after a seek never match a straight decode",
)];

/// Frames compared after each seek. Lossy alignment is judged on the second half.
const WINDOW: usize = 8192;
/// Frames per step of the warm-up measurement.
const WARM_UP_BLOCK: usize = 64;
/// The error, relative to the signal, below which a lossy decoder has settled.
const SETTLED_DB: f64 = -20.0;
/// How far around the target an alignment is searched.
const SEARCH: i64 = 48;
/// How far a misaligned seek is searched, to report how far off it is.
const WIDE_SEARCH: i64 = 16_384;
/// Frames that rank the candidates of the wide search.
const PROBE: usize = 64;

/// The frames to seek to: the start, a few odd frames, and towards the end (not
/// into the last tenth, where formats without padding information decode the
/// encoder's padding); in an order that seeks backwards as well as forwards.
fn targets(frames: usize) -> Vec<usize> {
    let mut out = vec![frames / 2 + 13, 7, frames / 10 * 9 + 3, frames / 3 + 1, 0];
    out.retain(|&t| t < frames);
    out.dedup();
    out
}

/// How far the FFmpeg bridge says a seek in `path` may land from its target.
fn seek_precision(path: &Path) -> Option<u64> {
    let decoder = MediaFile::open(path)
        .ok()?
        .into_decoder(StreamChoice::Auto)
        .ok()?;
    Some(decoder.seek_precision())
}

/// The next `frames` frames of `decoder`.
fn next(decoder: &mut dyn Decoder, frames: usize, channels: usize) -> Vec<f32> {
    let mut block = PcmBlock::new();
    let mut out = Vec::new();
    while out.len() < frames * channels && decoder.read(&mut block).unwrap_or(false) {
        block.append_f32(&mut out);
    }
    out.truncate(frames * channels);
    out
}

/// The frames after which `got` stays within [`SETTLED_DB`] of `want`, in steps
/// of [`WARM_UP_BLOCK`]; `None` if it never does.
fn settled_after(got: &[f32], want: &[f32], channels: usize) -> Option<usize> {
    let block = WARM_UP_BLOCK * channels;
    let settled: Vec<bool> = got
        .chunks_exact(block)
        .zip(want.chunks_exact(block))
        .map(|(g, w)| {
            let error: f64 = g.iter().zip(w).map(|(a, b)| f64::from(a - b).powi(2)).sum();
            let energy: f64 = w.iter().map(|v| f64::from(*v).powi(2)).sum();
            error <= energy * 10f64.powf(SETTLED_DB / 10.0)
        })
        .collect();
    let first_bad_from_end = settled.iter().rposition(|ok| !ok);
    match first_bad_from_end {
        None => Some(0),
        Some(i) if i + 1 < settled.len() => Some((i + 1) * WARM_UP_BLOCK),
        Some(_) => None,
    }
}

/// The offset from `target` (within `range`) at which `got` matches the straight
/// decode best, and its squared error.
fn best_offset(all: &Decoded, got: &[f32], target: usize, range: i64) -> (i64, f64) {
    (-range..=range)
        .filter_map(|offset| error_at(all, got, target as i64 + offset).map(|e| (offset, e)))
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.abs().cmp(&b.0.abs())))
        .unwrap_or((i64::MAX, f64::INFINITY))
}

/// Like [`best_offset`] over [`WIDE_SEARCH`], but quick: candidates are ranked on
/// the first [`PROBE`] frames only, and the best one is measured in full.
fn wide_offset(all: &Decoded, got: &[f32], target: usize) -> (i64, f64) {
    let probe = &got[..got.len().min(PROBE * all.channels)];
    let (offset, _) = best_offset(all, probe, target, WIDE_SEARCH);
    let error = error_at(all, got, target as i64 + offset).unwrap_or(f64::INFINITY);
    (offset, error)
}

/// The squared error of `got` against the straight decode starting at `at`.
fn error_at(all: &Decoded, got: &[f32], at: i64) -> Option<f64> {
    let start = usize::try_from(at).ok()? * all.channels;
    let end = start + got.len();
    let want = all.samples.get(start..end)?;
    Some(
        got.iter()
            .zip(want)
            .map(|(a, b)| f64::from(a - b).powi(2))
            .sum(),
    )
}

#[test]
fn seeks_are_exact_or_aligned_across_formats() {
    let Some(corpus) = Corpus::load("seeking") else {
        return;
    };
    let mut table = Table::new(
        "seeking",
        &[
            "file",
            "codec",
            "frames",
            "seeks",
            "check",
            "precision",
            "warm-up",
        ],
    );
    for entry in samples(&corpus) {
        let path = corpus.path(&entry);
        let all = match decoding::decode(&path) {
            Ok(all) => all,
            Err(error) => {
                let mut cells = vec![entry.display()];
                cells.extend(["-"; 6].map(String::from));
                if let Some(reason) = unsupported_by_design(&entry) {
                    table.unsupported(cells, reason);
                    continue;
                }
                match known_gap(&entry) {
                    Some(reason) if !strict() => table.gap(cells, reason),
                    _ => table.row(cells, Some(error.to_string())),
                }
                continue;
            }
        };
        let lossless = is_lossless(&all.codec);
        let rate = SampleRate::new(all.rate).unwrap();
        let mut decoder = decoding::open(&path).unwrap();
        let mut problems = Vec::new();
        let precision = seek_precision(&path).unwrap_or(0);
        if lossless && precision != 0 {
            problems.push(format!(
                "lossless, but the bridge only promises seeks within {precision} frames"
            ));
        }
        let comparable = !NOT_COMPARABLE.iter().any(|(p, _)| *p == entry.path);
        // How far lossy seeks may land off: the stated precision.
        let allowed = i64::try_from(precision).unwrap_or(i64::MAX);
        let mut warm_up = Some(0);
        let targets = targets(all.frames());
        for &target in &targets {
            if let Err(error) = decoder.seek(MediaTime::from_frames(target as u64, rate)) {
                problems.push(format!("seek to {target}: {error}"));
                continue;
            }
            if decoder.position() != target as u64 {
                problems.push(format!(
                    "seek to {target} reports position {}",
                    decoder.position()
                ));
            }
            let window = WINDOW.min(all.frames() - target);
            let got = next(decoder.as_mut(), window, all.channels);
            if got.len() != window * all.channels {
                problems.push(format!(
                    "seek to {target}: {} frames instead of {window}",
                    got.len() / all.channels
                ));
                continue;
            }
            let want = &all.samples[target * all.channels..(target + window) * all.channels];
            if lossless {
                if got != want {
                    let (far, error) = wide_offset(&all, &got, target);
                    problems.push(if error == 0.0 {
                        format!("seek to {target}: lands {far:+} frames off")
                    } else {
                        format!("seek to {target}: differs from the straight decode")
                    });
                }
                continue;
            }
            if !comparable {
                warm_up = None;
                continue;
            }
            // Alignment, on the second half, after the warm-up.
            let half = window / 2 * all.channels;
            let (late_got, late_target) = (&got[half..], target + window / 2);
            let at_target = error_at(&all, late_got, late_target as i64).unwrap_or(f64::INFINITY);
            let (best, best_error) = best_offset(&all, late_got, late_target, SEARCH);
            if best != 0 && best_error < at_target {
                let (far, _) = wide_offset(&all, late_got, late_target);
                if far.abs() > allowed {
                    problems.push(format!(
                        "seek to {target}: lands {far:+} frames off (allowed: {precision})"
                    ));
                }
            }
            warm_up = warm_up
                .zip(settled_after(&got, want, all.channels))
                .map(|(a, b)| a.max(b));
        }
        let check = match (lossless, comparable) {
            (true, _) => "exact",
            (false, true) => "aligned",
            (false, false) => "position",
        };
        let precision = match precision {
            0 => "exact".to_owned(),
            frames => format!("±{frames} frames"),
        };
        let warm_up = match warm_up {
            _ if lossless || !comparable => "-".into(),
            Some(frames) => format!("{frames} frames"),
            None => "not settled".into(),
        };
        let cells = vec![
            entry.display(),
            all.codec.clone(),
            all.frames().to_string(),
            targets.len().to_string(),
            check.into(),
            precision,
            warm_up,
        ];
        let gap = SEEK_GAPS.iter().find(|(path, _)| *path == entry.path);
        match gap {
            Some((_, reason)) if !problems.is_empty() && !strict() => {
                table.gap(cells, &format!("{reason} ({})", problems.join("; ")));
            }
            _ => {
                if gap.is_some() && problems.is_empty() {
                    println!("{} seeks right now: take it off SEEK_GAPS", entry.path);
                }
                table.row(cells, (!problems.is_empty()).then(|| problems.join("; ")));
            }
        }
    }
    table.finish();
}
