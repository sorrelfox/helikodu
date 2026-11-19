// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The analysis of a track, as the engine makes it, the library keeps it and
//! playback will read it: where its sound starts and ends, its silent stretches,
//! and a coarse loudness envelope with the details transitions need.
//!
//! Positions are frames from the start of the track (of its section, for a track
//! of a CUE sheet) at [`TrackAnalysis::rate`].
//!
//! The envelope and the silent stretches have compact byte forms for storage
//! ([`LoudnessEnvelope::to_bytes`], [`silences_to_bytes`]):
//!
//! - Envelope: byte 0 is the format ([`ENVELOPE_FORMAT`], 1), byte 1 the step in
//!   seconds (1 to 255), then one byte per step: 0 for no sound (below
//!   −119.5 LUFS, or digital silence), otherwise the loudness of the step as
//!   `LUFS = value / 2 − 120`, so 0.5 LU apart from −119.5 to +7.5 LUFS.
//! - Silent stretches: 16 bytes each, the first and the end frame (exclusive) as
//!   little-endian unsigned 64-bit integers.

use std::ops::Range;

/// The format of [`LoudnessEnvelope::to_bytes`].
pub const ENVELOPE_FORMAT: u8 = 1;

/// The loudness of the lowest non-zero envelope value, in LUFS.
const ENVELOPE_FLOOR_LUFS: f64 = -120.0;

/// A silent stretch inside a track: frames `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SilentSpan {
    /// The first silent frame.
    pub start: u64,
    /// The frame after the last silent one.
    pub end: u64,
}

impl SilentSpan {
    /// The number of frames.
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    /// Whether it holds no frames.
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// The loudness of a track at a fixed step, one byte per step; see the module
/// documentation for the byte form.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoudnessEnvelope {
    step_secs: u8,
    values: Vec<u8>,
}

impl LoudnessEnvelope {
    /// An envelope of `step_secs` seconds per value (at least 1) from the
    /// loudness of each step in LUFS (`-inf` or `NaN` for no sound).
    pub fn from_lufs(step_secs: u8, lufs: impl IntoIterator<Item = f64>) -> Self {
        Self {
            step_secs: step_secs.max(1),
            values: lufs.into_iter().map(quantize).collect(),
        }
    }

    /// The step in seconds.
    pub fn step_secs(&self) -> u8 {
        self.step_secs.max(1)
    }

    /// The number of steps.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether it has no steps.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The stored values, one byte per step.
    pub fn values(&self) -> &[u8] {
        &self.values
    }

    /// The loudness of step `index` in LUFS; `None` for no sound or past the end.
    pub fn lufs(&self, index: usize) -> Option<f64> {
        match self.values.get(index) {
            None | Some(0) => None,
            Some(&value) => Some(f64::from(value) / 2.0 + ENVELOPE_FLOOR_LUFS),
        }
    }

    /// The byte form.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(2 + self.values.len());
        bytes.push(ENVELOPE_FORMAT);
        bytes.push(self.step_secs());
        bytes.extend_from_slice(&self.values);
        bytes
    }

    /// Reads the byte form; `None` for another format or a step of 0.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [ENVELOPE_FORMAT, step, values @ ..] if *step > 0 => Some(Self {
                step_secs: *step,
                values: values.to_vec(),
            }),
            _ => None,
        }
    }
}

/// The byte value of a loudness in LUFS.
fn quantize(lufs: f64) -> u8 {
    if !lufs.is_finite() {
        return if lufs == f64::INFINITY { u8::MAX } else { 0 };
    }
    let value = ((lufs - ENVELOPE_FLOOR_LUFS) * 2.0)
        .round()
        .clamp(0.0, 255.0);
    // The value is whole and within 0..=255.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let value = value as u8;
    value
}

/// The byte form of silent stretches.
pub fn silences_to_bytes(silences: &[SilentSpan]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(silences.len() * 16);
    for span in silences {
        bytes.extend_from_slice(&span.start.to_le_bytes());
        bytes.extend_from_slice(&span.end.to_le_bytes());
    }
    bytes
}

/// Reads the byte form of silent stretches; a trailing partial entry is ignored.
pub fn silences_from_bytes(bytes: &[u8]) -> Vec<SilentSpan> {
    bytes
        .as_chunks::<16>()
        .0
        .iter()
        .map(|entry| {
            let (start, end) = entry.split_at(8);
            let word = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
            SilentSpan {
                start: word(start),
                end: word(end),
            }
        })
        .collect()
}

