// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Silence left out, from the track analysis of the items
//! ([`PlaybackItem::analysis`]).
//!
//! - **At joins.** Once the item after the current one is known (`SetNext`), the
//!   current item ends [`MARGIN`] after its sound and the next one starts
//!   [`MARGIN`] before its sound, when silence trimming is on, or when the crossfade
//!   takes its overlap from the analysis and both items have one. Not between
//!   tracks of one album kept gapless: their silence belongs to the music (a
//!   pregap, a hidden track). The last item of the queue ends after its sound too.
//! - **At a start.** A `Load` from the beginning, with silence trimming on, starts
//!   [`MARGIN`] before the sound. A later seek goes where it is asked to, silence or
//!   not.
//! - **Inside a track.** With skipping on, a silent stretch at least as long as the
//!   setting asks is played for [`SKIP_KEEP`] at each end, and the rest is jumped
//!   over. Stretches that begin in the first [`SKIP_GUARD`] of a track are kept.
//! - **Bit-perfect.** None of this on a bit-perfect path in exclusive mode.
//!
//! The margins, the silence kept and the guard are track time (they belong to the
//! music); what the transport derives from them for the ring buffer (the
//! trailing silence still to cut, the overlap of a crossfade, `NearEnd`'s lead)
//! is converted to listening time with the item's speed.
//!
//! Positions stay those of the file: a track that starts after its leading silence
//! reports its first position there, and a jump over silence moves the position
//! forward as a seek would. The length reported is the file's.
//!
//! The decision about the end of the current item waits for the next one. A
//! freewheeling sink waits for it at the end of the sound, as for a crossfade; in
//! real time, `NearEnd` comes earlier by the trailing silence, so the answer is
//! there in time. When it is not, the silence plays.

use std::time::Duration;

use helikodu_model::analysis::TrackAnalysis;
use helikodu_model::audio::{MediaTime, OutputMode};
use helikodu_model::player::PlaybackItem;
use helikodu_model::settings::TransitionSettings;

use super::source::Source;
use super::worker::Worker;
use crate::pipeline::{PathPlan, TimeScale};

/// The silence kept before and after the sound of a trimmed track.
pub(super) const MARGIN: Duration = Duration::from_millis(10);
/// The silence kept at each end of a silent stretch that is skipped.
pub(super) const SKIP_KEEP: Duration = Duration::from_secs(1);
/// How far into a track a skipped stretch must begin at the earliest.
pub(super) const SKIP_GUARD: Duration = Duration::from_secs(15);

/// `frames` at `from` Hz as frames at `to` Hz, rounded down.
fn at_rate(frames: u64, from: u32, to: u32) -> u64 {
    if from == to || from == 0 {
        return frames;
    }
    u64::try_from(u128::from(frames) * u128::from(to) / u128::from(from)).unwrap_or(u64::MAX)
}

/// The frames `duration` lasts at `rate`.
fn frames_of(duration: Duration, rate: u32) -> u64 {
    (duration.as_secs_f64() * f64::from(rate)).round() as u64
}

/// The analysis of `source`'s item, with the rate of the source.
fn analysis_of(source: &Source) -> Option<(&TrackAnalysis, u32)> {
    let analysis = source.item().analysis.as_deref()?;
    Some((analysis, source.rate().hz()))
}

/// Where `source` ends when its trailing silence is left out, in source frames;
/// `None` for a silent track or one without trailing silence.
pub(super) fn tail_end(source: &Source) -> Option<u64> {
    let (analysis, rate) = analysis_of(source)?;
    let audio = analysis.audio.as_ref()?;
    let end = at_rate(audio.end, analysis.rate, rate) + frames_of(MARGIN, rate);
    let frames = at_rate(analysis.frames, analysis.rate, rate);
    (end < frames).then_some(end)
}

/// Where `source` starts when its leading silence is left out, in source frames;
/// `None` for a silent track or one that starts with its sound.
pub(super) fn head_start(source: &Source) -> Option<u64> {
    let (analysis, rate) = analysis_of(source)?;
    let audio = analysis.audio.as_ref()?;
    let start = at_rate(audio.start, analysis.rate, rate).saturating_sub(frames_of(MARGIN, rate));
    (start > 0).then_some(start)
}

/// The overlap the analysis gives a crossfade out of `source`, in ring frames of
/// `scale` (listening time): from where the track begins to fade out for good to
/// the end of its sound (and the margin after it). Without a fade that is the last
/// half second or so.
pub(super) fn analysed_overlap(source: &Source, scale: &TimeScale) -> Option<u64> {
    let analysis = source.item().analysis.as_deref()?;
    let audio = analysis.audio.as_ref()?;
    let fade = analysis
        .fade_out_start
        .unwrap_or(audio.end)
        .clamp(audio.start, audio.end);
    let rate = scale.source_rate().hz();
    let frames = at_rate(audio.end - fade, analysis.rate, rate) + frames_of(MARGIN, rate);
    Some(scale.to_ring(frames))
}

