// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Time stretching by WSOLA (waveform-similarity overlap-add), written for this
//! project.
//!
//! The output is built from Hann-windowed frames of `2H` frames, overlapped by
//! half (the windows then sum to one). Frame `k` is placed at `k · H` in the output
//! and taken from around `k · H / stretch` in the input: within a tolerance of
//! `±Δ` frames, the stretcher takes the input segment most similar to the natural
//! continuation of the segment it took before (normalized cross-correlation over the
//! overlap, on the channels' sum). The waveforms therefore line up where frames
//! overlap, and the sound keeps its pitch while its length changes by `stretch`.
//!
//! The channels are linked: every channel takes the same segment, so the stereo
//! image stays. The stretcher allocates when it is built and never afterwards.
//!
//! The ends of a stream keep their level, so that streams played one after the
//! other join without a dip: the first frame has no frame before it to overlap
//! with, so its first half is taken as it is instead of faded in; after
//! [`finish`](Wsola::finish), the last frames are taken from within the input
//! instead of from the silence after it.

/// The streaming time stretcher.
#[derive(Debug, Clone)]
pub struct Wsola {
    channels: usize,
    /// Half the frame length: the output hop.
    hop: usize,
    /// The search tolerance, in frames on either side.
    tolerance: usize,
    stretch: f64,
    window: Box<[f32]>,
    /// The input, a ring of whole frames with absolute positions.
    input: Box<[f32]>,
    capacity: usize,
    pushed: u64,
    /// Where frame `k` is taken from nominally, in input frames.
    nominal: f64,
    /// Where the previous frame was taken from.
    previous: Option<u64>,
    /// The overlap-add accumulator, `2H` frames.
    sum: Box<[f32]>,
    /// The channels' sum of the target and of a candidate, for the search.
    target: Box<[f32]>,
    /// The input has ended after this many frames: no more is pushed, and the
    /// frames are taken from within it.
    end: Option<u64>,
}

impl Wsola {
    /// A stretcher for `channels` channels with an output hop of `hop` frames and a
    /// tolerance of `tolerance` frames, that makes the input `stretch` times as
    /// long, taking at most `chunk` frames per [`push`](Self::push).
    pub fn new(channels: usize, hop: usize, tolerance: usize, stretch: f64, chunk: usize) -> Self {
        let channels = channels.max(1);
        let hop = hop.max(8);
        let length = 2 * hop;
        let stretch = if stretch.is_finite() {
            stretch.clamp(0.25, 4.0)
        } else {
            1.0
        };
        // From the oldest frame still needed to the newest one pushed.
        let analysis = (hop as f64 / stretch).ceil() as usize;
        let capacity =
            (4 * tolerance + 2 * length + analysis + hop + chunk + 8).next_power_of_two();
        let window = (0..length)
            .map(|i| {
                let x = std::f64::consts::PI * i as f64 / length as f64;
                (x.sin() * x.sin()) as f32
            })
            .collect();
        Self {
            channels,
            hop,
            tolerance,
            stretch,
            window,
            input: vec![0.0; capacity * channels].into_boxed_slice(),
            capacity,
            pushed: 0,
            nominal: 0.0,
            previous: None,
            sum: vec![0.0; length * channels].into_boxed_slice(),
            target: vec![0.0; 2 * hop].into_boxed_slice(),
            end: None,
        }
    }

    /// The output hop: [`pull`](Self::pull) writes this many frames.
    pub fn hop(&self) -> usize {
        self.hop
    }

    /// How much longer the output is than the input.
    pub fn stretch(&self) -> f64 {
        self.stretch
    }

    /// Takes interleaved input frames.
    pub fn push(&mut self, frames: &[f32]) {
        let channels = self.channels;
        for frame in frames.chunks_exact(channels) {
            let slot = (self.pushed as usize) & (self.capacity - 1);
            self.input[slot * channels..(slot + 1) * channels].copy_from_slice(frame);
            self.pushed += 1;
        }
    }

