// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Software volume on the cubic curve of the volume control.
//!
//! The control's position `p` becomes the amplitude `a = p³`
//! ([`Volume::amplitude`]). Every change moves there over a short linear ramp, so
//! dragging the slider never clicks. Muting ramps to silence the same way and keeps
//! the volume for when it is unmuted.
//!
//! A control thread changes the volume through a [`SharedVolume`]: two atomics that
//! the audio side reads once per block, without locking.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use helikodu_model::player::Volume;

use super::ramp::{SmoothedGain, frames_in};

/// The software volume stage.
#[derive(Debug, Clone)]
pub struct SoftwareVolume {
    channels: usize,
    gain: SmoothedGain,
    volume: Volume,
    muted: bool,
    shared: Option<SharedVolume>,
}

impl SoftwareVolume {
    /// How long a volume change takes.
    pub const RAMP: Duration = Duration::from_millis(20);

    /// A volume stage for `channels` channels at `rate` frames per second, resting at
    /// `volume`.
    pub fn new(rate: u32, channels: usize, volume: Volume) -> Self {
        Self {
            channels,
            gain: SmoothedGain::new(f64::from(volume.amplitude()), frames_in(Self::RAMP, rate)),
            volume,
            muted: false,
            shared: None,
        }
    }

    /// A volume stage that follows `shared`: changes made there are picked up at the
    /// start of each block.
    pub fn following(rate: u32, channels: usize, shared: SharedVolume) -> Self {
        let mut stage = Self::new(rate, channels, shared.volume());
        stage.muted = shared.is_muted();
        stage.gain.jump_to(stage.target());
        stage.shared = Some(shared);
        stage
    }

    /// The volume the stage moves to or rests at.
    pub fn volume(&self) -> Volume {
        self.volume
    }

    /// Whether the stage is muted.
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Moves to `volume`.
    pub fn set_volume(&mut self, volume: Volume) {
        self.volume = volume;
        self.gain.set_target(self.target());
    }

    /// Mutes or unmutes, keeping the volume.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        self.gain.set_target(self.target());
    }

    /// Whether the stage currently leaves samples untouched: full volume, not muted,
    /// not moving.
    pub fn is_transparent(&self) -> bool {
        !self.gain.is_ramping() && self.gain.current() == 1.0
    }

    fn target(&self) -> f64 {
        if self.muted {
            0.0
        } else {
            f64::from(self.volume.amplitude())
        }
    }

    /// Picks up a change made through the [`SharedVolume`] this stage follows.
    /// [`apply`](Self::apply) does this itself.
    pub fn refresh(&mut self) {
        if let Some(shared) = &self.shared {
            let (volume, muted) = (shared.volume(), shared.is_muted());
            if volume != self.volume || muted != self.muted {
                self.volume = volume;
                self.muted = muted;
                self.gain.set_target(self.target());
            }
        }
    }

    /// Applies the volume to the interleaved frames of `block`.
    pub fn apply(&mut self, block: &mut [f32]) {
        self.refresh();
        self.gain.apply(block, self.channels);
    }
}

/// A volume and mute setting shared between a control thread and the audio side.
///
/// Cloning gives another handle to the same setting. Reading and writing are
/// lock-free.
#[derive(Debug, Clone)]
pub struct SharedVolume {
    inner: Arc<SharedInner>,
}

#[derive(Debug)]
struct SharedInner {
    position: AtomicU32,
    muted: AtomicBool,
}

impl SharedVolume {
    /// A shared setting at `volume`, not muted.
    pub fn new(volume: Volume) -> Self {
        Self {
            inner: Arc::new(SharedInner {
                position: AtomicU32::new(volume.position().to_bits()),
                muted: AtomicBool::new(false),
            }),
        }
    }

    /// The volume.
    pub fn volume(&self) -> Volume {
        Volume::clamped(f32::from_bits(self.inner.position.load(Ordering::Relaxed)))
    }

    /// Sets the volume.
    pub fn set_volume(&self, volume: Volume) {
        self.inner
            .position
            .store(volume.position().to_bits(), Ordering::Relaxed);
    }

    /// Whether the output is muted.
    pub fn is_muted(&self) -> bool {
        self.inner.muted.load(Ordering::Relaxed)
    }

    /// Mutes or unmutes.
    pub fn set_muted(&self, muted: bool) {
        self.inner.muted.store(muted, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(p: f32) -> Volume {
        Volume::new(p).unwrap()
    }

    #[test]
    fn uses_the_cubic_curve() {
        let mut stage = SoftwareVolume::new(48_000, 2, volume(0.5));
        let mut block = [1.0f32, -1.0];
        stage.apply(&mut block);
        assert_eq!(block, [0.125, -0.125]);
    }

    #[test]
    fn full_volume_is_transparent() {
        let mut stage = SoftwareVolume::new(48_000, 1, Volume::FULL);
        assert!(stage.is_transparent());
        let mut block = [0.1f32, 0.2, -0.3];
        stage.apply(&mut block);
        assert_eq!(block, [0.1, 0.2, -0.3]);
    }

    #[test]
    fn changes_ramp_without_steps() {
        let rate = 48_000;
        let ramp = frames_in(SoftwareVolume::RAMP, rate);
        let mut stage = SoftwareVolume::new(rate, 1, Volume::FULL);
        // A full-scale sine at 1 kHz, with the volume pulled to zero during it.
        let signal: Vec<f32> = (0..4800)
            .map(|n| (2.0 * std::f64::consts::PI * 1000.0 * f64::from(n) / 48_000.0).sin() as f32)
            .collect();
        let mut out = signal.clone();
        for (i, chunk) in out.chunks_mut(256).enumerate() {
            if i == 2 {
                stage.set_volume(Volume::SILENT);
            }
            stage.apply(chunk);
        }
        // The gain moves by at most 1/ramp per frame, so the output differs from the
        // input's own slope by at most that much.
        let largest_input_step = signal
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        let largest_output_step = out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(largest_output_step <= largest_input_step + 1.0 / ramp as f32 + 1e-6);
        assert!(out[512 + ramp..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn mute_keeps_the_volume() {
        let mut stage = SoftwareVolume::new(1000, 1, volume(0.8));
        stage.set_muted(true);
        let mut block = vec![1.0f32; 100];
        stage.apply(&mut block);
        assert_eq!(block[99], 0.0);
        assert_eq!(stage.volume(), volume(0.8));
        stage.set_muted(false);
        stage.apply(&mut block);
        let mut last = [1.0f32];
        stage.apply(&mut last);
        assert!((last[0] - 0.512).abs() < 1e-6);
    }

    #[test]
    fn follows_a_shared_setting() {
        let shared = SharedVolume::new(Volume::FULL);
        let mut stage = SoftwareVolume::following(1000, 1, shared.clone());
        shared.set_volume(volume(0.5));
        let mut block = vec![1.0f32; 100];
        stage.apply(&mut block);
        assert_eq!(block[99], 0.125);
        shared.set_muted(true);
        stage.apply(&mut block);
        assert!(stage.is_muted());
        assert_eq!(block[99], 0.0);
    }
}
