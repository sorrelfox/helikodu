// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! An invented track analysis for the sample tracks, for the File info tab:
//! one track in seven could not be read, one in seven is not analysed yet, the
//! others have an analysis. The sound starts 0.42 s in and ends 1.9 s before
//! the end; every third track of over 200 s has a silence from 2:10 to 2:22,
//! every fifth five silences; every fourth track ends without a fade-out, the
//! others fade out over their last 15 s of sound.

use std::sync::Arc;
use std::time::Duration;

use helikodu_model::analysis::{LoudnessEnvelope, SilentSpan, TrackAnalysis};

use crate::backend::TrackKey;
use crate::backend::services::AnalysisFacts;

const RATE: u32 = 44_100;

fn frames(seconds: f64) -> u64 {
    // Seconds of a track are short and never negative.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames = (seconds * f64::from(RATE)).round().max(0.0) as u64;
    frames
}

/// The analysis of the sample track `key` of `length`.
pub(super) fn sample_analysis(key: TrackKey, length: Option<Duration>) -> AnalysisFacts {
    match key.0 % 7 {
        0 => return AnalysisFacts::Failed,
        1 => return AnalysisFacts::NotYet,
        _ => {}
    }
    let seconds = length.map_or(240.0, |l| l.as_secs_f64()).max(20.0);
    let audio = frames(0.42)..frames(seconds - 1.9);
    let mut silences = Vec::new();
    if key.0.is_multiple_of(5) && seconds > 120.0 {
        for start in [20.0, 40.0, 60.0, 80.0, 100.0] {
            silences.push(SilentSpan {
                start: frames(start),
                end: frames(start + 3.0),
            });
        }
    } else if key.0.is_multiple_of(3) && seconds > 200.0 {
        silences.push(SilentSpan {
            start: frames(130.0),
            end: frames(142.0),
        });
    }
    let fade = if key.0.is_multiple_of(4) { 0.5 } else { 15.0 };
    let fade_out_start = audio.end.saturating_sub(frames(fade));
    let end_seconds = seconds - 1.9;
    let steps = seconds.ceil() as usize;
    let lufs: Vec<f64> = (0..steps)
        .map(|i| {
            let t = i as f64 + 0.5;
            let frame = frames(t);
            if t < 0.42
                || t > end_seconds
                || silences.iter().any(|s| frame >= s.start && frame < s.end)
            {
                return f64::NEG_INFINITY;
            }
            let x = t / seconds;
            let body = -16.0
                + 5.0 * (std::f64::consts::PI * x).sin() * (0.7 + 0.3 * (x * 9.0 + 0.6).sin())
                + 3.0 * ((i as f64 * 1.3).sin() * (i as f64 * 0.41).cos()).abs();
            let to_end = end_seconds - t;
            if fade > 1.0 && to_end < fade {
                body - 30.0 * (1.0 - to_end / fade)
            } else {
                body
            }
        })
        .collect();
    AnalysisFacts::Analysed(Arc::new(TrackAnalysis {
        rate: RATE,
        frames: frames(seconds),
        audio: Some(audio.clone()),
        silences,
        integrated_lufs: Some(-14.0),
        head_lufs: Some(-16.0),
        tail_lufs: Some(-24.0),
        fade_in_end: Some(audio.start + frames(0.5)),
        fade_out_start: Some(fade_out_start),
        envelope: LoudnessEnvelope::from_lufs(1, lufs),
    }))
}
