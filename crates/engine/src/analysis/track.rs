// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The track analysis: one decoding pass that finds where a track's sound starts
//! and ends, its silent stretches, and a coarse loudness envelope with the details
//! transitions need. The same pass can take the EBU R128 measurement of the
//! loudness scanner, so that a track is decoded only once.
//!
//! Silence is found in 10 ms windows: a window is silent when the RMS level of its
//! samples (the mean square over all channels) is below [`SILENCE_THRESHOLD_DBFS`].
//! The threshold is absolute, not relative to the track: silence is a property of
//! the signal (digital silence, dither, the noise floor of a recording), and a
//! threshold relative to the track's loudness would call the soft passages of a
//! quiet track silent. −60 dBFS sits above the dither and noise floor of almost
//! every recording (16-bit dither is near −90 dBFS, tape hiss and vinyl noise
//! between −70 and −60) and below anything meant to be heard; it is also the
//! default of the common `silencedetect` tools, so results are comparable.
//! Silent stretches inside the sound count from [`MIN_INNER_SILENCE_MS`]; shorter
//! pauses are part of the music.
//!
//! Loudness is measured K-weighted as BS.1770 defines it, in 100 ms blocks. The
//! envelope keeps the mean of each second (one byte per second, see
//! [`LoudnessEnvelope`]); a track longer than [`MAX_ENVELOPE_STEPS`] seconds gets
//! a longer step, so that no envelope is larger than about 3.6 KB. The blocks of
//! the first and the last [`EDGE_SEARCH_MS`] of sound are kept at their full
//! resolution while decoding, for:
//!
//! - the loudness of the first and the last [`EDGE_MS`] of sound;
//! - the end of a fade-in and the start of a fade-out: where a 1 s window first
//!   (last) reaches the body level of the track, its integrated loudness less
//!   [`BODY_DROP_LU`]. When the ending never reaches it within the last
//!   [`EDGE_SEARCH_MS`], the fade-out starts at the beginning of that span (and
//!   likewise for the fade-in at its end).
//!
//! The same pass collects the sound features used to find tracks that sound
//! alike (see the `features` module).

use std::collections::VecDeque;

use helikodu_model::analysis::{LoudnessEnvelope, SilentSpan, TrackAnalysis};
use helikodu_model::audio::{ChannelLayout, SampleRate};
use helikodu_model::library::MediaSource;

use super::features::{FeatureExtractor, loudness_spread};
use super::loudness::{AnalysisError, LoudnessMeter, TrackMeasurement};
use crate::decode::{DecoderRegistry, PcmBlock};

/// The version of the track analysis. It changes when a change to the analysis
/// changes its results, so that stored analyses of an older version are made
/// again. Version 2 added the sound features.
pub const ANALYSIS_VERSION: u32 = 2;

/// The RMS level below which a window is silent, in dBFS.
pub const SILENCE_THRESHOLD_DBFS: f64 = -60.0;

/// The length of the windows silence is found in.
pub const SILENCE_WINDOW_MS: u32 = 10;

/// The shortest silent stretch inside the sound that is recorded.
pub const MIN_INNER_SILENCE_MS: u32 = 2_000;

/// The most silent stretches recorded for one track (the first ones).
pub const MAX_INNER_SILENCES: usize = 64;

/// The most envelope values of a track before its step grows.
pub const MAX_ENVELOPE_STEPS: usize = 3_600;

/// The span at the start and the end of the sound whose loudness is kept.
pub const EDGE_MS: u32 = 5_000;

/// The span at the start and the end of the sound searched for fades.
pub const EDGE_SEARCH_MS: u32 = 30_000;

/// How far below the integrated loudness a 1 s window may be and still count as
/// the body of the track, in LU.
pub const BODY_DROP_LU: f64 = 3.0;

