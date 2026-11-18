// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The audio output settings: the `[output]` table.

mod device_lost_policy;
mod dsd_output;
mod output_format;
mod sample_rate_policy;
mod volume_control;

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

pub use device_lost_policy::DeviceLostPolicy;
pub use dsd_output::DsdOutput;
pub use output_format::OutputFormat;
pub use sample_rate_policy::SampleRatePolicy;
pub use volume_control::VolumeControl;

use super::{SettingsError, check_range};
use crate::audio::{BackendSelection, DeviceKey, DeviceSelection, OutputMode, OutputSelection};

/// The allowed length of the playback buffer, in milliseconds.
const BUFFER_MS: RangeInclusive<u32> = 50..=2000;

/// Where and how audio is played: the `[output]` table of `settings.toml`.
///
/// ```toml
/// [output]
/// backend = "auto"            # auto | pipewire | pulseaudio | alsa | jack
/// device = "default"          # default, or the backend's stable device name
/// mode = "shared"             # shared | exclusive
/// sample_rate = "match"       # match | device-default | fixed:48000
/// format = "auto"             # auto | s16 | s24 | s24_3 | s32 | f32
/// buffer_ms = 200
/// volume = "system"           # system | software | hardware:<mixer element> | fixed
/// on_device_lost = "pause"    # pause | fallback-default | stop
/// pause_on_unplug = true
/// preroll_ms = 0
/// exclusive_release_s = 30
/// dsd = "pcm"                 # pcm | dop
///
/// [output.profiles."alsa:hw:CARD=D10s,DEV=0"]
/// mode = "exclusive"
/// volume = "fixed"
/// fixed_volume_confirmed = true
/// exclusive_release_s = 0
/// ```
///
/// A profile applies while its device is in use: [`for_device`](Self::for_device)
/// gives the settings with it applied. "System default" has a profile of its own, keyed
/// `<backend>:default`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSettings {
    /// The backend.
    pub backend: BackendSelection,
    /// The device of that backend.
    pub device: DeviceSelection,
    /// Shared or exclusive use of the device.
    pub mode: OutputMode,
    /// Which sample rate to open the device at.
    pub sample_rate: SampleRatePolicy,
    /// Which sample format to send to the device.
    pub format: OutputFormat,
    /// How much decoded audio is kept ahead of the device, in milliseconds (50 to 2000).
    pub buffer_ms: u32,
    /// How the volume is applied.
    pub volume: VolumeControl,
    /// What happens when the chosen device disappears.
    pub on_device_lost: DeviceLostPolicy,
    /// Whether playback pauses when the device it plays to while following "System
    /// default" is unplugged (headphones or a USB or Bluetooth device removed), instead
    /// of going on through the next default device.
    pub pause_on_unplug: bool,
    /// Silence sent after the device is opened or reconfigured, in milliseconds, for
    /// converters that take a moment to lock on to a new signal.
    pub preroll_ms: u32,
    /// Seconds of pause after which a device held exclusively is released; 0 keeps it.
    pub exclusive_release_s: u32,
    /// How DSD files reach the device: converted to PCM, or as DoP where the path
    /// leaves the samples untouched.
    pub dsd: DsdOutput,
    /// Settings that apply to one device only, by backend and device name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<DeviceKey, DeviceProfile>,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            backend: BackendSelection::default(),
            device: DeviceSelection::default(),
            mode: OutputMode::default(),
            sample_rate: SampleRatePolicy::default(),
            format: OutputFormat::default(),
            buffer_ms: 200,
            volume: VolumeControl::default(),
            on_device_lost: DeviceLostPolicy::default(),
            pause_on_unplug: true,
            preroll_ms: 0,
            exclusive_release_s: 30,
            dsd: DsdOutput::Pcm,
            profiles: BTreeMap::new(),
        }
    }
}

impl OutputSettings {
    /// The backend, device and mode these settings choose.
    pub fn selection(&self) -> OutputSelection {
        OutputSelection {
            backend: self.backend,
            device: self.device.clone(),
            mode: self.mode,
        }
    }

    /// The settings with the profile of `key` applied: each value the profile sets
    /// replaces the general one. Without a profile they are these settings.
    ///
    /// A fixed volume needs the user's confirmation for the device
    /// ([`DeviceProfile::fixed_volume_confirmed`]); until then the volume is applied in
    /// software instead, so that nothing plays louder than the volume control says.
    pub fn for_device(&self, key: &DeviceKey) -> OutputSettings {
        let mut settings = self.clone();
        let profile = self.profiles.get(key);
        if let Some(profile) = profile {
            let DeviceProfile {
                mode,
                sample_rate,
                format,
                buffer_ms,
                volume,
                preroll_ms,
                exclusive_release_s,
                fixed_volume_confirmed: _,
            } = profile;
            settings.mode = mode.unwrap_or(self.mode);
            settings.sample_rate = sample_rate.unwrap_or(self.sample_rate);
            settings.format = format.unwrap_or(self.format);
            settings.buffer_ms = buffer_ms.unwrap_or(self.buffer_ms);
            settings.volume = volume.clone().unwrap_or_else(|| self.volume.clone());
            settings.preroll_ms = preroll_ms.unwrap_or(self.preroll_ms);
            settings.exclusive_release_s = exclusive_release_s.unwrap_or(self.exclusive_release_s);
        }
        let confirmed = profile.is_some_and(|p| p.fixed_volume_confirmed);
        if settings.volume == VolumeControl::Fixed && !confirmed && !key.backend.is_for_testing() {
            settings.volume = VolumeControl::Software;
        }
        settings
    }

