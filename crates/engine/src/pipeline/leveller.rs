// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The leveller: a slow gain that evens out the loudness of a track without
//! loudness values, toward the loudness tracks with values are played at.
//!
//! - **Target.** The ReplayGain 2.0 reference, −18 LUFS, plus the preamp of files
//!   with loudness values, so that levelled tracks sit with the others.
//! - **What it listens to.** With a track analysis, its loudness envelope (one value
//!   per second) from [`LOOK_BEHIND`] before the frame being played to
//!   [`LOOK_AHEAD`] after it: the gain begins to fall before a loud passage
//!   arrives. Without one, the K-weighted loudness (BS.1770) of the last
//!   [`SHORT_TERM`] of the audio itself, measured as it passes. Seconds quieter
//!   than [`GATE_LUFS`] (silence, the space between movements) do not count, and
//!   with nothing louder in the window the gain holds.
//! - **How it moves.** Every [`STEP`] the gain takes one step toward the target,
//!   at most [`RISE_DB_PER_S`] up and [`FALL_DB_PER_S`] down, and not at all while
//!   it is within [`DEADBAND_DB`] of it, so that it does not hunt. Between steps it
//!   moves in a straight line, frame by frame. The gain stays within
//!   ±[`MAX_GAIN_DB`].
//! - **Clipping.** When the loudness settings prevent clipping by lowering the
//!   gain, the gain never raises the highest sample seen so far above
//!   [`CEILING`], and a block that would go above it is played at the gain that
//!   keeps it there. With the limiter chosen instead, peaks are left to it.
//!
//! With an analysis the first step goes straight to the gain of the start of the
//! track; without one the gain starts at 0 dB and moves from there.

use std::sync::Arc;
use std::time::Duration;

use helikodu_model::analysis::TrackAnalysis;

use super::loudness::{db_to_linear, linear_to_db};

/// How often the gain takes a step.
pub const STEP: Duration = Duration::from_millis(100);
/// The window of live loudness without an analysis.
pub const SHORT_TERM: Duration = Duration::from_secs(3);
/// How far before the frame being played the envelope is read.
pub const LOOK_BEHIND: Duration = Duration::from_secs(1);
/// How far after the frame being played the envelope is read.
pub const LOOK_AHEAD: Duration = Duration::from_secs(4);
/// The quietest loudness that moves the gain, in LUFS.
pub const GATE_LUFS: f64 = -50.0;
/// The largest gain up or down, in dB.
pub const MAX_GAIN_DB: f64 = 12.0;
/// How fast the gain rises, in dB per second.
pub const RISE_DB_PER_S: f64 = 1.0;
/// How fast the gain falls, in dB per second.
pub const FALL_DB_PER_S: f64 = 3.0;
/// How far the gain may be from its target before it moves, in dB.
pub const DEADBAND_DB: f64 = 0.5;
/// The highest sample level the leveller lets the loudest sample reach.
pub const CEILING: f64 = 0.989;
/// How long the gain takes back to 0 dB when the leveller is switched off.
pub const RELEASE: Duration = Duration::from_millis(50);

/// The short-term window in steps.
const HISTORY: usize = (SHORT_TERM.as_millis() / STEP.as_millis()) as usize;
/// The steps of live loudness needed before the gain moves.
const MIN_HISTORY: usize = 10;
/// The largest number of frames the gain is applied to at one gain value check.
const CHUNK: usize = 512;

/// What the leveller levels to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevellerPlan {
    /// The loudness it brings the track to, in LUFS.
    pub target_lufs: f64,
    /// Whether it keeps the loudest sample seen below [`CEILING`] (the settings
    /// lower the gain to prevent clipping) or leaves peaks to the limiter.
    pub peak_safe: bool,
}