/// The loudness blocks.
const BLOCK_MS: u32 = 100;
/// Blocks per envelope step of one second.
const BLOCKS_PER_SECOND: u32 = 1000 / BLOCK_MS;
/// Blocks in the window that fades are judged by.
const FADE_WINDOW_BLOCKS: usize = BLOCKS_PER_SECOND as usize;
const EDGE_BLOCKS: usize = (EDGE_MS / BLOCK_MS) as usize;
const SEARCH_BLOCKS: usize = (EDGE_SEARCH_MS / BLOCK_MS) as usize;
/// The blocks kept at the end: twice the search span, so that a trailing silence
/// of up to that length still leaves the whole span of sound before it.
const TAIL_BLOCKS: usize = 2 * SEARCH_BLOCKS;

/// A track analysed, with the loudness measurement when it was asked for.
#[derive(Debug)]
pub struct SourceAnalysis {
    /// The analysis.
    pub analysis: TrackAnalysis,
    /// The EBU R128 measurement, when asked for.
    pub loudness: Option<TrackMeasurement>,
    /// The codec the backend decoded.
    pub codec: String,
}

/// Decodes `source` (a whole file or a section of one) to its end and analyses it,
/// with the loudness measurement of [`measure_source`](super::measure_source)
/// when `measure_loudness`.
///
/// `cancelled` is asked between decoded blocks; when it returns `true` the
/// analysis stops with [`AnalysisError::Cancelled`]. It may also wait, to slow
/// the analysis down.
pub fn analyze_source(
    registry: &DecoderRegistry,
    source: &MediaSource,
    measure_loudness: bool,
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<SourceAnalysis, AnalysisError> {
    let mut decoder = registry.open(source)?;
    let spec = decoder.spec();
    let codec = decoder.info().codec.clone();
    let mut analyzer = TrackAnalyzer::new(spec.rate, spec.layout, measure_loudness)?;
    let mut block = PcmBlock::new();
    loop {
        if cancelled() {
            return Err(AnalysisError::Cancelled);
        }
        if !decoder.read(&mut block)? {
            break;
        }
        analyzer.add_block(&block)?;
    }
    let (analysis, loudness) = analyzer.finish()?;
    let loudness = loudness.map(|mut measurement| {
        measurement.codec.clone_from(&codec);
        measurement
    });
    Ok(SourceAnalysis {
        analysis,
        loudness,
        codec,
    })
}

/// A 100 ms block of K-weighted energy.
#[derive(Debug, Clone, Copy)]
struct Block {
    start: u64,
    end: u64,
    energy: f64,
}

/// Analyses interleaved audio as it arrives; see the module documentation.
pub struct TrackAnalyzer {
    meter: LoudnessMeter,
    measure_loudness: bool,
    /// The rate the positions count in: the first one.
    rate: u32,
    /// The shape of the audio now.
    segment_rate: u32,
    layout: ChannelLayout,
    channels: usize,
    /// Where the current shape began, in frames at `rate`, and its frames since.
    segment_start: u64,
    segment_frames: u64,

    window_len: u64,
    window_fill: u64,
    window_sum: f64,
    window_start: u64,
    first_sound: Option<u64>,
    sound_end: Option<u64>,
    quiet_since: Option<u64>,
    silences: Vec<SilentSpan>,

    block_len: u64,
    block_fill: u64,
    block_start: u64,
    second_energy: f64,
    second_frames: u64,
    second_blocks: u32,
    seconds: Vec<f64>,
    head: Vec<Block>,
    tail: VecDeque<Block>,
    scratch: Vec<f32>,
    features: FeatureExtractor,
}

impl std::fmt::Debug for TrackAnalyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackAnalyzer")
            .field("rate", &self.rate)
            .field("frames", &self.position())
            .finish_non_exhaustive()
    }
}

