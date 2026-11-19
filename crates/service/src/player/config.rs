// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a player starts with.

use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::Duration;

use helikodu_engine::dsp::processing_settings;
use helikodu_engine::output::{Pacing, SharedReservation};
use helikodu_engine::pipeline::{
    ClippingPrevention, LoudnessMode, LoudnessSettings, ProcessingSettings,
};
use helikodu_engine::transport::EngineConfig;
use helikodu_library::PlayRules;
use helikodu_model::audio::{
    BackendSelection, DeviceSelection, OutputBackendKind, OutputSelection,
};
use helikodu_model::player::Volume;
use helikodu_model::settings::{
    ClippingPrevention as ClippingSetting, ContextAction, DecoderSettings, DspSettings,
    LoudnessMode as LoudnessSetting, LoudnessSettings as LoudnessTable, OutputSettings,
    PlaybackSettings, TransitionSettings,
};

use super::PlayerError;
use crate::Event;
use crate::bus::EventBus;
use crate::clock::{SharedClock, SystemClock};
use crate::library::MusicLibrary;

/// How the null and WAV backends, which have no device clock, take audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SinkPacing {
    /// At the speed it plays, like a device.
    #[default]
    RealTime,
    /// As fast as it is decoded: for tests and for writing files.
    Freewheel,
}

/// Which loudness gain the player applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ReplayGainMode {
    /// No loudness normalization.
    #[default]
    Off,
    /// The track gain, or the album gain if a file has no track gain.
    Track,
    /// The album gain, or the track gain if a file has no album gain. For files
    /// queued in album order.
    Album,
    /// The album gain for a file whose neighbour in the queue (the file before or
    /// after it) has the same album and album artists in its tags, the track gain
    /// otherwise.
    Auto,
}

/// Loudness normalization by the ReplayGain and Opus R128 tags of the files.
///
/// Gains are relative to the ReplayGain 2.0 reference of −18 LUFS; Opus R128 gains
/// (relative to −23 LUFS) are converted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplayGain {
    /// Which gain is applied.
    pub mode: ReplayGainMode,
    /// Added to the gain of files that have one, in dB.
    pub preamp_db: f64,
    /// The gain of files without loudness tags, in dB.
    pub untagged_preamp_db: f64,
    /// Whether a gain is lowered so that the file's peak (from its tags) stays at or
    /// below full scale. On by default.
    pub prevent_clipping: bool,
    /// Whether files without loudness values are evened out by the leveller
    /// instead of the untagged preamp. Off by default.
    pub leveller: bool,
}

impl ReplayGain {
    /// The range of both preamps, in dB.
    pub const PREAMP_RANGE: RangeInclusive<f64> = -24.0..=24.0;

    /// Loudness normalization as the `[loudness]` settings ask for it.
    pub fn from_settings(loudness: &LoudnessTable) -> Self {
        Self {
            mode: match loudness.mode {
                LoudnessSetting::Off => ReplayGainMode::Off,
                LoudnessSetting::Track => ReplayGainMode::Track,
                LoudnessSetting::Album => ReplayGainMode::Album,
                LoudnessSetting::Auto => ReplayGainMode::Auto,
            },
            preamp_db: loudness.preamp_db,
            untagged_preamp_db: loudness.untagged_preamp_db,
            prevent_clipping: loudness.clipping == ClippingSetting::LowerGain,
            leveller: loudness.leveller,
        }
    }
}

impl Default for ReplayGain {
    fn default() -> Self {
        Self {
            mode: ReplayGainMode::Off,
            preamp_db: 0.0,
            untagged_preamp_db: 0.0,
            prevent_clipping: true,
            leveller: false,
        }
    }
}

