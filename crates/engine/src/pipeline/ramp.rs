// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Gain ramps: smoothed parameter changes and anti-click fades.
//!
//! A gain that jumps from one block to the next makes a step in the waveform, which
//! is heard as a click. Every gain the pipeline applies therefore moves to a new value
//! over a few milliseconds, one frame at a time, starting from wherever it is now, so
//! the curve stays continuous even when a ramp is interrupted by the next one.
//!
//! [`SmoothedGain`] moves in a straight line, for volume and loudness changes.
//! [`Fader`] follows a raised cosine, for the fades on start, pause, seek and stop.

use std::f64::consts::PI;
use std::time::Duration;

/// The number of frames `duration` lasts at `rate`, at least one.
pub(crate) fn frames_in(duration: Duration, rate: u32) -> usize {
    let frames = (duration.as_secs_f64() * f64::from(rate)).round();
    // A duration of hours still fits; `as` saturates in any case.
    (frames as usize).max(1)
}

/// A gain that moves to a new value in a straight line over a fixed number of frames.
///
/// The gain is held in 64-bit float, and applied to interleaved `f32` frames. A gain
/// of exactly 1.0 that is not moving leaves the samples untouched.
#[derive(Debug, Clone)]
pub struct SmoothedGain {
    current: f64,
    target: f64,
    step: f64,
    remaining: usize,
    ramp_frames: usize,
}

impl SmoothedGain {
    /// A gain resting at `gain`, that takes `ramp_frames` frames for each change.
    pub fn new(gain: f64, ramp_frames: usize) -> Self {
        Self {
            current: gain,
            target: gain,
            step: 0.0,
            remaining: 0,
            ramp_frames: ramp_frames.max(1),
        }
    }

    /// The gain applied to the next frame.
    pub fn current(&self) -> f64 {
        self.current
    }

    /// The gain the ramp moves to.
    pub fn target(&self) -> f64 {
        self.target
    }

    /// Whether the gain is still moving.
    pub fn is_ramping(&self) -> bool {
        self.remaining > 0
    }

    /// Moves to `gain` over the ramp length, starting from the current gain.
    pub fn set_target(&mut self, gain: f64) {
        if gain == self.target {
            return;
        }
        self.target = gain;
        self.remaining = self.ramp_frames;
        self.step = (gain - self.current) / self.ramp_frames as f64;
    }

    /// Jumps to `gain` at once, without a ramp. For a stream that has not started.
    pub fn jump_to(&mut self, gain: f64) {
        self.current = gain;
        self.target = gain;
        self.remaining = 0;
        self.step = 0.0;
    }

    /// Multiplies the interleaved frames of `block` by the gain, moving it one step
    /// per frame while it ramps.
    pub fn apply(&mut self, block: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let mut frames = block.chunks_exact_mut(channels);
        while self.remaining > 0 {
            let Some(frame) = frames.next() else {
                return;
            };
            self.remaining -= 1;
            self.current = if self.remaining == 0 {
                self.target
            } else {
                self.current + self.step
            };
            scale(frame, self.current);
        }
        if self.current == 1.0 {
            return;
        }
        for frame in frames {
            scale(frame, self.current);
        }
    }
}

fn scale(frame: &mut [f32], gain: f64) {
    for sample in frame {
        *sample = (f64::from(*sample) * gain) as f32;
    }
}

/// Why a fade happens. Each has its own length in [`FadeDurations`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transition {
    /// Playback starts.
    Start,
    /// Playback pauses.
    Pause,
    /// Playback resumes after a pause.
    Resume,
    /// Playback moves to another position: a fade out before, a fade in after.
    Seek,
    /// Playback stops.
    Stop,
    /// Playback moves to another device: a fade out before, a fade in after.
    DeviceSwitch,
}

/// How long the anti-click fade of each [`Transition`] lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FadeDurations {
    /// The fade in when playback starts.
    pub start: Duration,
    /// The fade out before a pause.
    pub pause: Duration,
    /// The fade in after a pause.
    pub resume: Duration,
    /// The fade out and the fade in around a seek.
    pub seek: Duration,
    /// The fade out before playback stops.
    pub stop: Duration,
    /// The fade out and the fade in around a device switch.
    pub device_switch: Duration,
}

impl FadeDurations {
    /// The longest fade allowed.
    pub const MAX: Duration = Duration::from_secs(2);