impl TrackAnalyzer {
    /// An analyzer for audio at `rate` with the channels of `layout`, which also
    /// takes the full loudness measurement when `measure_loudness`.
    pub fn new(
        rate: SampleRate,
        layout: ChannelLayout,
        measure_loudness: bool,
    ) -> Result<Self, AnalysisError> {
        let meter = if measure_loudness {
            LoudnessMeter::new(rate, layout)?
        } else {
            LoudnessMeter::light(rate, layout)?
        };
        Ok(Self {
            meter,
            measure_loudness,
            rate: rate.hz(),
            segment_rate: rate.hz(),
            layout,
            channels: usize::from(layout.channel_count().max(1)),
            segment_start: 0,
            segment_frames: 0,
            window_len: frames_in(rate.hz(), SILENCE_WINDOW_MS),
            window_fill: 0,
            window_sum: 0.0,
            window_start: 0,
            first_sound: None,
            sound_end: None,
            quiet_since: None,
            silences: Vec::new(),
            block_len: frames_in(rate.hz(), BLOCK_MS),
            block_fill: 0,
            block_start: 0,
            second_energy: 0.0,
            second_frames: 0,
            second_blocks: 0,
            seconds: Vec::new(),
            head: Vec::new(),
            tail: VecDeque::new(),
            scratch: Vec::new(),
            features: FeatureExtractor::new(rate.hz(), usize::from(layout.channel_count().max(1))),
        })
    }

    /// Adds a decoded block, following a change of rate or channels.
    pub fn add_block(&mut self, block: &PcmBlock) -> Result<(), AnalysisError> {
        if block.is_empty() {
            return Ok(());
        }
        let spec = block.spec();
        if spec.rate.hz() != self.segment_rate || spec.layout != self.layout {
            self.change(spec.rate, spec.layout)?;
        }
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        block.append_f32(&mut scratch);
        let result = self.add_f32(&scratch);
        self.scratch = scratch;
        result
    }

    /// Adds interleaved samples in the analyzer's current layout, full scale at
    /// ±1.0.
    pub fn add_f32(&mut self, interleaved: &[f32]) -> Result<(), AnalysisError> {
        let channels = self.channels;
        let frames = interleaved.len() / channels;
        self.features.add(&interleaved[..frames * channels]);
        let mut offset = 0;
        while offset < frames {
            let room = usize::try_from(self.block_len - self.block_fill).unwrap_or(usize::MAX);
            let take = (frames - offset).min(room);
            let slice = &interleaved[offset * channels..(offset + take) * channels];
            self.meter.add_f32(slice)?;
            self.find_silence(slice);
            offset += take;
            self.block_fill += take as u64;
            if self.block_fill == self.block_len {
                self.close_block();
            }
        }
        Ok(())
    }

    /// The frames analysed so far, at the first rate.
    fn position(&self) -> u64 {
        self.at(self.segment_frames)
    }

    /// The position of frame `frames` of the current shape, at the first rate.
    fn at(&self, frames: u64) -> u64 {
        if self.segment_rate == self.rate {
            return self.segment_start + frames;
        }
        let scaled =
            u128::from(frames) * u128::from(self.rate) / u128::from(self.segment_rate.max(1));
        self.segment_start + u64::try_from(scaled).unwrap_or(u64::MAX)
    }

    fn find_silence(&mut self, slice: &[f32]) {
        let channels = self.channels;
        #[allow(clippy::cast_precision_loss)]
        let scale = 1.0 / channels as f64;
        for frame in slice.chunks_exact(channels) {
            if self.window_fill == 0 {
                self.window_start = self.at(self.segment_frames);
            }
            let square: f64 = frame.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
            self.window_sum += square * scale;
            self.window_fill += 1;
            self.segment_frames += 1;
            if self.window_fill == self.window_len {
                self.close_window();
            }
        }
    }

    fn close_window(&mut self) {
        if self.window_fill == 0 {
            return;
        }
        #[allow(clippy::cast_precision_loss)]
        let mean_square = self.window_sum / self.window_fill as f64;
        let (start, end) = (self.window_start, self.position());
        self.window_fill = 0;
        self.window_sum = 0.0;
        if mean_square < threshold_mean_square() {
            if self.quiet_since.is_none() {
                self.quiet_since = Some(start);
            }
            return;
        }
        if let Some(quiet) = self.quiet_since.take() {
            let long = frames_in(self.rate, MIN_INNER_SILENCE_MS);
            if self.first_sound.is_some()
                && start - quiet >= long
                && self.silences.len() < MAX_INNER_SILENCES
            {
                self.silences.push(SilentSpan {
                    start: quiet,
                    end: start,
                });
            }
        }
        if self.first_sound.is_none() {
            self.first_sound = Some(start);
        }
        self.sound_end = Some(end);
    }

