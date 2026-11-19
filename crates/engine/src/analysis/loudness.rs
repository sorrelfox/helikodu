// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The loudness scanner: EBU R128 / ITU-R BS.1770-4 measurement with the `ebur128`
//! crate.
//!
//! - K-weighting, 400 ms gating blocks with 75 % overlap, an absolute gate at
//!   −70 LUFS and a relative gate at −10 LU, channel weights of 1.0 for the front
//!   channels and 1.41 for the surrounds, the LFE channel left out. A mono source
//!   counts as dual mono (it plays on two speakers), as ReplayGain 2.0 scanners do.
//! - Loudness range from 3 s short-term blocks (EBU Tech 3342).
//! - True peak by 4× oversampling (2× above 96 kHz), as the crate does it.
//! - Album values are the loudness of the album's tracks measured as one programme:
//!   the gating blocks of all tracks are gated together. They are not the mean of
//!   the track values.
//!
//! Gains derived from the measurements are relative to the ReplayGain 2.0
//! reference of −18 LUFS ([`REFERENCE_LUFS`]).

use std::fmt;

use ebur128::{Channel, EbuR128, Mode};
use helikodu_model::audio::{ChannelLayout, ChannelPosition, SampleRate};
use helikodu_model::library::MediaSource;

use crate::decode::{DecodeError, DecoderRegistry, PcmBlock};
use crate::pipeline::REFERENCE_LUFS;

/// The version of the measurement. It changes when a change to the scanner (or to
/// the `ebur128` crate) changes its results, so that stored results of an older
/// version can be measured again.
pub const MEASUREMENT_VERSION: u32 = 1;

/// What the meter measures: integrated loudness, loudness range and both peaks,
/// with histograms so that a finished track keeps only a small state for its album.
fn mode() -> Mode {
    Mode::I | Mode::LRA | Mode::SAMPLE_PEAK | Mode::TRUE_PEAK | Mode::HISTOGRAM
}

/// The rate a finished meter is switched to, which shrinks its audio buffer to
/// almost nothing while it keeps its gating blocks for [`album_loudness`].
const IDLE_RATE: u32 = 16;

/// Why a source could not be measured.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AnalysisError {
    /// The source could not be opened or decoded.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// The meter cannot measure audio of this shape.
    #[error("cannot measure {channels} channels at {rate} Hz")]
    Unsupported {
        /// The number of channels.
        channels: u16,
        /// The sample rate in Hz.
        rate: u32,
    },
    /// The source decoded to no audio at all.
    #[error("the source holds no audio")]
    Empty,
    /// The measurement was stopped by its caller.
    #[error("the measurement was cancelled")]
    Cancelled,
}

/// The loudness of one track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackLoudness {
    /// The integrated (programme) loudness in LUFS; `None` when no gating block
    /// is above the absolute gate (silence).
    pub integrated_lufs: Option<f64>,
    /// The loudness range in LU (0 for tracks shorter than a few seconds).
    pub range_lu: f64,
    /// The largest sample, linear (1.0 is full scale).
    pub sample_peak: f64,
    /// The true peak, linear, from the oversampled signal.
    pub true_peak: f64,
    /// The number of frames measured.
    pub frames: u64,
    /// The sample rate of the source in Hz (the last one, if it changed).
    pub rate: u32,
}

impl TrackLoudness {
    /// The ReplayGain 2.0 track gain in dB: the change that brings the track to
    /// −18 LUFS. `None` for silence.
    pub fn gain_db(&self) -> Option<f64> {
        self.integrated_lufs.map(|lufs| REFERENCE_LUFS - lufs)
    }
}

/// The loudness of an album, measured as one programme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlbumLoudness {
    /// The integrated loudness of all tracks together, in LUFS; `None` for silence.
    pub integrated_lufs: Option<f64>,
    /// The loudness range of all tracks together, in LU.
    pub range_lu: f64,
    /// The largest sample peak of the tracks.
    pub sample_peak: f64,
    /// The largest true peak of the tracks.
    pub true_peak: f64,
}

impl AlbumLoudness {
    /// The ReplayGain 2.0 album gain in dB. `None` for silence.
    pub fn gain_db(&self) -> Option<f64> {
        self.integrated_lufs.map(|lufs| REFERENCE_LUFS - lufs)
    }
}