/// How the player behaves, from the `[playback]` settings. It can be changed while
/// the player runs ([`PlayerCommand::Configure`](super::PlayerCommand::Configure)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerPrefs {
    /// How much of an entry must have played for "Previous" to restart it instead of
    /// going to the entry before; `None` always goes to the entry before.
    pub previous_restart: Option<Duration>,
    /// What counts as a play and a skip, and which tracks remember where listening
    /// stopped.
    pub play_rules: PlayRules,
    /// Whether playing a context adds it to the end of the queue instead of
    /// replacing the queue.
    pub append_contexts: bool,
    /// Whether long library tracks start where listening last stopped.
    pub resume_long_tracks: bool,
}

impl Default for PlayerPrefs {
    fn default() -> Self {
        Self::from_settings(&PlaybackSettings::default())
    }
}

impl PlayerPrefs {
    /// The preferences the `[playback]` settings ask for.
    pub fn from_settings(playback: &PlaybackSettings) -> Self {
        Self {
            previous_restart: playback.previous_restart(),
            play_rules: PlayRules {
                play_percent: playback.play_count_percent,
                play_time: playback.play_count_time(),
                resume_after: playback.long_track(),
            },
            append_contexts: playback.play_context == ContextAction::Append,
            resume_long_tracks: playback.resume_long_tracks,
        }
    }
}

/// Where the session is kept, and whether it is restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// The folder of the session file and the crash sentinel: normally
    /// `$XDG_STATE_HOME/helikodu`.
    pub dir: PathBuf,
    /// Whether the saved queue, position, modes, volume and output are restored at
    /// the start. The crash sentinel is read either way.
    pub restore: bool,
    /// How long after a change the session is saved, so that a burst of changes is
    /// written once. Zero saves after every command.
    pub debounce: Duration,
    /// How often the position is saved while playing.
    pub position_interval: Duration,
}

impl SessionConfig {
    /// The session in `dir`, restored, saved a second after changes and every
    /// fifteen seconds while playing.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            restore: true,
            debounce: Duration::from_secs(1),
            position_interval: Duration::from_secs(15),
        }
    }
}

/// What a player starts with.
#[derive(Debug, Clone)]
pub struct PlayerConfig {
    /// The `[output]` settings. A restored session's output selection replaces
    /// the backend, device and mode.
    pub output: OutputSettings,
    /// The volume to start with, unless a session is restored.
    pub volume: Volume,
    /// How the null and WAV backends take audio.
    pub pacing: SinkPacing,
    /// Loudness normalization; off by default.
    pub replay_gain: ReplayGain,
    /// How the player behaves.
    pub prefs: PlayerPrefs,
    /// The library: plays library tracks, resolves contexts and records statistics.
    /// Without it only files play and nothing is recorded.
    pub library: Option<MusicLibrary>,
    /// The session; without it nothing is saved or restored and no crash sentinel is
    /// kept.
    pub session: Option<SessionConfig>,
    /// The time source.
    pub clock: SharedClock,
    /// The bus to publish on; a new one when `None`.
    pub bus: Option<EventBus<Event>>,
    /// The seed of the shuffle order; from the time when `None`.
    pub shuffle_seed: Option<u64>,
    /// How the ALSA backend takes a sound card that another program holds, for
    /// exclusive output. The desktop integration supplies one; the default takes
    /// nothing.
    pub reservation: SharedReservation,
    /// The `[dsp]` settings: the DSP chain and the resampler quality.
    pub dsp: DspSettings,
    /// The crossfades and fades (`[playback.transitions]`).
    pub transitions: TransitionSettings,
    /// The long-tail decoders (`[playback.decoders]`): loops, fades, the SoundFont,
    /// SID song lengths and ROMs, tracker rendering, whole-file buffering.
    pub decoders: DecoderSettings,
}