    /// The length of the fade for `transition`, at most [`MAX`](Self::MAX).
    pub fn of(&self, transition: Transition) -> Duration {
        let duration = match transition {
            Transition::Start => self.start,
            Transition::Pause => self.pause,
            Transition::Resume => self.resume,
            Transition::Seek => self.seek,
            Transition::Stop => self.stop,
            Transition::DeviceSwitch => self.device_switch,
        };
        duration.min(Self::MAX)
    }
}

impl Default for FadeDurations {
    /// Short ramps, just long enough not to click: 10 ms, and 5 ms around a seek.
    fn default() -> Self {
        let short = Duration::from_millis(10);
        Self {
            start: short,
            pause: short,
            resume: short,
            seek: Duration::from_millis(5),
            stop: short,
            device_switch: short,
        }
    }
}

/// Where a [`Fader`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeState {
    /// Silent: every sample becomes zero.
    Silent,
    /// Rising towards full level.
    FadingIn,
    /// At full level: samples pass untouched.
    Open,
    /// Falling towards silence.
    FadingOut,
}

/// Anti-click fades on start, pause, resume, seek, stop and device switch.
///
/// A fade follows a raised cosine from the current level to the new one, so both
/// the level and its slope are continuous at its ends. A fade that interrupts another
/// starts from the level reached so far.
#[derive(Debug, Clone)]
pub struct Fader {
    rate: u32,
    level: f64,
    from: f64,
    to: f64,
    position: usize,
    length: usize,
}

impl Fader {
    /// A fader for a stream at `rate` frames per second that starts silent, or open
    /// when `open` is set.
    pub fn new(rate: u32, open: bool) -> Self {
        let level = if open { 1.0 } else { 0.0 };
        Self {
            rate,
            level,
            from: level,
            to: level,
            position: 0,
            length: 0,
        }
    }

    /// The state of the fader.
    pub fn state(&self) -> FadeState {
        if self.position < self.length {
            if self.to > self.from {
                FadeState::FadingIn
            } else {
                FadeState::FadingOut
            }
        } else if self.level > 0.0 {
            FadeState::Open
        } else {
            FadeState::Silent
        }
    }

    /// Whether the fader has reached silence: a fade out is complete, and the
    /// transport may pause, seek or stop.
    pub fn is_silent(&self) -> bool {
        self.state() == FadeState::Silent
    }

    /// Whether the fader is fully open and leaves samples untouched.
    pub fn is_open(&self) -> bool {
        self.state() == FadeState::Open
    }

    /// The level applied to the next frame, from 0.0 to 1.0.
    pub fn level(&self) -> f64 {
        self.level
    }

    /// The frames left until the current fade ends.
    pub fn remaining_frames(&self) -> usize {
        self.length - self.position
    }

    /// Fades in to full level over `duration`.
    pub fn fade_in(&mut self, duration: Duration) {
        self.fade_to(1.0, duration);
    }

    /// Fades out to silence over `duration`.
    pub fn fade_out(&mut self, duration: Duration) {
        self.fade_to(0.0, duration);
    }

    /// Jumps to full level or silence at once, without a fade.
    pub fn set_open(&mut self, open: bool) {
        let level = if open { 1.0 } else { 0.0 };
        self.level = level;
        self.from = level;
        self.to = level;
        self.position = 0;
        self.length = 0;
    }

    fn fade_to(&mut self, to: f64, duration: Duration) {
        if self.to == to && self.position >= self.length && self.level == to {
            return;
        }
        // A shorter fade over the part of the distance that is left keeps the
        // speed of a fade that interrupts another.
        let full = frames_in(duration, self.rate);
        let distance = (to - self.level).abs();
        self.from = self.level;
        self.to = to;
        self.position = 0;
        self.length = ((full as f64 * distance).ceil() as usize).max(1);
    }