/// A measured track: its values, and what [`album_loudness`] needs of it.
pub struct TrackMeasurement {
    /// The track's values.
    pub loudness: TrackLoudness,
    /// The codec the backend decoded, such as `flac` or `opus`; empty when the
    /// samples did not come from [`measure_source`].
    pub codec: String,
    /// The meter, its audio buffer shrunk, holding the gating blocks.
    state: EbuR128,
}

impl fmt::Debug for TrackMeasurement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrackMeasurement")
            .field("loudness", &self.loudness)
            .field("codec", &self.codec)
            .finish_non_exhaustive()
    }
}

/// Measures interleaved audio as it arrives.
pub struct LoudnessMeter {
    state: EbuR128,
    rate: u32,
    layout: ChannelLayout,
    frames: u64,
    /// The peaks of the parts before a change of the channel count, which resets
    /// the meter's own.
    sample_peak: f64,
    true_peak: f64,
    scratch: Vec<f32>,
}

impl fmt::Debug for LoudnessMeter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoudnessMeter")
            .field("rate", &self.rate)
            .field("layout", &self.layout)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

impl LoudnessMeter {
    /// A meter for audio at `rate` with the channels of `layout`.
    pub fn new(rate: SampleRate, layout: ChannelLayout) -> Result<Self, AnalysisError> {
        Self::with_mode(rate, layout, mode())
    }

    /// A meter of the integrated loudness and short windows only, without the
    /// loudness range and the peaks: what the track analysis needs when the
    /// loudness is not measured with it.
    pub(crate) fn light(rate: SampleRate, layout: ChannelLayout) -> Result<Self, AnalysisError> {
        Self::with_mode(rate, layout, Mode::I | Mode::HISTOGRAM)
    }

    fn with_mode(
        rate: SampleRate,
        layout: ChannelLayout,
        mode: Mode,
    ) -> Result<Self, AnalysisError> {
        let unsupported = || AnalysisError::Unsupported {
            channels: layout.channel_count(),
            rate: rate.hz(),
        };
        let mut state = EbuR128::new(u32::from(layout.channel_count()), rate.hz(), mode)
            .map_err(|_| unsupported())?;
        state
            .set_channel_map(&channel_map(layout))
            .map_err(|_| unsupported())?;
        Ok(Self {
            state,
            rate: rate.hz(),
            layout,
            frames: 0,
            sample_peak: 0.0,
            true_peak: 0.0,
            scratch: Vec::new(),
        })
    }

    /// Adds interleaved samples in the meter's layout, full scale at ±1.0.
    pub fn add_f32(&mut self, interleaved: &[f32]) -> Result<(), AnalysisError> {
        let channels = usize::from(self.layout.channel_count());
        let whole = interleaved.len() - interleaved.len() % channels;
        self.state
            .add_frames_f32(&interleaved[..whole])
            .map_err(|_| self.unsupported())?;
        self.frames += (whole / channels) as u64;
        Ok(())
    }

    /// Adds a decoded block, following a change of rate or channels.
    pub fn add_block(&mut self, block: &PcmBlock) -> Result<(), AnalysisError> {
        if block.is_empty() {
            return Ok(());
        }
        let spec = block.spec();
        if spec.rate.hz() != self.rate || spec.layout != self.layout {
            self.change(spec.rate, spec.layout)?;
        }
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        block.append_f32(&mut scratch);
        let result = self.add_f32(&scratch);
        self.scratch = scratch;
        result
    }

    /// The number of frames measured so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Switches to audio at `rate` with the channels of `layout`, if it is not
    /// already in that shape. The short windows start again from silence.
    pub(crate) fn follow(
        &mut self,
        rate: SampleRate,
        layout: ChannelLayout,
    ) -> Result<(), AnalysisError> {
        if rate.hz() != self.rate || layout != self.layout {
            self.change(rate, layout)?;
        }
        Ok(())
    }

    /// The K-weighted loudness of the last `ms` milliseconds (at most 400) in
    /// LUFS; `-inf` for digital silence.
    pub(crate) fn window_lufs(&self, ms: u32) -> f64 {
        self.state
            .loudness_window(ms.clamp(1, 400))
            .unwrap_or(f64::NEG_INFINITY)
    }

    /// The integrated loudness so far in LUFS; `None` for silence.
    pub(crate) fn integrated(&self) -> Option<f64> {
        self.state
            .loudness_global()
            .ok()
            .filter(|lufs| lufs.is_finite())
    }

