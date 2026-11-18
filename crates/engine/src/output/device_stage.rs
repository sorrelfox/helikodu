// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The processing that runs on the device side of the ring buffer: software volume,
//! anti-click fades and dither.
//!
//! These stages answer the user (volume, pause, seek, stop), so they run after the
//! ring buffer, where a change is heard within one device period instead of after
//! the whole buffer. The render thread controls them with [`DeviceAction`]s
//! scheduled at ring positions, so that a fade starts on exactly the frame it was
//! meant for.

use std::time::Duration;

use helikodu_model::audio::AudioSpec;

use crate::pipeline::{FadeState, Fader, SharedVolume, SoftwareVolume, TpdfDither};

/// A change to the device-side processing, made when the device side reaches a
/// given position of the ring buffer.
///
/// See [`SampleWriter::schedule`](super::SampleWriter::schedule).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeviceAction {
    /// Fades in (`open`) or out over `duration`, with the raised cosine of the
    /// pipeline's [`Fader`]. A fade that interrupts another starts from the level
    /// reached so far.
    Fade {
        /// Fade in rather than out.
        open: bool,
        /// The length of a full fade.
        duration: Duration,
    },
    /// Opens or shuts the fader at once, without a fade. Only for moments when
    /// nothing is heard, such as before the first sample of a stream.
    SetOpen(bool),
    /// Drops the frames from here up to the ring position `until` without playing
    /// them. With `hold`, the device side then plays silence, without taking frames
    /// and without counting underruns, until a [`Fade`](Self::Fade) in or a
    /// [`SetOpen(true)`](Self::SetOpen) is scheduled: the render thread can refill
    /// the buffer before playback continues.
    Discard {
        /// The ring position to drop up to; at most the frames written so far.
        until: u64,
        /// Wait for a fade in before taking frames again.
        hold: bool,
    },
    /// Whether every sample is dithered (the audio was processed, or the source has
    /// more bits than the device), or only while volume or a fade changes them.
    AlwaysDither(bool),
}

/// Software volume, the fader and dither, in the order of the
/// [`OutputChain`](crate::pipeline::OutputChain) they replace on the device side.
#[derive(Debug)]
pub(crate) struct DeviceStage {
    channels: usize,
    rate: u32,
    shared_volume: SharedVolume,
    /// Nothing has been processed yet: the volume starts where it is set, without
    /// a ramp.
    started: bool,
    volume: SoftwareVolume,
    fader: Fader,
    dither: Option<TpdfDither>,
    always_dither: bool,
    held: bool,
}

impl DeviceStage {
    /// A stage for the negotiated `spec`, following `volume`. The fader starts open,
    /// the volume at whatever `volume` holds, so that samples pass untouched until
    /// the render thread asks for something else.
    pub(crate) fn new(spec: AudioSpec, volume: SharedVolume) -> Self {
        let rate = spec.rate.hz();
        let channels = usize::from(spec.channel_count().max(1));
        Self {
            channels,
            rate,
            shared_volume: volume.clone(),
            started: false,
            volume: SoftwareVolume::following(rate, channels, volume),
            fader: Fader::new(rate, true),
            dither: TpdfDither::for_format(spec.format),
            always_dither: false,
            held: false,
        }
    }

    /// Carries out `action`, except [`DeviceAction::Discard`], which the ring reader
    /// handles; for it this only sets the hold.
    pub(crate) fn act(&mut self, action: DeviceAction) {
        match action {
            DeviceAction::Fade { open, duration } => {
                if open {
                    self.held = false;
                    self.fader.fade_in(duration);
                } else {
                    self.fader.fade_out(duration);
                }
            }
            DeviceAction::SetOpen(open) => {
                if open {
                    self.held = false;
                }
                self.fader.set_open(open);
            }
            DeviceAction::Discard { hold, .. } => self.held |= hold,
            DeviceAction::AlwaysDither(always) => self.always_dither = always,
        }
    }

    /// Whether the device side waits for a fade in before taking frames.
    pub(crate) fn is_held(&self) -> bool {
        self.held
    }

    /// Whether the fader is shut and not moving.
    pub(crate) fn is_silent(&self) -> bool {
        self.fader.is_silent()
    }

    /// The frames until a fade out reaches silence, if one is under way.
    pub(crate) fn frames_to_silence(&self) -> Option<usize> {
        (self.fader.state() == FadeState::FadingOut).then(|| self.fader.remaining_frames())
    }

    /// Takes the volume as it is before the first frame, without a ramp.
    fn begin(&mut self) {
        if !self.started {
            // Real-time safe: no allocation, and the old stage only drops a reference.
            self.volume =
                SoftwareVolume::following(self.rate, self.channels, self.shared_volume.clone());
            self.started = true;
        }
    }

    /// Whether [`apply`](Self::apply) would leave the next frames untouched: the
    /// fader open, the volume full and still, and nothing to dither. Integer samples
    /// then go to the device without passing through a float.
    pub(crate) fn is_transparent(&mut self) -> bool {
        self.begin();
        self.volume.refresh();
        self.fader.state() == FadeState::Open
            && self.volume.is_transparent()
            && !(self.always_dither && self.dither.is_some())
    }

