// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a stream is doing, and its counters.

use helikodu_model::audio::{MediaTime, SampleRate};

/// The state of an output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamState {
    /// Open and negotiated, not started yet.
    Ready,
    /// Playing.
    Running,
    /// Paused by [`pause`](super::OutputStream::pause).
    Paused,
    /// Its device went away. An [`OutputEvent::DeviceLost`](super::OutputEvent::DeviceLost)
    /// was sent.
    Lost,
    /// The backend failed. An [`OutputEvent::Error`](super::OutputEvent::Error) was
    /// sent, or the error is returned by [`stop`](super::OutputStream::stop).
    Failed,
    /// Stopped and closed.
    Closed,
}

impl StreamState {
    pub(crate) const fn to_u8(self) -> u8 {
        match self {
            Self::Ready => 0,
            Self::Running => 1,
            Self::Paused => 2,
            Self::Lost => 3,
            Self::Failed => 4,
            Self::Closed => 5,
        }
    }

    pub(crate) const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Ready,
            1 => Self::Running,
            2 => Self::Paused,
            3 => Self::Lost,
            4 => Self::Failed,
            _ => Self::Closed,
        }
    }

    /// Whether the stream has ended for good: lost, failed or closed.
    pub const fn is_finished(self) -> bool {
        matches!(self, Self::Lost | Self::Failed | Self::Closed)
    }
}

/// A snapshot of a stream's counters.
///
/// All frame counts are at the stream's [`rate`](Self::rate) and count from when
/// the stream was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamTiming {
    /// The stream's sample rate.
    pub rate: SampleRate,
    /// Frames the render side has written into the ring buffer.
    pub frames_written: u64,
    /// Frames the device side has taken from the ring buffer and sent on.
    pub frames_delivered: u64,
    /// Frames the device side has dropped from the ring buffer unheard, for a seek
    /// or a pause ([`DeviceAction::Discard`](super::DeviceAction::Discard)).
    pub frames_discarded: u64,
    /// Frames delivered but not yet heard: the device's and sound server's delay,
    /// as last reported.
    pub delay_frames: u64,
    /// Device periods or callbacks that found the ring buffer short.
    pub underruns: u64,
    /// Frames of silence played in place of missing samples.
    pub silent_frames: u64,
    /// The rate the device or the sound server's graph actually runs at, if known.
    /// When it differs from [`rate`](Self::rate), the server resamples.
    pub device_rate: Option<SampleRate>,
}

impl StreamTiming {
    /// Frames that have reached the listener: delivered minus the device delay.
    pub const fn frames_played(&self) -> u64 {
        self.frames_delivered.saturating_sub(self.delay_frames)
    }

    /// [`frames_played`](Self::frames_played) as a time.
    pub const fn played(&self) -> MediaTime {
        MediaTime::from_frames(self.frames_played(), self.rate)
    }

    /// Frames in the ring buffer, written but neither delivered nor discarded.
    pub const fn queued_frames(&self) -> u64 {
        self.frames_written
            .saturating_sub(self.frames_delivered)
            .saturating_sub(self.frames_discarded)
    }

    /// The ring position (frames written before it) of the frame the listener hears
    /// now: the frames taken from the ring buffer, delivered or discarded, minus the
    /// device delay.
    pub const fn heard_position(&self) -> u64 {
        (self.frames_delivered + self.frames_discarded).saturating_sub(self.delay_frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_through_u8() {
        for state in [
            StreamState::Ready,
            StreamState::Running,
            StreamState::Paused,
            StreamState::Lost,
            StreamState::Failed,
            StreamState::Closed,
        ] {
            assert_eq!(StreamState::from_u8(state.to_u8()), state);
        }
    }

    #[test]
    fn played_position_subtracts_the_delay() {
        let timing = StreamTiming {
            rate: SampleRate::HZ_48000,
            frames_written: 96_000,
            frames_delivered: 48_000,
            frames_discarded: 0,
            delay_frames: 4_800,
            underruns: 0,
            silent_frames: 0,
            device_rate: None,
        };
        assert_eq!(timing.frames_played(), 43_200);
        assert_eq!(timing.played(), MediaTime::from_millis(900));
        assert_eq!(timing.queued_frames(), 48_000);
        assert_eq!(timing.heard_position(), 43_200);
        let flushed = StreamTiming {
            frames_discarded: 40_000,
            ..timing
        };
        assert_eq!(flushed.queued_frames(), 8_000);
        assert_eq!(flushed.heard_position(), 83_200);
        let early = StreamTiming {
            frames_delivered: 100,
            ..timing
        };
        assert_eq!(early.frames_played(), 0);
    }
}