    fn sample(&self, frame: u64, channel: usize) -> f32 {
        let slot = (frame as usize) & (self.capacity - 1);
        self.input[slot * self.channels + channel]
    }

    fn mono(&self, frame: u64) -> f32 {
        (0..self.channels).map(|c| self.sample(frame, c)).sum()
    }

    /// The input frames the next output frame needs, up to (not including) this
    /// position.
    fn needed(&self) -> u64 {
        let length = 2 * self.hop as u64;
        let search = self.nominal.floor() as u64 + self.tolerance as u64 + length;
        match self.previous {
            Some(previous) => search.max(previous + self.hop as u64 + length),
            None => search,
        }
    }

    /// The end of the input: what was pushed is all there is. From now on
    /// [`pull`](Self::pull) always writes, taking every frame from within the
    /// input (but for an input shorter than a frame, which is followed by
    /// silence). Nothing more may be pushed until [`reset`](Self::reset).
    pub fn finish(&mut self) {
        if self.end.is_some() {
            return;
        }
        self.end = Some(self.pushed);
        // Silence after the end, for an input too short to stay within: these
        // slots held frames older than any still needed.
        let after = 4 * self.hop + 2 * self.tolerance;
        for frame in self.pushed..self.pushed + after as u64 {
            let slot = (frame as usize) & (self.capacity - 1);
            self.input[slot * self.channels..(slot + 1) * self.channels].fill(0.0);
        }
    }

    /// Writes the next `H` output frames into `out` when enough input has been
    /// pushed (always after [`finish`](Self::finish)), and returns whether it did.
    pub fn pull(&mut self, out: &mut [f32]) -> bool {
        if self.end.is_none() && self.pushed < self.needed() {
            return false;
        }
        let channels = self.channels;
        let hop = self.hop;
        let length = 2 * hop;
        let nominal = self.nominal.floor() as u64;
        // After the end, no frame reaches past the input.
        let last = self.end.map(|end| end.saturating_sub(length as u64));
        let start = match (self.previous, last) {
            (None, None) => nominal,
            (None, Some(last)) => nominal.min(last),
            (Some(previous), _) => self.best_match(nominal, previous + hop as u64, last),
        };
        let first = self.previous.is_none();
        for i in 0..length {
            // The first frame has none before it to overlap with: its first half
            // is taken as it is.
            let w = if first && i < hop {
                1.0
            } else {
                self.window[i]
            };
            for c in 0..channels {
                self.sum[i * channels + c] += w * self.sample(start + i as u64, c);
            }
        }
        out[..hop * channels].copy_from_slice(&self.sum[..hop * channels]);
        self.sum.copy_within(hop * channels.., 0);
        self.sum[hop * channels..].fill(0.0);
        for sample in &mut out[..hop * channels] {
            if sample.abs() < 1e-30 {
                *sample = 0.0;
            }
        }
        self.previous = Some(start);
        self.nominal += hop as f64 / self.stretch;
        true
    }

    /// The start within `±Δ` of `nominal` whose first `H` frames are most like
    /// those at `continuation`; at most `last` when given (the search range then
    /// moves down to end there).
    fn best_match(&mut self, nominal: u64, continuation: u64, last: Option<u64>) -> u64 {
        let hop = self.hop;
        for i in 0..hop {
            self.target[i] = self.mono(continuation + i as u64);
        }
        let mut low = nominal.saturating_sub(self.tolerance as u64);
        let mut high = nominal + self.tolerance as u64;
        if let Some(last) = last.filter(|&last| last < high) {
            high = last;
            low = low.min(last.saturating_sub(2 * self.tolerance as u64));
        }
        let score = |this: &Self, start: u64, step: usize| -> f64 {
            let (mut dot, mut energy) = (0.0f64, 0.0f64);
            let mut i = 0;
            while i < hop {
                let x = f64::from(this.mono(start + i as u64));
                dot += x * f64::from(this.target[i]);
                energy += x * x;
                i += step;
            }
            if energy > 1e-20 {
                dot / energy.sqrt()
            } else {
                0.0
            }
        };
        // A coarse search on every fourth position and sample, then a fine one
        // around the best.
        let mut best = (f64::NEG_INFINITY, nominal.clamp(low, high));
        let mut start = low;
        while start <= high {
            let s = score(self, start, 4);
            if s > best.0 {
                best = (s, start);
            }
            start += 4;
        }
        let around = best.1;
        let mut fine = (f64::NEG_INFINITY, around);
        for start in around.saturating_sub(3).max(low)..=(around + 3).min(high) {
            let s = score(self, start, 1);
            if s > fine.0 {
                fine = (s, start);
            }
        }
        fine.1
    }