impl PlayerConfig {
    /// Plays to `selection`, with the default output settings otherwise, at full
    /// volume and in real time, without library or session.
    pub fn new(selection: OutputSelection) -> Self {
        let OutputSelection {
            backend,
            device,
            mode,
        } = selection;
        Self {
            output: OutputSettings {
                backend,
                device,
                mode,
                ..OutputSettings::default()
            },
            volume: Volume::FULL,
            pacing: SinkPacing::RealTime,
            replay_gain: ReplayGain::default(),
            prefs: PlayerPrefs::default(),
            library: None,
            session: None,
            clock: SystemClock::shared(),
            bus: None,
            shuffle_seed: None,
            reservation: SharedReservation::default(),
            dsp: DspSettings::default(),
            transitions: TransitionSettings::default(),
            decoders: DecoderSettings::default(),
        }
    }

    /// Checks the settings: the ranges of the output settings and the preamps, and
    /// that the WAV backend has a file name as its device.
    pub fn validate(&self) -> Result<(), PlayerError> {
        self.output
            .validate()
            .map_err(|error| PlayerError::InvalidConfig(error.to_string()))?;
        for (name, value) in [
            ("preamp", self.replay_gain.preamp_db),
            ("untagged preamp", self.replay_gain.untagged_preamp_db),
        ] {
            if !ReplayGain::PREAMP_RANGE.contains(&value) {
                return Err(PlayerError::InvalidConfig(format!(
                    "the {name} is {value} dB, but must be from {} to {} dB",
                    ReplayGain::PREAMP_RANGE.start(),
                    ReplayGain::PREAMP_RANGE.end()
                )));
            }
        }
        if self.output.backend == BackendSelection::Backend(OutputBackendKind::Wav)
            && self.output.device == DeviceSelection::SystemDefault
        {
            return Err(PlayerError::InvalidConfig(
                "the wav backend needs a file name as its device".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn engine_config(&self, output: &OutputSelection) -> EngineConfig {
        let mut settings = self.output.clone();
        settings.backend = output.backend;
        settings.device = output.device.clone();
        settings.mode = output.mode;
        EngineConfig {
            output: settings,
            volume: self.volume,
            sink_pacing: match self.pacing {
                SinkPacing::RealTime => Pacing::RealTime,
                SinkPacing::Freewheel => Pacing::Freewheel,
            },
            processing: processing_settings(
                ProcessingSettings {
                    loudness: self.loudness_settings(),
                    ..ProcessingSettings::default()
                },
                &self.dsp,
            ),
            dsp: self.dsp.clone(),
            device_reservation: self.reservation.clone(),
            ..EngineConfig::default()
        }
        .with_transitions(self.transitions)
    }
}

impl PlayerConfig {
    fn loudness_settings(&self) -> LoudnessSettings {
        let gain = &self.replay_gain;
        LoudnessSettings {
            mode: match gain.mode {
                ReplayGainMode::Off => LoudnessMode::Off,
                ReplayGainMode::Track => LoudnessMode::Track,
                ReplayGainMode::Album => LoudnessMode::Album,
                ReplayGainMode::Auto => LoudnessMode::Auto,
            },
            preamp_db: gain.preamp_db,
            untagged_preamp_db: gain.untagged_preamp_db,
            clipping: if gain.prevent_clipping {
                ClippingPrevention::Peak
            } else {
                ClippingPrevention::Limiter
            },
            leveller: gain.leveller,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helikodu_model::audio::OutputMode;

    #[test]
    fn selection_becomes_output_settings() {
        let selection = OutputSelection {
            backend: BackendSelection::Backend(OutputBackendKind::Alsa),
            device: "hw:CARD=D10s,DEV=0".parse().unwrap(),
            mode: OutputMode::Exclusive,
        };
        let config = PlayerConfig::new(selection.clone());
        assert_eq!(config.output.selection(), selection);
        assert_eq!(config.output.buffer_ms, OutputSettings::default().buffer_ms);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn wav_needs_a_file() {
        let mut selection = OutputSelection {
            backend: BackendSelection::Backend(OutputBackendKind::Wav),
            ..OutputSelection::default()
        };
        assert!(matches!(
            PlayerConfig::new(selection.clone()).validate(),
            Err(PlayerError::InvalidConfig(_))
        ));
        selection.device = "out.wav".parse().unwrap();
        assert_eq!(PlayerConfig::new(selection).validate(), Ok(()));
    }

    #[test]
    fn device_profiles_reach_the_engine() {
        use helikodu_model::audio::{DeviceId, DeviceKey};
        use helikodu_model::settings::DeviceProfile;

        let mut config = PlayerConfig::new(OutputSelection::default());
        let key = DeviceKey::new(
            OutputBackendKind::PulseAudio,
            DeviceId::new("alsa_output.usb-dac").unwrap(),
        );
        config.output.profiles.insert(
            key.clone(),
            DeviceProfile {
                buffer_ms: Some(500),
                exclusive_release_s: Some(0),
                ..DeviceProfile::default()
            },
        );
        config.output.pause_on_unplug = false;
        let selection = OutputSelection {
            backend: BackendSelection::Backend(OutputBackendKind::PulseAudio),
            device: DeviceSelection::Device(key.device.clone()),
            ..OutputSelection::default()
        };
        let engine = config.engine_config(&selection).output;
        assert_eq!(engine.profiles, config.output.profiles);
        assert!(!engine.pause_on_unplug);
        let applied = engine.for_device(&key);
        assert_eq!(applied.buffer_ms, 500);
        assert_eq!(applied.exclusive_release_s, 0);
    }

    #[test]
    fn replay_gain_reaches_the_engine() {
        let mut config = PlayerConfig::new(OutputSelection::default());
        assert_eq!(
            config
                .engine_config(&OutputSelection::default())
                .processing
                .loudness
                .mode,
            LoudnessMode::Off
        );
        config.replay_gain = ReplayGain {
            mode: ReplayGainMode::Album,
            preamp_db: 3.0,
            untagged_preamp_db: -2.0,
            prevent_clipping: false,
            leveller: true,
        };
        let loudness = config
            .engine_config(&OutputSelection::default())
            .processing
            .loudness;
        assert_eq!(loudness.mode, LoudnessMode::Album);
        assert_eq!(loudness.preamp_db, 3.0);
        assert_eq!(loudness.untagged_preamp_db, -2.0);
        assert_eq!(loudness.clipping, ClippingPrevention::Limiter);
        assert!(loudness.leveller);

        config.replay_gain.preamp_db = 30.0;
        assert!(matches!(
            config.validate(),
            Err(PlayerError::InvalidConfig(_))
        ));
        config.replay_gain.preamp_db = f64::NAN;
        assert!(config.validate().is_err());
    }

    #[test]
    fn preferences_follow_the_settings() {
        let playback = PlaybackSettings {
            previous_restarts_after_s: 0,
            play_count_percent: 60,
            play_count_minutes: 2,
            play_context: ContextAction::Append,
            ..PlaybackSettings::default()
        };
        let prefs = PlayerPrefs::from_settings(&playback);
        assert_eq!(prefs.previous_restart, None);
        assert_eq!(prefs.play_rules.play_percent, 60);
        assert_eq!(prefs.play_rules.play_time, Duration::from_secs(120));
        assert!(prefs.append_contexts);
        assert_eq!(
            PlayerPrefs::default().previous_restart,
            Some(Duration::from_secs(3))
        );
        let gain = ReplayGain::from_settings(&LoudnessTable::default());
        assert_eq!(gain.mode, ReplayGainMode::Auto);
        assert_eq!(gain.untagged_preamp_db, -6.0);
        assert!(gain.prevent_clipping);
        assert!(!gain.leveller);
        let levelled = LoudnessTable {
            leveller: true,
            ..LoudnessTable::default()
        };
        assert!(ReplayGain::from_settings(&levelled).leveller);
    }

    #[test]
    fn invalid_buffer_is_refused() {
        let mut config = PlayerConfig::new(OutputSelection::default());
        config.output.buffer_ms = 1;
        assert!(config.validate().is_err());
    }
}