    fn close_block(&mut self) {
        if self.block_fill == 0 {
            return;
        }
        let ms = u32::try_from(self.block_fill * 1000 / u64::from(self.segment_rate.max(1)))
            .unwrap_or(BLOCK_MS)
            .clamp(1, BLOCK_MS);
        let energy = lufs_to_energy(self.meter.window_lufs(ms));
        let end = self.position();
        let block = Block {
            start: self.block_start,
            end,
            energy,
        };
        self.block_start = end;
        self.block_fill = 0;

        let len = block.end - block.start;
        #[allow(clippy::cast_precision_loss)]
        let weight = len as f64;
        self.second_energy += energy * weight;
        self.second_frames += len;
        self.second_blocks += 1;
        if self.second_blocks == BLOCKS_PER_SECOND {
            self.close_second();
        }
        if self.first_sound.is_some() && self.head.len() < SEARCH_BLOCKS {
            self.head.push(block);
        }
        self.tail.push_back(block);
        if self.tail.len() > TAIL_BLOCKS {
            self.tail.pop_front();
        }
    }

    fn close_second(&mut self) {
        if self.second_blocks == 0 {
            return;
        }
        #[allow(clippy::cast_precision_loss)]
        let mean = self.second_energy / self.second_frames.max(1) as f64;
        self.seconds.push(mean);
        self.second_energy = 0.0;
        self.second_frames = 0;
        self.second_blocks = 0;
    }

    fn change(&mut self, rate: SampleRate, layout: ChannelLayout) -> Result<(), AnalysisError> {
        self.close_window();
        self.close_block();
        self.segment_start = self.position();
        self.segment_frames = 0;
        self.meter.follow(rate, layout)?;
        self.segment_rate = rate.hz();
        self.layout = layout;
        self.channels = usize::from(layout.channel_count().max(1));
        self.window_len = frames_in(rate.hz(), SILENCE_WINDOW_MS);
        self.block_len = frames_in(rate.hz(), BLOCK_MS);
        self.features.follow(rate.hz(), self.channels);
        Ok(())
    }

    /// The analysis of everything added, and the loudness measurement when it was
    /// asked for.
    pub fn finish(mut self) -> Result<(TrackAnalysis, Option<TrackMeasurement>), AnalysisError> {
        self.close_window();
        self.close_block();
        self.close_second();
        let frames = self.position();
        if frames == 0 {
            return Err(AnalysisError::Empty);
        }
        let integrated = self.meter.integrated();
        let audio = self.first_sound.zip(self.sound_end).map(|(s, e)| s..e);
        let spread = loudness_spread(self.seconds.iter().copied().map(energy_to_lufs));
        let features = self
            .features
            .finish(spread)
            .filter(|_| audio.is_some())
            .map(Box::new);

        let mut analysis = TrackAnalysis {
            rate: self.rate,
            frames,
            audio: audio.clone(),
            silences: std::mem::take(&mut self.silences),
            integrated_lufs: integrated,
            envelope: envelope(&self.seconds),
            features,
            ..TrackAnalysis::default()
        };
        if let Some(audio) = audio {
            let head: Vec<Block> = self
                .head
                .iter()
                .copied()
                .filter(|b| b.start < audio.end)
                .collect();
            let sounding: Vec<Block> = self
                .tail
                .iter()
                .copied()
                .filter(|b| b.start < audio.end)
                .collect();
            let tail = &sounding[sounding.len().saturating_sub(SEARCH_BLOCKS)..];
            analysis.head_lufs = mean_lufs(&head[..head.len().min(EDGE_BLOCKS)]);
            analysis.tail_lufs = mean_lufs(&tail[tail.len().saturating_sub(EDGE_BLOCKS)..]);
            if let Some(body) = integrated.map(|lufs| lufs - BODY_DROP_LU) {
                let clamp = |at: u64| at.clamp(audio.start, audio.end);
                analysis.fade_in_end = fade_in_end(&head, body).map(clamp);
                analysis.fade_out_start = fade_out_start(tail, body).map(clamp);
            }
        }
        let loudness = if self.measure_loudness {
            Some(self.meter.finish()?)
        } else {
            None
        };
        Ok((analysis, loudness))
    }
}