/// The silent stretches of `source` to jump over with `transitions`, in source
/// frames: from [`SKIP_KEEP`] into each long enough stretch to [`SKIP_KEEP`] before
/// its end.
pub(super) fn skips_for(transitions: &TransitionSettings, source: &Source) -> Vec<(u64, u64)> {
    let (Some(min), Some((analysis, rate))) = (transitions.silence_skip(), analysis_of(source))
    else {
        return Vec::new();
    };
    let keep = frames_of(SKIP_KEEP, rate);
    let guard = frames_of(SKIP_GUARD, rate);
    let min = frames_of(min, rate).max(2 * keep + 1);
    analysis
        .silences
        .iter()
        .map(|span| {
            (
                at_rate(span.start, analysis.rate, rate),
                at_rate(span.end, analysis.rate, rate),
            )
        })
        .filter(|&(start, end)| end.saturating_sub(start) >= min && start >= guard)
        .map(|(start, end)| (start + keep, end - keep))
        .collect()
}

/// Whether the settings leave silence out where tracks join: trimming is on, or the
/// crossfade takes its overlap from the analysis (then both items need one).
fn trims_joins(transitions: &TransitionSettings, both_analysed: bool) -> bool {
    transitions.trim_silence || (transitions.analysed_crossfade() && both_analysed)
}

impl Worker {
    /// Whether the current item plays on a bit-perfect path in exclusive mode:
    /// nothing is crossfaded, trimmed, skipped or levelled then. Before a stream
    /// is open, the selected mode and the last plan decide.
    pub(super) fn bit_perfect_exclusive(&self) -> bool {
        let plan = self.current.as_ref().and_then(Source::plan);
        match &self.output {
            Some(output) => {
                output.mode == OutputMode::Exclusive && plan.is_some_and(PathPlan::is_bit_perfect)
            }
            None => {
                self.selection.mode == OutputMode::Exclusive
                    && plan.is_none_or(PathPlan::is_bit_perfect)
            }
        }
    }

    /// Whether `a` and `b` join without anything changed: consecutive tracks of
    /// one album while the settings keep albums gapless.
    pub(super) fn gapless_join(&self, a: &PlaybackItem, b: &PlaybackItem) -> bool {
        self.config.transitions.keep_albums_gapless && a.same_album(b)
    }

    /// Decides where the current item ends and where the next one starts, now that
    /// the next one is known (or known to be none).
    pub(super) fn plan_joins(&mut self) {
        if self.resume.as_ref().is_some_and(|r| r.follower.is_some()) {
            // Paused after the join was rendered: decided again on resume.
            return;
        }
        let bit_perfect = self.bit_perfect_exclusive();
        let transitions = self.config.transitions;
        let Some(current) = &self.current else {
            return;
        };
        let id = current.item().id;
        let (gapless, next_analysed) = match &self.next {
            Some(next) => (
                self.gapless_join(current.item(), next.item()),
                next.item().analysis.is_some(),
            ),
            None => (false, false),
        };
        let both = current.item().analysis.is_some() && next_analysed;
        let trims = !gapless && !bit_perfect && trims_joins(&transitions, both);
        let end = if trims { tail_end(current) } else { None };
        if let Some(current) = &mut self.current {
            current.set_end(end);
        }
        if trims
            && let Some(next) = &mut self.next
            && next.is_at_start()
            && let Some(start) = head_start(next)
        {
            let rate = next.rate();
            // A failed seek leaves the silence in; the item still plays.
            let _ = next.seek(MediaTime::from_frames(start, rate));
        }
        self.joins_planned = Some(id);
    }

    /// The trailing silence of the current item that may still be left out, in
    /// frames at the device rate, while the item after it is not known.
    pub(super) fn pending_tail_cut(&self) -> Option<u64> {
        let transitions = &self.config.transitions;
        if !(transitions.trim_silence || transitions.analysed_crossfade()) {
            return None;
        }
        let current = self.current.as_ref()?;
        if self.joins_planned == Some(current.item().id) || self.bit_perfect_exclusive() {
            return None;
        }
        let end = tail_end(current)?;
        let length = current.length()?.frames_at(current.rate());
        self.output.as_ref()?;
        Some(current.scale().to_ring(length.saturating_sub(end))).filter(|&f| f > 0)
    }