    /// The values of everything added, and the state an album measurement needs.
    pub fn finish(mut self) -> Result<TrackMeasurement, AnalysisError> {
        if self.frames == 0 {
            return Err(AnalysisError::Empty);
        }
        let (sample_peak, true_peak) = self.peaks();
        let integrated = self
            .state
            .loudness_global()
            .map_err(|_| self.unsupported())?;
        let range = self
            .state
            .loudness_range()
            .map_err(|_| self.unsupported())?;
        let loudness = TrackLoudness {
            integrated_lufs: integrated.is_finite().then_some(integrated),
            range_lu: if range.is_finite() { range } else { 0.0 },
            sample_peak,
            true_peak,
            frames: self.frames,
            rate: self.rate,
        };
        // The gating blocks stay; only the audio buffer and the unfinished block go.
        let channels = self.state.channels();
        self.state
            .change_parameters(channels, IDLE_RATE)
            .map_err(|_| self.unsupported())?;
        Ok(TrackMeasurement {
            loudness,
            codec: String::new(),
            state: self.state,
        })
    }

    /// The largest sample and true peak over all channels so far.
    fn peaks(&self) -> (f64, f64) {
        let mut sample = self.sample_peak;
        let mut true_peak = self.true_peak;
        for channel in 0..self.state.channels() {
            sample = sample.max(self.state.sample_peak(channel).unwrap_or(0.0));
            true_peak = true_peak.max(self.state.true_peak(channel).unwrap_or(0.0));
        }
        // The oversampled peak can fall a hair below a sample; a true peak is never
        // lower than the largest sample.
        (sample, true_peak.max(sample))
    }

    fn change(&mut self, rate: SampleRate, layout: ChannelLayout) -> Result<(), AnalysisError> {
        let (sample, true_peak) = self.peaks();
        self.sample_peak = sample;
        self.true_peak = true_peak;
        let unsupported = AnalysisError::Unsupported {
            channels: layout.channel_count(),
            rate: rate.hz(),
        };
        self.state
            .change_parameters(u32::from(layout.channel_count()), rate.hz())
            .map_err(|_| unsupported)?;
        self.state
            .set_channel_map(&channel_map(layout))
            .map_err(|_| AnalysisError::Unsupported {
                channels: layout.channel_count(),
                rate: rate.hz(),
            })?;
        self.rate = rate.hz();
        self.layout = layout;
        Ok(())
    }

    fn unsupported(&self) -> AnalysisError {
        AnalysisError::Unsupported {
            channels: self.layout.channel_count(),
            rate: self.rate,
        }
    }
}

/// The weighting of each channel of `layout`, in interleaving order.
fn channel_map(layout: ChannelLayout) -> Vec<Channel> {
    let count = usize::from(layout.channel_count());
    if count == 1 {
        return vec![Channel::DualMono];
    }
    let layout = if layout.is_unspecified() {
        match ChannelLayout::default_for_count(layout.channel_count()) {
            Some(known) if !known.is_unspecified() => known,
            _ => return unspecified_map(count),
        }
    } else {
        layout
    };
    let sides = layout.contains(ChannelPosition::SideLeft);
    let backs = layout.contains(ChannelPosition::BackLeft);
    layout
        .positions()
        .map(|position| match position {
            ChannelPosition::FrontLeft => Channel::Left,
            ChannelPosition::FrontRight => Channel::Right,
            ChannelPosition::FrontCenter => Channel::Center,
            ChannelPosition::LowFrequency => Channel::Unused,
            // With both pairs, the back pair sits at ±135° and the side pair at ±90°;
            // with one pair, it is the ±110° surround pair of BS.1770.
            ChannelPosition::BackLeft if sides => Channel::Mp135,
            ChannelPosition::BackRight if sides => Channel::Mm135,
            ChannelPosition::SideLeft if backs => Channel::Mp090,
            ChannelPosition::SideRight if backs => Channel::Mm090,
            ChannelPosition::BackLeft | ChannelPosition::SideLeft => Channel::LeftSurround,
            ChannelPosition::BackRight | ChannelPosition::SideRight => Channel::RightSurround,
            ChannelPosition::FrontLeftOfCenter => Channel::MpSC,
            ChannelPosition::FrontRightOfCenter => Channel::MmSC,
            ChannelPosition::BackCenter => Channel::Mp180,
            ChannelPosition::TopCenter => Channel::Tp000,
            ChannelPosition::TopFrontLeft => Channel::Up030,
            ChannelPosition::TopFrontCenter => Channel::Up000,
            ChannelPosition::TopFrontRight => Channel::Um030,
            ChannelPosition::TopBackLeft => Channel::Up135,
            ChannelPosition::TopBackCenter => Channel::Up180,
            ChannelPosition::TopBackRight => Channel::Um135,
        })
        .collect()
}