    /// Clears all state, as if just built.
    pub fn reset(&mut self) {
        self.input.fill(0.0);
        self.pushed = 0;
        self.nominal = 0.0;
        self.previous = None;
        self.sum.fill(0.0);
        self.end = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(stretch: f64, input: &[f32]) -> Vec<f32> {
        let mut wsola = Wsola::new(1, 480, 144, stretch, 256);
        let mut out = Vec::new();
        let mut hop = vec![0.0; 480];
        for chunk in input.chunks(256) {
            wsola.push(chunk);
            while wsola.pull(&mut hop) {
                out.extend_from_slice(&hop);
            }
        }
        out
    }

    /// The dominant period of `x`, by autocorrelation, in frames.
    fn period(x: &[f32]) -> usize {
        (20..400)
            .max_by(|a, b| {
                let c = |lag: usize| -> f64 {
                    x[..x.len() - lag]
                        .iter()
                        .zip(&x[lag..])
                        .map(|(a, b)| f64::from(*a) * f64::from(*b))
                        .sum()
                };
                c(*a).total_cmp(&c(*b))
            })
            .unwrap()
    }

    #[test]
    fn the_length_changes_and_the_pitch_stays() {
        let tone: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 48_000.0).sin() * 0.5)
            .collect();
        for stretch in [0.5, 0.8, 1.0, 1.25, 2.0] {
            let out = run(stretch, &tone);
            let expected = 48_000.0 * stretch;
            assert!(
                (out.len() as f64 - expected).abs() < expected * 0.05 + 2000.0,
                "{stretch}: {}",
                out.len()
            );
            let middle = &out[out.len() / 4..out.len() / 4 + 8000];
            // 48000 / 220 = 218.2 frames.
            let p = period(middle);
            assert!((217..=219).contains(&p), "{stretch}: period {p}");
            // The level stays (the windows sum to one, the segments line up).
            let peak = middle.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!((peak - 0.5).abs() < 0.05, "{stretch}: peak {peak}");
        }
    }

    #[test]
    fn neither_end_is_faded() {
        let tone: Vec<f32> = (0..24_000)
            .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 48_000.0).sin() * 0.5)
            .collect();
        let mut wsola = Wsola::new(1, 480, 144, 0.8, 256);
        let mut out = Vec::new();
        let mut hop = vec![0.0; 480];
        for chunk in tone.chunks(256) {
            wsola.push(chunk);
            while wsola.pull(&mut hop) {
                out.extend_from_slice(&hop);
            }
        }
        wsola.finish();
        let length = (24_000.0 * 0.8) as usize;
        while out.len() < length {
            assert!(wsola.pull(&mut hop), "it always pulls after the end");
            out.extend_from_slice(&hop);
        }
        // The start is the input as it is.
        assert_eq!(out[..480], tone[..480]);
        // The last 10 ms keep the level.
        let peak = out[length - 480..length]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.05, "peak {peak}");
    }

    #[test]
    fn silence_stays_silent() {
        let out = run(1.5, &vec![0.0; 20_000]);
        assert!(!out.is_empty());
        assert!(out.iter().all(|s| *s == 0.0));
    }
}