    /// Checks the values that the types alone do not constrain.
    pub fn validate(&self) -> Result<(), SettingsError> {
        check_buffer("output.buffer_ms", self.buffer_ms)?;
        for (key, profile) in &self.profiles {
            if let Some(buffer_ms) = profile.buffer_ms {
                check_buffer(&format!("output.profiles.\"{key}\".buffer_ms"), buffer_ms)?;
            }
        }
        Ok(())
    }
}

fn check_buffer(key: &str, value: u32) -> Result<(), SettingsError> {
    check_range(key, value, BUFFER_MS)
}

/// Settings for one device that replace the general ones while it is in use. A field
/// that is not set keeps the general setting.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceProfile {
    /// Replaces [`OutputSettings::mode`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<OutputMode>,
    /// Replaces [`OutputSettings::sample_rate`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<SampleRatePolicy>,
    /// Replaces [`OutputSettings::format`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<OutputFormat>,
    /// Replaces [`OutputSettings::buffer_ms`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_ms: Option<u32>,
    /// Replaces [`OutputSettings::volume`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<VolumeControl>,
    /// Replaces [`OutputSettings::preroll_ms`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preroll_ms: Option<u32>,
    /// Replaces [`OutputSettings::exclusive_release_s`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_release_s: Option<u32>,
    /// Whether the user has accepted full, fixed output volume on this device. A fixed
    /// volume is not applied to a device until this is set.
    #[serde(skip_serializing_if = "is_false")]
    pub fixed_volume_confirmed: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