    /// How many frames a freewheeling sink may render before it waits for the next
    /// item at the end of the current item's sound, when the trailing silence may be
    /// left out. `None` when it need not wait.
    pub(super) fn trim_wait(&mut self, channels: usize) -> Option<u64> {
        if self.next_answered || !self.output.as_ref().is_some_and(|o| o.freewheel) {
            return None;
        }
        let cut = self.pending_tail_cut()?;
        let remaining = self.current.as_ref()?.remaining_frames()?;
        let horizon = remaining.saturating_sub(cut);
        let room = (self.staging.len() / channels.max(1)) as u64;
        if horizon > room {
            return None;
        }
        self.waits_for_next().then_some(horizon)
    }

    /// How much earlier `NearEnd` comes for `item`: its trailing silence, when it
    /// may be left out, in track time.
    pub(super) fn tail_lead(&self, item: &PlaybackItem) -> Duration {
        let transitions = &self.config.transitions;
        if !(transitions.trim_silence || transitions.analysed_crossfade())
            || self.bit_perfect_exclusive()
        {
            return Duration::ZERO;
        }
        item.analysis
            .as_deref()
            .filter(|a| a.audio.is_some() && a.rate > 0)
            .map_or(Duration::ZERO, |a| {
                Duration::from_secs_f64(a.trailing_silence() as f64 / f64::from(a.rate))
            })
    }

    /// The skips of the current item, as the settings ask for them now.
    pub(super) fn update_skips(&mut self) {
        let bit_perfect = self.bit_perfect_exclusive();
        let transitions = self.config.transitions;
        if let Some(current) = &mut self.current {
            let skips = if bit_perfect {
                Vec::new()
            } else {
                skips_for(&transitions, current)
            };
            current.set_skips(skips);
        }
    }

    /// A `Load` from the beginning: the leading silence of the item is left out
    /// when it starts, if the settings ask for it.
    pub(super) fn trim_start_of(&mut self, item: &PlaybackItem, start: MediaTime) {
        self.trim_start =
            (self.config.transitions.trim_silence && start.is_zero()).then_some(item.id);
    }

