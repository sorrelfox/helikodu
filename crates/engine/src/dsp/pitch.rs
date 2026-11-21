// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pitch shift that keeps the tempo: the audio is stretched in time by the pitch
//! ratio with [`Wsola`], then read back faster or slower by the same ratio with
//! cubic interpolation, so that its length is what it was and its pitch is moved.
//!
//! The stage delays the audio by a fixed number of frames (about 33 ms), enough
//! that the stretcher always has output ready, whatever the ratio.

use helikodu_model::settings::PitchSettings;

use super::DspStage;
use super::plan::PitchPlan;
use super::wsola::Wsola;

/// Frames handed to the stretcher at once.
const CHUNK: usize = 256;

/// The pitch shifter for one rate and channel count.
#[derive(Debug, Clone)]
pub struct PitchShifter {
    plan: PitchPlan,
    channels: usize,
    ratio: f64,
    wsola: Wsola,
    /// The stretched audio, a ring of whole frames with absolute positions.
    fifo: Box<[f32]>,
    capacity: usize,
    written: u64,
    /// Where the next output frame is read, in stretched frames.
    read: f64,
    latency: usize,
    /// Input frames taken so far, up to the latency.
    seen: usize,
    hop: Box<[f32]>,
}

impl PitchShifter {
    /// The pitch shift `settings` describe, for `channels` channels at `rate` Hz.
    pub fn new(settings: &PitchSettings, rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        let semitones = if settings.semitones.is_finite() {
            settings.semitones.clamp(-24.0, 24.0)
        } else {
            0.0
        };
        let ratio = (semitones / 12.0).exp2();
        let rate_hz = f64::from(rate.max(1));
        let hop = ((rate_hz * 0.010).round() as usize).max(16);
        let tolerance = ((rate_hz * 0.003).round() as usize).max(4);
        let latency = tolerance + 3 * hop + 8;
        let wsola = Wsola::new(channels, hop, tolerance, ratio, CHUNK);
        let capacity = (4 * (latency + 2 * hop + CHUNK)).next_power_of_two();
        Self {
            plan: PitchPlan {
                semitones,
                latency_ms: latency as f64 * 1000.0 / rate_hz,
            },
            channels,
            ratio,
            wsola,
            fifo: vec![0.0; capacity * channels].into_boxed_slice(),
            capacity,
            written: 0,
            read: 1.0,
            latency,
            seen: 0,
            hop: vec![0.0; hop * channels].into_boxed_slice(),
        }
    }

    /// Whether the shift leaves the pitch where it is.
    pub fn is_identity(&self) -> bool {
        self.ratio == 1.0
    }

    /// What the signal path shows.
    pub fn plan(&self) -> PitchPlan {
        self.plan
    }

    fn at(&self, frame: u64, channel: usize) -> f32 {
        let slot = (frame as usize) & (self.capacity - 1);
        self.fifo[slot * self.channels + channel]
    }

    fn take_stretched(&mut self) {
        let channels = self.channels;
        let hop = self.wsola.hop();
        while self.wsola.pull(&mut self.hop) {
            for i in 0..hop {
                let slot = (self.written as usize) & (self.capacity - 1);
                self.fifo[slot * channels..(slot + 1) * channels]
                    .copy_from_slice(&self.hop[i * channels..(i + 1) * channels]);
                self.written += 1;
            }
        }
    }

    fn emit(&mut self, frame: &mut [f32]) {
        if self.seen < self.latency {
            self.seen += 1;
            frame.fill(0.0);
            return;
        }
        let i = self.read.floor();
        let t = (self.read - i) as f32;
        let i = i as u64;
        if i + 2 >= self.written || i == 0 {
            // Never happens with the latency above; silence is the safe answer.
            frame.fill(0.0);
        } else {
            for (c, out) in frame.iter_mut().enumerate() {
                let (y0, y1, y2, y3) = (
                    self.at(i - 1, c),
                    self.at(i, c),
                    self.at(i + 1, c),
                    self.at(i + 2, c),
                );
                // Catmull-Rom.
                let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
                let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
                let c1 = -0.5 * y0 + 0.5 * y2;
                *out = ((a * t + b) * t + c1) * t + y1;
            }
        }
        self.read += self.ratio;
    }
}

impl DspStage for PitchShifter {
    fn name(&self) -> &'static str {
        "pitch"
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn process(&mut self, block: &mut [f32]) {
        let channels = self.channels;
        for chunk in block.chunks_mut(CHUNK * channels) {
            self.wsola.push(chunk);
            self.take_stretched();
            for frame in chunk.chunks_exact_mut(channels) {
                self.emit(frame);
            }
        }
    }

    fn reset(&mut self) {
        self.wsola.reset();
        self.fifo.fill(0.0);
        self.written = 0;
        self.read = 1.0;
        self.seen = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn tone(hz: f32, frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels)
            .map(|i| {
                let n = (i / channels) as f32;
                (2.0 * std::f32::consts::PI * hz * n / RATE as f32).sin() * 0.5
            })
            .collect()
    }

    /// The frequency of a tone, by its zero crossings.
    fn frequency(x: &[f32]) -> f64 {
        let crossings = x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        crossings as f64 * f64::from(RATE) / x.len() as f64
    }

    #[test]
    fn an_octave_up_doubles_the_frequency_and_keeps_the_length() {
        for (semitones, expected) in [(12.0, 880.0), (-12.0, 220.0), (7.0, 659.26), (-3.0, 370.0)] {
            let settings = PitchSettings {
                enabled: true,
                semitones,
            };
            let mut stage = PitchShifter::new(&settings, RATE, 2);
            let mut block = tone(440.0, 48_000, 2);
            for chunk in block.chunks_mut(2 * 1000) {
                stage.process(chunk);
            }
            let latency = stage.latency();
            assert!(latency < 2000, "{latency}");
            assert!(block[..2 * (latency - 1)].iter().all(|s| *s == 0.0));
            let left: Vec<f32> = block.iter().step_by(2).copied().collect();
            let right: Vec<f32> = block.iter().skip(1).step_by(2).copied().collect();
            assert_eq!(left, right);
            let steady = &left[latency + 4800..];
            let f = frequency(steady);
            assert!((f - expected).abs() < expected * 0.01, "{semitones}: {f}");
            let peak = steady.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!((0.4..0.6).contains(&peak), "{semitones}: peak {peak}");
        }
    }

    #[test]
    fn no_shift_is_identity_and_silence_stays_clean() {
        assert!(PitchShifter::new(&PitchSettings::default(), RATE, 2).is_identity());
        let settings = PitchSettings {
            enabled: true,
            semitones: 5.0,
        };
        let mut stage = PitchShifter::new(&settings, RATE, 1);
        let mut block = tone(300.0, 9600, 1);
        stage.process(&mut block);
        let mut quiet = vec![0.0f32; 48_000];
        stage.process(&mut quiet);
        assert!(quiet[10_000..].iter().all(|s| *s == 0.0));
        stage.reset();
        let mut block = vec![0.25f32; 4000];
        stage.process(&mut block);
        assert!(block.iter().all(|s| s.is_finite()));
    }
}