    /// Applies volume, fade and dither to the interleaved frames of `block`.
    ///
    /// Dither runs on the frames that something changes: all of them when the audio
    /// is processed or the volume is not full, otherwise only those a fade passes
    /// over. Silence after a fade out stays digital silence.
    pub(crate) fn apply(&mut self, block: &mut [f32]) {
        self.begin();
        let frames = block.len() / self.channels;
        let (audible, fading) = match self.fader.state() {
            FadeState::Silent => (0, 0),
            FadeState::FadingOut => {
                let left = self.fader.remaining_frames().min(frames);
                (left, left)
            }
            FadeState::FadingIn => (frames, self.fader.remaining_frames().min(frames)),
            FadeState::Open => (frames, 0),
        };
        self.volume.refresh();
        let dithered = if self.always_dither || !self.volume.is_transparent() {
            audible
        } else {
            fading
        };
        self.volume.apply(block);
        self.fader.apply(block, self.channels);
        if let Some(dither) = &mut self.dither {
            dither.apply(&mut block[..dithered * self.channels]);
        }
    }
}

#[cfg(test)]
mod tests {
    use helikodu_model::audio::{ChannelLayout, Endianness, SampleFormat, SampleRate};
    use helikodu_model::player::Volume;

    use super::*;

    fn stage(format: SampleFormat) -> (DeviceStage, SharedVolume) {
        let volume = SharedVolume::new(Volume::FULL);
        let spec = AudioSpec::new(format, SampleRate::HZ_48000, ChannelLayout::STEREO);
        (DeviceStage::new(spec, volume.clone()), volume)
    }

    #[test]
    fn untouched_until_told_otherwise() {
        let (mut stage, _) = stage(SampleFormat::S16(Endianness::Little));
        let mut block = [0.123_456_79f32, -0.5, 0.25, 1.0e-7];
        let copy = block;
        stage.apply(&mut block);
        assert_eq!(block.map(f32::to_bits), copy.map(f32::to_bits));
    }

    #[test]
    fn fades_out_to_digital_silence_and_holds() {
        let (mut stage, _) = stage(SampleFormat::S16(Endianness::Little));
        stage.act(DeviceAction::Fade {
            open: false,
            duration: Duration::from_millis(10),
        });
        assert_eq!(stage.frames_to_silence(), Some(480));
        let mut block = vec![0.5f32; 2 * 600];
        stage.apply(&mut block);
        assert!(stage.is_silent());
        assert!(block[2 * 480..].iter().all(|&s| s == 0.0));
        stage.act(DeviceAction::Discard {
            until: 0,
            hold: true,
        });
        assert!(stage.is_held());
        stage.act(DeviceAction::Fade {
            open: true,
            duration: Duration::from_millis(5),
        });
        assert!(!stage.is_held());
    }

    #[test]
    fn follows_the_shared_volume() {
        let (mut stage, volume) = stage(SampleFormat::F32(Endianness::Little));
        volume.set_volume(Volume::new(0.5).unwrap());
        // Set before the first sample: no ramp.
        let mut block = vec![1.0f32; 2 * 10];
        stage.apply(&mut block);
        assert!(block.iter().all(|&s| (s - 0.125).abs() < 1e-6));
        // Changed while playing: a ramp.
        volume.set_volume(Volume::FULL);
        let mut block = vec![1.0f32; 2 * 4800];
        stage.apply(&mut block);
        assert!(block[0] < 0.2);
        assert_eq!(block[block.len() - 1], 1.0);
    }

    #[test]
    fn transparent_only_while_nothing_changes_the_samples() {
        let (mut stage, volume) = stage(SampleFormat::S32(Endianness::Little));
        assert!(stage.is_transparent());
        stage.act(DeviceAction::Fade {
            open: false,
            duration: Duration::from_millis(1),
        });
        assert!(!stage.is_transparent());
        stage.act(DeviceAction::SetOpen(true));
        assert!(stage.is_transparent());
        volume.set_volume(Volume::new(0.5).unwrap());
        assert!(!stage.is_transparent());
        // No dither for 32-bit integers, so always dithering changes nothing.
        let (mut stage, _) = super::tests::stage(SampleFormat::S32(Endianness::Little));
        stage.act(DeviceAction::AlwaysDither(true));
        assert!(stage.is_transparent());
        let (mut stage, _) = super::tests::stage(SampleFormat::S16(Endianness::Little));
        stage.act(DeviceAction::AlwaysDither(true));
        assert!(!stage.is_transparent());
    }

    #[test]
    fn dither_stops_with_the_fade() {
        let (mut stage, _) = stage(SampleFormat::S16(Endianness::Little));
        stage.act(DeviceAction::SetOpen(false));
        stage.act(DeviceAction::Fade {
            open: true,
            duration: Duration::from_millis(1),
        });
        let value = 1000.0 / 32_768.0;
        let mut block = vec![value; 2 * 200];
        stage.apply(&mut block);
        assert!(block[2 * 48..].iter().all(|&s| s == value));
    }
}
