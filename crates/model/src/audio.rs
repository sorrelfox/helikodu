// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Audio vocabulary: sample formats, sample rates, channel layouts, exact times,
//! output backends and devices.

mod audio_spec;
mod channel_layout;
mod channel_position;
mod device_id;
mod device_info;
mod media_time;
mod output_backend;
mod output_mode;
mod output_selection;
mod sample_format;
mod sample_rate;

pub use audio_spec::AudioSpec;
pub use channel_layout::ChannelLayout;
pub use channel_position::ChannelPosition;
pub use device_id::{DeviceId, DeviceKey, DeviceSelection};
pub use device_info::{DeviceCapabilities, DeviceInfo};
pub use media_time::MediaTime;
pub use output_backend::{BackendSelection, OutputBackendKind};
pub use output_mode::OutputMode;
pub use output_selection::OutputSelection;
pub use sample_format::{Endianness, SampleFormat};
pub use sample_rate::SampleRate;