    /// Moves the current item past its leading silence if its `Load` asked for it
    /// and nothing has been played of it yet. Called once its plan is known.
    pub(super) fn trim_leading(&mut self) {
        let Some(id) = self.trim_start.take() else {
            return;
        };
        if self.bit_perfect_exclusive() {
            return;
        }
        if let Some(source) = &mut self.current
            && source.item().id == id
            && source.is_at_start()
            && let Some(start) = head_start(source)
        {
            let rate = source.rate();
            let _ = source.seek(MediaTime::from_frames(start, rate));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use helikodu_model::analysis::SilentSpan;
    use helikodu_model::audio::{AudioSpec, SampleRate};
    use helikodu_model::library::MediaSource;
    use helikodu_model::player::{ItemId, Volume};

    use super::super::timeline::Segment;
    use super::*;
    use crate::decode::DecoderRegistry;
    use crate::pipeline::{LoudnessInfo, PathRequest, ProcessingSettings, VolumePlan};

    const RATE: u32 = 1_000;

    #[test]
    fn rates_convert_down() {
        assert_eq!(at_rate(44_100, 44_100, 48_000), 48_000);
        assert_eq!(at_rate(1, 48_000, 44_100), 0);
        assert_eq!(at_rate(7, 0, 48_000), 7);
    }

    /// The sample `frame` of [`counting_wav`].
    fn counted(frame: u64) -> i16 {
        i16::try_from(frame % 30_000 + 1).unwrap_or(0)
    }

    /// A mono 16-bit WAV file whose samples count up (never silent: the analyses
    /// below are made up).
    fn counting_wav(name: &str, frames: u64) -> PathBuf {
        let data: Vec<u8> = (0..frames).flat_map(|i| counted(i).to_le_bytes()).collect();
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&RATE.to_le_bytes());
        b.extend_from_slice(&(RATE * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(&data);
        let path = std::env::temp_dir().join(format!("helikodu-{}-{name}", std::process::id()));
        std::fs::write(&path, b).unwrap();
        path
    }

    /// The source of `path` with `analysis`, attached to an unchanged path.
    fn source(path: &std::path::Path, analysis: TrackAnalysis) -> Source {
        let item = PlaybackItem::new(ItemId::new(1), MediaSource::file(path))
            .with_analysis(Some(Arc::new(analysis)));
        let mut source = Source::open(&DecoderRegistry::default(), item).unwrap();
        let request = PathRequest {
            source: source.format(),
            loudness: LoudnessInfo::default(),
            album_context: false,
            track_volume_db: 0.0,
            device: source.spec(),
            volume: VolumePlan::Software(Volume::FULL),
            dsp: false,
            crossfade: false,
        };
        source
            .attach(PathPlan::new(&request, &ProcessingSettings::default()))
            .unwrap();
        source
    }

    /// Renders all of `source` in small blocks, recording its jumps in a timeline
    /// segment as the engine does. Returns the samples and the segment.
    fn render_all(source: &mut Source, spec: AudioSpec) -> (Vec<f32>, Segment) {
        let mut segment = Segment {
            ring_start: 0,
            item: source.item().clone(),
            spec,
            length: source.length(),
            start_frames: source.position(),
            scale: crate::pipeline::TimeScale::new(spec.rate, spec.rate),
            error: None,
            mix_from: None,
            rewinds: Vec::new(),
        };
        let mut out = Vec::new();
        let mut block = vec![0.0f32; source.max_output_frames()];
        let mut idle = 0;
        while !source.is_finished() {
            let at = out.len() as u64;
            let n = source.render(&mut block);
            idle = if n == 0 { idle + 1 } else { 0 };
            assert!(idle < 10, "the source renders nothing and does not end");
            for &(offset, frame) in source.wraps() {
                segment.rewind(at + offset as u64, frame);
            }
            out.extend_from_slice(&block[..n]);
        }
        (out, segment)
    }

    #[test]
    fn trims_and_skips_land_on_their_frames_and_keep_file_time() {
        // 40 s at 1 kHz: sound from 2 s to 37 s, silent from 20 s to 32 s.
        let path = counting_wav("skips.wav", 40_000);
        let analysis = TrackAnalysis {
            rate: RATE,
            frames: 40_000,
            audio: Some(2_000..37_000),
            silences: vec![SilentSpan {
                start: 20_000,
                end: 32_000,
            }],
            ..TrackAnalysis::default()
        };
        let transitions = TransitionSettings {
            skip_silence: true,
            skip_silence_min_s: 10,
            ..TransitionSettings::default()
        };
        let mut source = source(&path, analysis);
        let spec = source.spec();
        let skips = skips_for(&transitions, &source);
        assert_eq!(skips, [(21_000, 31_000)]);
        source.set_skips(skips);
        let (head, tail) = (head_start(&source).unwrap(), tail_end(&source).unwrap());
        assert_eq!((head, tail), (1_990, 37_010));
        source
            .seek(MediaTime::from_frames(head, spec.rate))
            .unwrap();
        source.set_end(Some(tail));
        // What is left to play is known exactly, the jump included.
        let expected = (21_000 - 1_990) + (37_010 - 31_000);
        assert_eq!(source.remaining_frames(), Some(expected));

        let (out, segment) = render_all(&mut source, spec);
        let sample = |frame: u64| f32::from(counted(frame)) / 32_768.0;
        assert_eq!(out.len() as u64, expected);
        assert_eq!(out[0], sample(1_990));
        let jump = (21_000 - 1_990) as usize;
        assert_eq!(out[jump - 1], sample(20_999));
        assert_eq!(out[jump], sample(31_000));
        assert_eq!(*out.last().unwrap(), sample(37_009));
        // Positions are file time on both sides of the jump.
        let at = |ring: u64| {
            segment
                .position_at(ring)
                .frames_at(SampleRate::new(RATE).unwrap())
        };
        assert_eq!(at(0), 1_990);
        assert_eq!(at(jump as u64 - 1), 20_999);
        assert_eq!(at(jump as u64), 31_000);
        assert_eq!(at(expected - 1), 37_009);

        // A seek into the stretch plays it from there: nothing to jump over.
        source
            .seek(MediaTime::from_frames(25_000, spec.rate))
            .unwrap();
        assert_eq!(source.remaining_frames(), Some(37_010 - 31_000 + 6_000));
        let (out, _) = render_all(&mut source, spec);
        assert_eq!(out.len(), 37_010 - 25_000);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_end_is_not_moved_once_passed() {
        let path = counting_wav("end.wav", 5_000);
        let analysis = TrackAnalysis {
            rate: RATE,
            frames: 5_000,
            audio: Some(0..3_000),
            ..TrackAnalysis::default()
        };
        let mut source = source(&path, analysis);
        let spec = source.spec();
        let mut block = vec![0.0f32; 4_000];
        assert_eq!(source.render(&mut block), 4_000);
        // Too late to end at 3,010: the rest plays.
        source.set_end(tail_end(&source));
        assert_eq!(source.end(), None);
        let (out, _) = render_all(&mut source, spec);
        assert_eq!(out.len(), 1_000);
        let _ = std::fs::remove_file(path);
    }
}