/// Channels without positions: the first two are left and right, the rest count
/// with weight 1.0 as further front channels would.
fn unspecified_map(count: usize) -> Vec<Channel> {
    (0..count)
        .map(|index| match index {
            0 => Channel::Left,
            1 => Channel::Right,
            _ => Channel::Center,
        })
        .collect()
}

/// Decodes `source` (a whole file or a section of one) to its end and measures it.
///
/// `cancelled` is asked between blocks; when it returns `true` the measurement
/// stops with [`AnalysisError::Cancelled`].
pub fn measure_source(
    registry: &DecoderRegistry,
    source: &MediaSource,
    cancelled: &dyn Fn() -> bool,
) -> Result<TrackMeasurement, AnalysisError> {
    let mut decoder = registry.open(source)?;
    let spec = decoder.spec();
    let codec = decoder.info().codec.clone();
    let mut meter = LoudnessMeter::new(spec.rate, spec.layout)?;
    let mut block = PcmBlock::new();
    loop {
        if cancelled() {
            return Err(AnalysisError::Cancelled);
        }
        if !decoder.read(&mut block)? {
            break;
        }
        meter.add_block(&block)?;
    }
    let mut measurement = meter.finish()?;
    measurement.codec = codec;
    Ok(measurement)
}

/// The loudness of `tracks` measured as one programme, or `None` without tracks.
pub fn album_loudness<'a>(
    tracks: impl IntoIterator<Item = &'a TrackMeasurement>,
) -> Option<AlbumLoudness> {
    let tracks: Vec<&TrackMeasurement> = tracks.into_iter().collect();
    if tracks.is_empty() {
        return None;
    }
    let integrated = EbuR128::loudness_global_multiple(tracks.iter().map(|t| &t.state)).ok()?;
    let range = EbuR128::loudness_range_multiple(tracks.iter().map(|t| &t.state)).ok()?;
    let peak = |of: fn(&TrackLoudness) -> f64| {
        tracks
            .iter()
            .map(|t| of(&t.loudness))
            .fold(0.0_f64, f64::max)
    };
    Some(AlbumLoudness {
        integrated_lufs: integrated.is_finite().then_some(integrated),
        range_lu: if range.is_finite() { range } else { 0.0 },
        sample_peak: peak(|l| l.sample_peak),
        true_peak: peak(|l| l.true_peak),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_maps_follow_bs_1770() {
        assert_eq!(channel_map(ChannelLayout::MONO), [Channel::DualMono]);
        assert_eq!(
            channel_map(ChannelLayout::SURROUND_5_1),
            [
                Channel::Left,
                Channel::Right,
                Channel::Center,
                Channel::Unused,
                Channel::LeftSurround,
                Channel::RightSurround,
            ]
        );
        assert_eq!(
            channel_map(ChannelLayout::SURROUND_7_1),
            [
                Channel::Left,
                Channel::Right,
                Channel::Center,
                Channel::Unused,
                Channel::Mp135,
                Channel::Mm135,
                Channel::Mp090,
                Channel::Mm090,
            ]
        );
        let unknown = ChannelLayout::unspecified(2).unwrap();
        assert_eq!(channel_map(unknown), [Channel::Left, Channel::Right]);
        let many = ChannelLayout::unspecified(10).unwrap();
        assert_eq!(channel_map(many).len(), 10);
    }

    #[test]
    fn silence_has_no_loudness() {
        let rate = SampleRate::new(48_000).unwrap();
        let mut meter = LoudnessMeter::new(rate, ChannelLayout::STEREO).unwrap();
        meter.add_f32(&vec![0.0; 48_000 * 2 * 3]).unwrap();
        let measured = meter.finish().unwrap();
        assert_eq!(measured.loudness.integrated_lufs, None);
        assert_eq!(measured.loudness.gain_db(), None);
        assert_eq!(measured.loudness.frames, 48_000 * 3);
        let album = album_loudness([&measured]).unwrap();
        assert_eq!(album.integrated_lufs, None);
        assert!(album_loudness([]).is_none());
    }

    #[test]
    fn nothing_measured_is_an_error() {
        let rate = SampleRate::new(44_100).unwrap();
        let meter = LoudnessMeter::new(rate, ChannelLayout::STEREO).unwrap();
        assert!(matches!(meter.finish(), Err(AnalysisError::Empty)));
    }
}