fn is_false(value: &bool) -> bool {
    !value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{DeviceId, OutputBackendKind, SampleRate};
    use crate::settings::Settings;

    /// The documented example, as a user would write it.
    const EXAMPLE: &str = r#"
[output]
backend = "auto"            # auto | pipewire | pulseaudio | alsa | jack
device = "default"          # backend-specific stable ID
mode = "shared"             # shared | exclusive
sample_rate = "match"       # match | device-default | fixed:48000
format = "auto"             # auto | s16 | s24 | s24_3 | s32 | f32
buffer_ms = 200
volume = "system"           # system | software | hardware:<mixer element> | fixed
on_device_lost = "pause"    # pause | fallback-default | stop
preroll_ms = 0
exclusive_release_s = 30    # seconds paused before an exclusive device is released; 0 = keep it

# Per-device overrides, keyed by backend and device ID.
[output.profiles."alsa:hw:CARD=D10s,DEV=0"]
mode = "exclusive"
volume = "fixed"
fixed_volume_confirmed = true
"#;

    fn d10s() -> DeviceKey {
        DeviceKey::new(
            OutputBackendKind::Alsa,
            DeviceId::new("hw:CARD=D10s,DEV=0").unwrap(),
        )
    }

    #[test]
    fn documented_example_parses() {
        let settings: Settings = toml::from_str(EXAMPLE).unwrap();
        let output = &settings.output;
        let general = OutputSettings {
            profiles: BTreeMap::new(),
            ..output.clone()
        };
        assert_eq!(general, OutputSettings::default());
        assert_eq!(output.profiles.len(), 1);
        let profile = &output.profiles[&d10s()];
        assert_eq!(profile.mode, Some(OutputMode::Exclusive));
        assert_eq!(profile.volume, Some(VolumeControl::Fixed));
        assert!(profile.fixed_volume_confirmed);
        assert_eq!(profile.sample_rate, None);
        assert_eq!(settings.validate(), Ok(()));
    }

    #[test]
    fn every_value_round_trips() {
        let mut output = OutputSettings {
            backend: BackendSelection::Backend(OutputBackendKind::PipeWire),
            device: DeviceSelection::Device(
                DeviceId::new("alsa_output.usb-Topping_D10s-00.analog-stereo").unwrap(),
            ),
            mode: OutputMode::Exclusive,
            sample_rate: SampleRatePolicy::Fixed(SampleRate::HZ_48000),
            format: OutputFormat::S24Packed,
            buffer_ms: 500,
            volume: VolumeControl::Hardware("PCM".into()),
            on_device_lost: DeviceLostPolicy::FallbackDefault,
            pause_on_unplug: false,
            preroll_ms: 250,
            exclusive_release_s: 0,
            dsd: DsdOutput::Dop,
            profiles: BTreeMap::new(),
        };
        output.profiles.insert(
            d10s(),
            DeviceProfile {
                sample_rate: Some(SampleRatePolicy::DeviceDefault),
                format: Some(OutputFormat::S32),
                buffer_ms: Some(100),
                preroll_ms: Some(1000),
                exclusive_release_s: Some(5),
                ..DeviceProfile::default()
            },
        );
        let settings = Settings {
            output,
            ..Settings::default()
        };
        let text = toml::to_string(&settings).unwrap();
        assert!(text.contains(r#"sample_rate = "fixed:48000""#), "{text}");
        assert!(text.contains(r#"volume = "hardware:PCM""#), "{text}");
        assert!(
            text.contains(r#"on_device_lost = "fallback-default""#),
            "{text}"
        );
        assert!(text.contains(r#"format = "s24_3""#), "{text}");
        assert!(!text.contains("fixed_volume_confirmed"), "{text}");
        assert!(text.contains("pause_on_unplug = false"), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn partial_tables_keep_defaults() {
        let settings: Settings = toml::from_str("[output]\nbackend = \"alsa\"\n").unwrap();
        assert_eq!(
            settings.output.backend,
            BackendSelection::Backend(OutputBackendKind::Alsa)
        );
        assert_eq!(settings.output.buffer_ms, 200);
        assert_eq!(settings.output.exclusive_release_s, 30);
    }

    #[test]
    fn invalid_values_are_errors() {
        for text in [
            "[output]\nbackend = \"coreaudio\"\n",
            "[output]\nmode = \"bitperfect\"\n",
            "[output]\nsample_rate = \"fixed:0\"\n",
            "[output]\nvolume = \"hardware:\"\n",
            "[output]\ndevice = \"\"\n",
            "[output.profiles.\"nowhere\"]\nmode = \"shared\"\n",
        ] {
            assert!(toml::from_str::<Settings>(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_profile_replaces_what_it_sets() {
        let mut output = OutputSettings::default();
        output.profiles.insert(
            d10s(),
            DeviceProfile {
                mode: Some(OutputMode::Exclusive),
                sample_rate: Some(SampleRatePolicy::Fixed(SampleRate::HZ_48000)),
                format: Some(OutputFormat::S24Packed),
                buffer_ms: Some(500),
                volume: Some(VolumeControl::Hardware("PCM".into())),
                preroll_ms: Some(250),
                exclusive_release_s: Some(0),
                fixed_volume_confirmed: false,
            },
        );
        let applied = output.for_device(&d10s());
        assert_eq!(applied.mode, OutputMode::Exclusive);
        assert_eq!(
            applied.sample_rate,
            SampleRatePolicy::Fixed(SampleRate::HZ_48000)
        );
        assert_eq!(applied.format, OutputFormat::S24Packed);
        assert_eq!(applied.buffer_ms, 500);
        assert_eq!(applied.volume, VolumeControl::Hardware("PCM".into()));
        assert_eq!(applied.preroll_ms, 250);
        assert_eq!(applied.exclusive_release_s, 0);
        assert_eq!(applied.profiles, output.profiles);

        // Another device, or "System default", keeps the general settings.
        let other =
            DeviceKey::of_selection(OutputBackendKind::Alsa, &DeviceSelection::SystemDefault);
        assert_eq!(output.for_device(&other), output);

        // Only what the profile sets is replaced.
        output.profiles.insert(
            other.clone(),
            DeviceProfile {
                buffer_ms: Some(1000),
                ..DeviceProfile::default()
            },
        );
        let applied = output.for_device(&other);
        assert_eq!(applied.buffer_ms, 1000);
        assert_eq!(applied.mode, output.mode);
        assert_eq!(applied.exclusive_release_s, 30);
    }

    #[test]
    fn fixed_volume_needs_the_confirmation_of_the_device() {
        let mut output = OutputSettings {
            volume: VolumeControl::Fixed,
            ..OutputSettings::default()
        };
        assert_eq!(output.for_device(&d10s()).volume, VolumeControl::Software);
        let null = DeviceKey::new(OutputBackendKind::Null, DeviceId::new("null").unwrap());
        assert_eq!(output.for_device(&null).volume, VolumeControl::Fixed);
        output.profiles.insert(
            d10s(),
            DeviceProfile {
                fixed_volume_confirmed: true,
                ..DeviceProfile::default()
            },
        );
        assert_eq!(output.for_device(&d10s()).volume, VolumeControl::Fixed);
    }

    #[test]
    fn buffer_range_is_validated() {
        let mut output = OutputSettings {
            buffer_ms: 10,
            ..OutputSettings::default()
        };
        let error = output.validate().unwrap_err();
        assert_eq!(error.key, "output.buffer_ms");
        assert_eq!(
            error.to_string(),
            "output.buffer_ms is 10, but must be from 50 to 2000"
        );

        output.buffer_ms = 2000;
        output.profiles.insert(
            d10s(),
            DeviceProfile {
                buffer_ms: Some(5000),
                ..DeviceProfile::default()
            },
        );
        let error = output.validate().unwrap_err();
        assert_eq!(
            error.key,
            "output.profiles.\"alsa:hw:CARD=D10s,DEV=0\".buffer_ms"
        );
    }
}