/// The mean square of a signal at the silence threshold.
pub(super) fn threshold_mean_square() -> f64 {
    10f64.powf(SILENCE_THRESHOLD_DBFS / 10.0)
}

/// The frames in `ms` milliseconds at `rate`, at least one.
fn frames_in(rate: u32, ms: u32) -> u64 {
    (u64::from(rate) * u64::from(ms) / 1000).max(1)
}

/// BS.1770: loudness = −0.691 + 10 log10(energy).
fn lufs_to_energy(lufs: f64) -> f64 {
    if lufs.is_finite() {
        10f64.powf((lufs + 0.691) / 10.0)
    } else {
        0.0
    }
}

fn energy_to_lufs(energy: f64) -> f64 {
    if energy > 0.0 {
        -0.691 + 10.0 * energy.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// The loudness of `blocks` together; `None` without blocks or sound.
fn mean_lufs(blocks: &[Block]) -> Option<f64> {
    let frames: u64 = blocks.iter().map(|b| b.end - b.start).sum();
    if frames == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let energy = blocks
        .iter()
        .map(|b| b.energy * (b.end - b.start) as f64)
        .sum::<f64>()
        / frames as f64;
    Some(energy_to_lufs(energy)).filter(|lufs| lufs.is_finite())
}

/// The middle of a window of blocks.
fn middle(window: &[Block]) -> u64 {
    let (start, end) = (window[0].start, window[window.len() - 1].end);
    start + (end - start) / 2
}

/// The middle of the first 1 s window of `head` at body level; the end of `head`
/// when none is.
fn fade_in_end(head: &[Block], body: f64) -> Option<u64> {
    let size = FADE_WINDOW_BLOCKS.min(head.len());
    if size == 0 {
        return None;
    }
    head.windows(size)
        .find(|w| mean_lufs(w).is_some_and(|l| l >= body))
        .map(middle)
        .or_else(|| head.last().map(|b| b.end))
}

/// The middle of the last 1 s window of `tail` at body level; the start of `tail`
/// when none is.
fn fade_out_start(tail: &[Block], body: f64) -> Option<u64> {
    let size = FADE_WINDOW_BLOCKS.min(tail.len());
    if size == 0 {
        return None;
    }
    tail.windows(size)
        .rev()
        .find(|w| mean_lufs(w).is_some_and(|l| l >= body))
        .map(middle)
        .or_else(|| tail.first().map(|b| b.start))
}

/// The envelope of the mean energies of each second: one value per second, or per
/// a longer step for very long tracks.
fn envelope(seconds: &[f64]) -> LoudnessEnvelope {
    let mut values = seconds.to_vec();
    let mut step: u8 = 1;
    while values.len() > MAX_ENVELOPE_STEPS && step < 128 {
        values = values
            .chunks(2)
            .map(|pair| {
                #[allow(clippy::cast_precision_loss)]
                let n = pair.len() as f64;
                pair.iter().sum::<f64>() / n
            })
            .collect();
        step *= 2;
    }
    LoudnessEnvelope::from_lufs(step, values.into_iter().map(energy_to_lufs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_tracks_get_a_longer_step() {
        let seconds = vec![1.0; MAX_ENVELOPE_STEPS * 3];
        let envelope = envelope(&seconds);
        assert_eq!(envelope.step_secs(), 4);
        assert_eq!(envelope.len(), MAX_ENVELOPE_STEPS * 3 / 4);
        assert!(envelope.len() <= MAX_ENVELOPE_STEPS);
        let lufs = envelope.lufs(0).unwrap();
        assert!((lufs - (-0.691)).abs() < 0.3, "{lufs}");
    }

    #[test]
    fn energies_and_loudness_agree() {
        for lufs in [-70.0, -23.0, -0.691, 3.0] {
            assert!((energy_to_lufs(lufs_to_energy(lufs)) - lufs).abs() < 1e-9);
        }
        assert_eq!(lufs_to_energy(f64::NEG_INFINITY), 0.0);
        assert_eq!(energy_to_lufs(0.0), f64::NEG_INFINITY);
    }
}