/// A biquad in direct form II transposed, 64-bit.
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The two filters of the BS.1770 K-weighting at `rate`: the high shelf and the
/// RLB high pass, for any rate.
fn k_weighting(rate: f64) -> [Biquad; 2] {
    use std::f64::consts::PI;
    let (f0, gain_db, q) = (
        1_681.974_450_955_533,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (PI * f0 / rate).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Biquad {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    [shelf, high_pass]
}

/// The loudness of a mean square energy (summed over channels), in LUFS.
fn lufs_of(energy: f64) -> f64 {
    -0.691 + 10.0 * energy.log10()
}

/// The energy of a loudness in LUFS.
fn energy_of(lufs: f64) -> f64 {
    10f64.powf((lufs + 0.691) / 10.0)
}

/// The leveller for one source, on interleaved frames at the source rate.
#[derive(Debug, Clone)]
pub struct Leveller {
    plan: LevellerPlan,
    rate: u32,
    channels: usize,
    filters: Box<[[Biquad; 2]]>,
    analysis: Option<Arc<TrackAnalysis>>,
    /// The source frame the next frame given is.
    position: u64,
    step_frames: u64,
    step_fill: u64,
    step_energy: f64,
    history: [f64; HISTORY],
    history_len: usize,
    history_at: usize,
    /// The gain the current step moves to, in dB, and the linear gain now.
    gain_db: f64,
    gain: f64,
    /// The linear change per frame, and the frames it still applies to.
    slope: f64,
    sloping: u64,
    peak: f64,
    /// Nothing has been levelled yet: the first step goes straight to its gain.
    fresh: bool,
    /// Switched off: moving back to 0 dB, then transparent.
    released: bool,
}

impl Leveller {
    /// A leveller for `channels` channels at `rate`, reading the loudness envelope
    /// of `analysis` when there is one.
    pub fn new(
        plan: LevellerPlan,
        rate: u32,
        channels: usize,
        analysis: Option<Arc<TrackAnalysis>>,
    ) -> Self {
        let rate = rate.max(1);
        let channels = channels.max(1);
        let analysis = analysis.filter(|a| !a.envelope.is_empty());
        Self {
            plan,
            rate,
            channels,
            filters: vec![k_weighting(f64::from(rate)); channels].into_boxed_slice(),
            analysis,
            position: 0,
            step_frames: frames(STEP, rate),
            step_fill: 0,
            step_energy: 0.0,
            history: [0.0; HISTORY],
            history_len: 0,
            history_at: 0,
            gain_db: 0.0,
            gain: 1.0,
            slope: 0.0,
            sloping: 0,
            peak: 0.0,
            fresh: true,
            released: false,
        }
    }

    /// The gain it applies now, in dB.
    pub fn gain_db(&self) -> f64 {
        linear_to_db(self.gain)
    }

    /// Whether it reads an analysis envelope.
    pub fn has_envelope(&self) -> bool {
        self.analysis.is_some()
    }

    /// Changes the target and the clipping rule; the gain moves on from where it
    /// is. A released leveller levels again.
    pub fn set_plan(&mut self, plan: LevellerPlan) {
        self.plan = plan;
        self.released = false;
    }

    /// Moves back to 0 dB over [`RELEASE`] and stays there.
    pub fn release(&mut self) {
        self.released = true;
        self.gain_db = 0.0;
        self.sloping = frames(RELEASE, self.rate);
        self.slope = (1.0 - self.gain) / self.sloping as f64;
    }

    /// The next frame given is the source frame `frame` (after a seek or a jump).
    /// The gain moves on from where it is; the live loudness starts anew.
    pub fn locate(&mut self, frame: u64) {
        if frame == self.position {
            return;
        }
        self.position = frame;
        self.step_fill = 0;
        self.step_energy = 0.0;
        self.history_len = 0;
        for filters in &mut self.filters {
            for filter in filters {
                filter.z = [0.0; 2];
            }
        }
    }

    /// Levels the interleaved frames of `block`.
    pub fn apply(&mut self, block: &mut [f32]) {
        let channels = self.channels;
        let mut offset = 0;
        let frames = block.len() / channels;
        while offset < frames {
            if self.step_fill == 0 {
                self.plan_step();
            }
            let left = usize::try_from(self.step_frames - self.step_fill).unwrap_or(usize::MAX);
            let n = (frames - offset).min(left).min(CHUNK);
            let chunk = &mut block[offset * channels..(offset + n) * channels];
            let peak = self.measure(chunk);
            self.keep_below_ceiling(peak, n);
            self.scale(chunk);
            offset += n;
            self.position += n as u64;
            self.step_fill += n as u64;
            if self.step_fill >= self.step_frames {
                self.end_step();
            }
        }
    }

    /// Adds the K-weighted energy of `chunk` to the step's, and returns its peak.
    fn measure(&mut self, chunk: &[f32]) -> f64 {
        let mut peak = 0.0f64;
        for frame in chunk.chunks_exact(self.channels) {
            for (x, filters) in frame.iter().zip(self.filters.iter_mut()) {
                let x = f64::from(*x);
                peak = peak.max(x.abs());
                let shelved = filters[0].run(x);
                let y = filters[1].run(shelved);
                self.step_energy += y * y;
            }
        }
        self.peak = self.peak.max(peak);
        peak
    }

    /// Lowers the gain at once when the next `frames` frames would take the
    /// loudest sample above the ceiling.
    fn keep_below_ceiling(&mut self, peak: f64, frames: usize) {
        if !self.plan.peak_safe || self.released || peak <= 0.0 {
            return;
        }
        let most = self
            .gain
            .max(self.gain + self.slope * frames.min(self.sloping as usize) as f64);
        let limit = CEILING / peak;
        if most > limit {
            self.gain = self.gain.min(limit);
            self.gain_db = self.gain_db.min(linear_to_db(limit));
            let left = self.step_frames.saturating_sub(self.step_fill).max(1);
            self.sloping = left;
            self.slope = (db_to_linear(self.gain_db) - self.gain) / left as f64;
        }
    }

    fn scale(&mut self, chunk: &mut [f32]) {
        if self.gain == 1.0 && self.sloping == 0 {
            return;
        }
        for frame in chunk.chunks_exact_mut(self.channels) {
            if self.sloping > 0 {
                self.sloping -= 1;
                self.gain += self.slope;
                if self.sloping == 0 {
                    self.gain = db_to_linear(self.gain_db);
                }
            }
            for sample in frame {
                *sample = (f64::from(*sample) * self.gain) as f32;
            }
        }
    }

    /// The step that ends: its loudness goes into the short-term history.
    fn end_step(&mut self) {
        let mean = self.step_energy / self.step_frames as f64;
        self.history[self.history_at] = mean;
        self.history_at = (self.history_at + 1) % HISTORY;
        self.history_len = (self.history_len + 1).min(HISTORY);
        self.step_fill = 0;
        self.step_energy = 0.0;
    }

    /// Decides where the gain moves over the next step.
    fn plan_step(&mut self) {
        if self.released {
            return;
        }
        let Some(loudness) = self.loudness() else {
            return;
        };
        let mut wanted = (self.plan.target_lufs - loudness).clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
        if self.plan.peak_safe && self.peak > 0.0 {
            wanted = wanted.min(linear_to_db(CEILING / self.peak));
        }
        let now = self.gain_db;
        if self.fresh {
            self.fresh = false;
            if self.analysis.is_some() {
                self.gain_db = wanted;
                self.gain = db_to_linear(wanted);
                self.sloping = 0;
                return;
            }
        }
        let difference = wanted - now;
        if difference.abs() < DEADBAND_DB {
            return;
        }
        let seconds = STEP.as_secs_f64();
        let next = now + difference.clamp(-FALL_DB_PER_S * seconds, RISE_DB_PER_S * seconds);
        self.gain_db = next;
        self.sloping = self.step_frames;
        self.slope = (db_to_linear(next) - self.gain) / self.step_frames as f64;
    }

    /// The loudness the gain follows now, in LUFS; `None` to hold.
    fn loudness(&self) -> Option<f64> {
        match &self.analysis {
            Some(analysis) => {
                let envelope = &analysis.envelope;
                let step = u64::from(envelope.step_secs()) * u64::from(self.rate);
                let index = self.position / step.max(1);
                let behind = LOOK_BEHIND.as_secs() / u64::from(envelope.step_secs());
                let ahead = LOOK_AHEAD
                    .as_secs()
                    .div_ceil(u64::from(envelope.step_secs()));
                let from = index.saturating_sub(behind);
                let (sum, count) = (from..=index + ahead)
                    .filter_map(|i| envelope.lufs(usize::try_from(i).ok()?))
                    .filter(|lufs| *lufs >= GATE_LUFS)
                    .fold((0.0, 0u32), |(sum, n), lufs| (sum + energy_of(lufs), n + 1));
                (count > 0).then(|| lufs_of(sum / f64::from(count)))
            }
            None => {
                if self.history_len < MIN_HISTORY {
                    return None;
                }
                // Gated as the envelope is: quiet steps do not count.
                let gate = energy_of(GATE_LUFS);
                let (sum, count) = self.history[..self.history_len]
                    .iter()
                    .filter(|&&energy| energy >= gate)
                    .fold((0.0, 0u32), |(sum, n), energy| (sum + energy, n + 1));
                (count > 0).then(|| lufs_of(sum / f64::from(count)))
            }
        }
    }
}

/// The frames `duration` lasts at `rate`, at least one.
fn frames(duration: Duration, rate: u32) -> u64 {
    ((duration.as_secs_f64() * f64::from(rate)).round() as u64).max(1)
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use helikodu_model::analysis::LoudnessEnvelope;

    use super::*;

    const RATE: u32 = 48_000;

    fn plan() -> LevellerPlan {
        LevellerPlan {
            target_lufs: -18.0,
            peak_safe: true,
        }
    }

    /// A 1 kHz stereo sine whose amplitude `amplitude(t)` changes over time.
    fn sine(seconds: f64, amplitude: impl Fn(f64) -> f64) -> Vec<f32> {
        let frames = (seconds * f64::from(RATE)) as usize;
        let mut out = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f64 / f64::from(RATE);
            let x = (amplitude(t) * (TAU * 1000.0 * t).sin()) as f32;
            out.extend_from_slice(&[x, x]);
        }
        out
    }

    /// The loudness of a stereo 1 kHz sine of `amplitude`, in LUFS: the gain of
    /// the K-weighting at 1 kHz makes up for the −0.691 of the formula.
    fn sine_lufs(amplitude: f64) -> f64 {
        20.0 * amplitude.log10()
    }

    /// Runs `signal` through `leveller` in blocks of 1000 frames and returns the
    /// gain after each block, in dB.
    fn run(leveller: &mut Leveller, signal: &mut [f32]) -> Vec<f64> {
        signal
            .chunks_mut(2000)
            .map(|block| {
                leveller.apply(block);
                leveller.gain_db()
            })
            .collect()
    }

    #[test]
    fn k_weighting_is_near_unity_at_1_khz() {
        let mut filters = k_weighting(f64::from(RATE));
        let mut energy = 0.0;
        let n = RATE as usize;
        for i in 0..n {
            let x = (TAU * 1000.0 * i as f64 / f64::from(RATE)).sin();
            let shelved = filters[0].run(x);
            let y = filters[1].run(shelved);
            if i >= n / 2 {
                energy += y * y;
            }
        }
        let gain_db = 10.0 * (energy / (n / 2) as f64 / 0.5).log10();
        assert!((gain_db - 0.69).abs() < 0.2, "{gain_db}");
    }

    #[test]
    fn a_quiet_track_rises_to_the_target_and_no_further() {
        // −34 LUFS or so: 16 dB below the target, more than the leveller gives.
        let amplitude = 0.02;
        let mut signal = sine(30.0, |_| amplitude);
        let mut leveller = Leveller::new(plan(), RATE, 2, None);
        let gains = run(&mut leveller, &mut signal);
        let last = *gains.last().unwrap();
        assert!(last > MAX_GAIN_DB - DEADBAND_DB, "{last}");
        // It rose at its pace: not more than 1 dB a second.
        let per_block = RISE_DB_PER_S * 1000.0 / f64::from(RATE) + 0.05;
        for pair in gains.windows(2) {
            assert!(pair[1] - pair[0] <= per_block, "{pair:?}");
        }
        assert!(gains.iter().all(|g| *g <= MAX_GAIN_DB + 1e-9));
        // A level that is reachable is reached, within the dead band.
        let amplitude = 0.1;
        let wanted = -18.0 - sine_lufs(amplitude);
        let mut signal = sine(30.0, |_| amplitude);
        let mut leveller = Leveller::new(plan(), RATE, 2, None);
        let last = *run(&mut leveller, &mut signal).last().unwrap();
        assert!(
            (last - wanted).abs() < DEADBAND_DB + 0.3,
            "{last} for {wanted}"
        );
    }

    #[test]
    fn a_loud_track_falls_and_stays_bounded() {
        let mut signal = sine(20.0, |_| 0.4);
        let mut leveller = Leveller::new(plan(), RATE, 2, None);
        let gains = run(&mut leveller, &mut signal);
        let wanted = -18.0 - sine_lufs(0.4);
        let last = *gains.last().unwrap();
        assert!(
            wanted < -9.0 && (last - wanted).abs() < DEADBAND_DB + 0.3,
            "{last}"
        );
        // Louder still: the cut stops at its bound.
        let mut signal = sine(20.0, |_| 0.9);
        let mut leveller = Leveller::new(plan(), RATE, 2, None);
        let gains = run(&mut leveller, &mut signal);
        assert!(*gains.last().unwrap() < -MAX_GAIN_DB + DEADBAND_DB);
        assert!(gains.iter().all(|g| *g >= -MAX_GAIN_DB - 1e-9));
        // The output never went above the ceiling.
        assert!(signal.iter().all(|x| f64::from(x.abs()) <= CEILING + 1e-6));
    }

    #[test]
    fn a_boost_never_takes_the_loudest_sample_over_the_ceiling() {
        // Quiet, with a peak near full scale every 2 s: the boost stops below it.
        let mut signal = sine(20.0, |t| if (t / 2.0).fract() < 0.01 { 0.8 } else { 0.02 });
        let mut leveller = Leveller::new(plan(), RATE, 2, None);
        let gains = run(&mut leveller, &mut signal);
        assert!(signal.iter().all(|x| f64::from(x.abs()) <= CEILING + 1e-6));
        let most = gains.iter().copied().fold(f64::MIN, f64::max);
        assert!(most <= linear_to_db(CEILING / 0.8) + 1e-6, "{most}");
    }

    #[test]
    fn alternating_loud_and_quiet_beats_do_not_pump() {
        // 250 ms loud, 250 ms 20 dB quieter, for 40 s.
        let mut signal = sine(40.0, |t| if (t * 2.0).fract() < 0.5 { 0.2 } else { 0.02 });
        let mut leveller = Leveller::new(plan(), RATE, 2, None);
        let gains = run(&mut leveller, &mut signal);
        // After it has settled (20 s), the gain barely moves within each beat.
        let settled = &gains[gains.len() / 2..];
        let (low, high) = settled
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, h), g| (l.min(*g), h.max(*g)));
        assert!(high - low <= 1.0, "the gain swings {low} .. {high}");
    }

    #[test]
    fn silence_holds_the_gain() {
        let mut signal = sine(20.0, |t| if t < 10.0 { 0.05 } else { 0.0 });
        let mut leveller = Leveller::new(plan(), RATE, 2, None);
        let gains = run(&mut leveller, &mut signal);
        // Once the last sound has left the short-term window, the gain stays.
        let after = gains[(14.0 * f64::from(RATE) / 1000.0) as usize];
        let last = *gains.last().unwrap();
        assert!((last - after).abs() < 1e-9, "{after} then {last}");
        assert!(last < MAX_GAIN_DB - DEADBAND_DB, "{last}");
    }

    #[test]
    fn the_envelope_looks_ahead() {
        // 20 s quiet, then 20 s loud: the envelope says so.
        let quiet = sine_lufs(0.02);
        let loud = sine_lufs(0.3);
        let analysis = TrackAnalysis {
            rate: RATE,
            frames: 40 * u64::from(RATE),
            envelope: LoudnessEnvelope::from_lufs(
                1,
                (0..40).map(|s| if s < 20 { quiet } else { loud }),
            ),
            ..TrackAnalysis::default()
        };
        let mut signal = sine(40.0, |t| if t < 20.0 { 0.02 } else { 0.3 });
        let mut leveller = Leveller::new(
            LevellerPlan {
                peak_safe: false,
                ..plan()
            },
            RATE,
            2,
            Some(Arc::new(analysis)),
        );
        let gains = run(&mut leveller, &mut signal);
        let at = |s: f64| gains[(s * f64::from(RATE) / 1000.0) as usize];
        // The first step goes straight to the gain of the start.
        assert!((at(0.1) - MAX_GAIN_DB).abs() < 0.01, "{}", at(0.1));
        // The gain is falling before the loud part arrives.
        assert!(at(19.9) < MAX_GAIN_DB - 2.0, "{}", at(19.9));
        let wanted = -18.0 - loud;
        assert!(
            (at(39.0) - wanted).abs() < DEADBAND_DB + 0.3,
            "{}",
            at(39.0)
        );
    }

    #[test]
    fn a_release_goes_back_to_unity() {
        let mut signal = sine(10.0, |_| 0.02);
        let mut leveller = Leveller::new(plan(), RATE, 2, None);
        run(&mut leveller, &mut signal);
        assert!(leveller.gain_db() > 3.0);
        leveller.release();
        let mut more = sine(1.0, |_| 0.02);
        run(&mut leveller, &mut more);
        assert!(leveller.gain_db().abs() < 1e-9);
        assert_eq!(
            &more[more.len() - 2..],
            &sine(1.0, |_| 0.02)[more.len() - 2..]
        );
    }
}