/// The analysis of one track.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrackAnalysis {
    /// The sample rate the positions count in, in Hz.
    pub rate: u32,
    /// The length of the track in frames.
    pub frames: u64,
    /// The frames with audible sound, from the first to the last; `None` when the
    /// track is silent throughout. Before it is the leading silence, after it the
    /// trailing silence.
    pub audio: Option<Range<u64>>,
    /// The silent stretches inside `audio` that are long enough to matter, in
    /// order.
    pub silences: Vec<SilentSpan>,
    /// The integrated loudness of the track in LUFS (EBU R128); `None` for silence.
    pub integrated_lufs: Option<f64>,
    /// The loudness of the first seconds of sound, in LUFS.
    pub head_lufs: Option<f64>,
    /// The loudness of the last seconds of sound, in LUFS.
    pub tail_lufs: Option<f64>,
    /// Where the beginning reaches the track's body level (the end of a fade-in;
    /// near the start of the sound when there is none).
    pub fade_in_end: Option<u64>,
    /// Where the ending falls away from the track's body level for good (the start
    /// of a fade-out; near the end of the sound when there is none).
    pub fade_out_start: Option<u64>,
    /// The loudness over the whole track.
    pub envelope: LoudnessEnvelope,
}

impl TrackAnalysis {
    /// The frames of silence before the sound (all of them for a silent track).
    pub fn leading_silence(&self) -> u64 {
        self.audio.as_ref().map_or(self.frames, |audio| audio.start)
    }

    /// The frames of silence after the sound (none for a silent track).
    pub fn trailing_silence(&self) -> u64 {
        self.audio
            .as_ref()
            .map_or(0, |audio| self.frames.saturating_sub(audio.end))
    }

    /// Whether the track has no audible sound.
    pub fn is_silent(&self) -> bool {
        self.audio.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_round_trip_in_half_lu() {
        let envelope = LoudnessEnvelope::from_lufs(
            1,
            [
                f64::NEG_INFINITY,
                -23.0,
                -23.2,
                -200.0,
                12.0,
                f64::NAN,
                -119.5,
            ],
        );
        let bytes = envelope.to_bytes();
        assert_eq!(bytes.len(), 2 + 7);
        assert_eq!(&bytes[..2], &[ENVELOPE_FORMAT, 1]);
        let back = LoudnessEnvelope::from_bytes(&bytes).unwrap();
        assert_eq!(back, envelope);
        assert_eq!(back.lufs(0), None);
        assert_eq!(back.lufs(1), Some(-23.0));
        assert_eq!(back.lufs(2), Some(-23.0));
        assert_eq!(back.lufs(3), None, "below the floor is no sound");
        assert_eq!(back.lufs(4), Some(7.5), "clamped at the top");
        assert_eq!(back.lufs(5), None);
        assert_eq!(back.lufs(6), Some(-119.5));
        assert_eq!(back.lufs(7), None);
        assert_eq!(LoudnessEnvelope::from_bytes(&[2, 1, 5]), None);
        assert_eq!(LoudnessEnvelope::from_bytes(&[1, 0]), None);
    }

    #[test]
    fn silences_round_trip() {
        let spans = [
            SilentSpan { start: 10, end: 20 },
            SilentSpan {
                start: 1 << 40,
                end: (1 << 40) + 5,
            },
        ];
        let bytes = silences_to_bytes(&spans);
        assert_eq!(bytes.len(), 32);
        assert_eq!(silences_from_bytes(&bytes), spans);
        assert_eq!(spans[1].len(), 5);
    }

    #[test]
    fn leading_and_trailing_silence() {
        let analysis = TrackAnalysis {
            rate: 48_000,
            frames: 1000,
            audio: Some(100..900),
            ..TrackAnalysis::default()
        };
        assert_eq!(analysis.leading_silence(), 100);
        assert_eq!(analysis.trailing_silence(), 100);
        let silent = TrackAnalysis {
            frames: 1000,
            ..TrackAnalysis::default()
        };
        assert!(silent.is_silent());
        assert_eq!(silent.leading_silence(), 1000);
        assert_eq!(silent.trailing_silence(), 0);
    }
}