    /// Applies the level to the interleaved frames of `block`.
    pub fn apply(&mut self, block: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let mut frames = block.chunks_exact_mut(channels);
        while self.position < self.length {
            let Some(frame) = frames.next() else {
                return;
            };
            self.position += 1;
            let t = self.position as f64 / self.length as f64;
            let shape = 0.5 - 0.5 * (PI * t).cos();
            self.level = self.from + (self.to - self.from) * shape;
            if self.position == self.length {
                self.level = self.to;
            }
            scale(frame, self.level);
        }
        if self.level == 1.0 {
            return;
        }
        if self.level == 0.0 {
            for frame in frames {
                frame.fill(0.0);
            }
        } else {
            for frame in frames {
                scale(frame, self.level);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The largest change of gain from one frame to the next, measured on a
    /// constant signal of 1.0.
    fn largest_step(apply: &mut dyn FnMut(&mut [f32]), frames: usize) -> f32 {
        let mut block = vec![1.0f32; frames];
        apply(&mut block);
        block
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn smoothed_gain_ramps_linearly_and_lands_exactly() {
        let mut gain = SmoothedGain::new(1.0, 100);
        gain.set_target(0.0);
        let mut block = vec![1.0f32; 150];
        gain.apply(&mut block, 1);
        assert!((block[0] - 0.99).abs() < 1e-6);
        assert!((block[49] - 0.5).abs() < 1e-6);
        assert_eq!(block[99], 0.0);
        assert_eq!(block[149], 0.0);
        assert!(!gain.is_ramping());
    }

    #[test]
    fn smoothed_gain_is_continuous_across_blocks_and_interruptions() {
        let mut gain = SmoothedGain::new(1.0, 480);
        let step = largest_step(
            &mut |block| {
                gain.set_target(0.1);
                for chunk in block.chunks_mut(37) {
                    gain.apply(chunk, 1);
                }
            },
            200,
        );
        assert!(step <= 0.9 / 480.0 + 1e-6, "{step}");
        // Turned back up half way: it starts from where it is.
        let before = gain.current();
        gain.set_target(1.0);
        let mut block = [1.0f32; 1];
        gain.apply(&mut block, 1);
        assert!((f64::from(block[0]) - before).abs() < 0.01);
    }

    #[test]
    fn unity_gain_leaves_samples_untouched() {
        let mut gain = SmoothedGain::new(1.0, 10);
        let mut block = [0.123_456_79f32, -1.0, f32::MIN_POSITIVE];
        let copy = block;
        gain.apply(&mut block, 1);
        assert_eq!(block.map(f32::to_bits), copy.map(f32::to_bits));
    }

    #[test]
    fn fader_fades_out_to_silence_and_back() {
        let mut fader = Fader::new(48_000, true);
        assert!(fader.is_open());
        fader.fade_out(Duration::from_millis(10));
        assert_eq!(fader.state(), FadeState::FadingOut);
        assert_eq!(fader.remaining_frames(), 480);
        let mut block = vec![1.0f32; 2 * 600];
        fader.apply(&mut block, 2);
        assert!(fader.is_silent());
        assert!(block[0] > 0.99 && block[0] == block[1]);
        assert!(block[2 * 480..].iter().all(|&s| s == 0.0));
        let step = block
            .chunks(2)
            .map(|f| f[0])
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        // A raised cosine of n frames has a largest step of about pi / (2 n).
        assert!(step < 1.6 / 480.0, "{step}");

        fader.fade_in(Duration::from_millis(5));
        assert_eq!(fader.state(), FadeState::FadingIn);
        let mut block = vec![1.0f32; 300];
        fader.apply(&mut block, 1);
        assert!(fader.is_open());
        assert!(block[0] < 0.01);
        assert_eq!(block[299], 1.0);
    }

    #[test]
    fn interrupted_fade_starts_from_the_current_level() {
        let mut fader = Fader::new(1000, false);
        fader.fade_in(Duration::from_millis(100));
        let mut block = vec![1.0f32; 50];
        fader.apply(&mut block, 1);
        let reached = fader.level();
        assert!(reached > 0.4 && reached < 0.6, "{reached}");
        fader.fade_out(Duration::from_millis(100));
        let mut next = [1.0f32; 1];
        fader.apply(&mut next, 1);
        assert!((f64::from(next[0]) - reached).abs() < 0.05);
        // Half the distance left: half the length.
        assert!(fader.remaining_frames() < 50);
    }

    #[test]
    fn default_durations_are_short() {
        let durations = FadeDurations::default();
        assert_eq!(durations.of(Transition::Pause), Duration::from_millis(10));
        assert_eq!(durations.of(Transition::Seek), Duration::from_millis(5));
        let long = FadeDurations {
            stop: Duration::from_secs(10),
            ..durations
        };
        assert_eq!(long.of(Transition::Stop), FadeDurations::MAX);
    }
}
