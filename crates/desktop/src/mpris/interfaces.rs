// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org.mpris.MediaPlayer2` and `org.mpris.MediaPlayer2.Player` at
//! `/org/mpris/MediaPlayer2`.
//!
//! The getters read the values last announced, so a client never sees a value
//! before the `PropertiesChanged` signal that announces it. Method calls become
//! player commands; the player's events then change the properties.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use helikodu_model::audio::MediaTime;
use helikodu_model::player::{ShuffleMode, Volume};
use helikodu_service::player::PlayerCommand;
use zbus::object_server::SignalEmitter;
use zbus::{ObjectPath, OwnedVariant, fdo, interface};

use super::state::{self, MetaValue, Metadata};
use super::{Shared, path_from_uri};

/// The root interface.
pub(crate) struct Root {
    pub(crate) shared: Arc<Shared>,
}

#[interface(name = "org.mpris.MediaPlayer2", introspection_docs = false)]
impl Root {
    fn raise(&self) {
        if let Some(raise) = &self.shared.hooks.raise {
            raise();
        }
    }

    fn quit(&self) {
        if let Some(quit) = &self.shared.hooks.quit {
            quit();
        }
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_quit(&self) -> bool {
        self.shared.hooks.quit.is_some()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_raise(&self) -> bool {
        self.shared.hooks.raise.is_some()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn identity(&self) -> String {
        self.shared.config.identity.clone()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn desktop_entry(&self) -> String {
        self.shared.config.desktop_entry.clone()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["file".to_owned()]
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn supported_mime_types(&self) -> Vec<String> {
        self.shared.config.supported_mime_types.clone()
    }
}

/// The player interface.
pub(crate) struct Player {
    pub(crate) shared: Arc<Shared>,
}

impl Player {
    fn send(&self, command: PlayerCommand) {
        (self.shared.hooks.command)(command);
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player", introspection_docs = false)]
impl Player {
    fn next(&self) {
        if self.shared.state().published().can_go_next {
            self.send(PlayerCommand::Next);
        }
    }

    fn previous(&self) {
        if self.shared.state().published().can_go_previous {
            self.send(PlayerCommand::Previous);
        }
    }

    fn pause(&self) {
        if self.shared.state().published().can_pause {
            self.send(PlayerCommand::Pause);
        }
    }

    fn play_pause(&self) {
        let props = self.shared.state().published().clone();
        if props.can_pause || props.can_play {
            self.send(PlayerCommand::PlayPause);
        }
    }

    fn stop(&self) {
        self.send(PlayerCommand::Stop);
    }

    fn play(&self) {
        if self.shared.state().published().can_play {
            self.send(PlayerCommand::Play);
        }
    }

    /// Seeks by `offset` microseconds. Past the end goes to the next track; before
    /// the start goes to the start.
    fn seek(&self, offset: i64) {
        let (can_seek, position, length) = {
            let state = self.shared.state();
            (
                state.published().can_seek,
                state.position_micros(),
                state.length(),
            )
        };
        if !can_seek || offset == 0 {
            return;
        }
        let target = position.saturating_add(offset);
        if let Some(length) = length
            && target > state::micros(length)
        {
            self.send(PlayerCommand::Next);
            return;
        }
        let target = u64::try_from(target).unwrap_or(0);
        self.send(PlayerCommand::Seek(MediaTime::from_duration(
            Duration::from_micros(target),
        )));
    }

    /// Moves to `position` microseconds in `track_id`; ignored when that is not
    /// the current track or the position is outside it.
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        let (can_seek, current, length) = {
            let state = self.shared.state();
            (
                state.published().can_seek,
                state.current_track_id(),
                state.length(),
            )
        };
        if !can_seek || current.as_deref() != Some(track_id.as_str()) {
            return;
        }
        let Ok(position) = u64::try_from(position) else {
            return;
        };
        let position = Duration::from_micros(position);
        if length.is_some_and(|length| position > length) {
            return;
        }
        self.send(PlayerCommand::Seek(MediaTime::from_duration(position)));
    }

    /// Opens a `file://` URI. Other schemes are refused.
    fn open_uri(&self, uri: String) -> fdo::Result<()> {
        let path = path_from_uri(&uri)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("not a local file: {uri}")))?;
        match &self.shared.hooks.open {
            Some(open) => open(path),
            None => self.send(PlayerCommand::PlayFiles {
                files: vec![path],
                start: 0,
            }),
        }
        Ok(())
    }

    #[zbus(signal)]
    pub(crate) async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.shared.state().published().playback_status.to_owned()
    }

    #[zbus(property)]
    fn loop_status(&self) -> String {
        self.shared.state().published().loop_status.to_owned()
    }

    #[zbus(property)]
    fn set_loop_status(&self, value: String) -> fdo::Result<()> {
        let repeat = state::repeat_mode(&value)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("unknown loop status {value}")))?;
        self.shared.state().assume(|state| {
            let modes = state::with_repeat(state.modes(), repeat);
            state.set_modes(modes);
        });
        self.send(PlayerCommand::SetRepeat(repeat));
        Ok(())
    }

    /// Playback runs at its own rate only.
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    /// A rate of 0 pauses, as the specification allows; other rates are ignored.
    #[zbus(property)]
    fn set_rate(&self, value: f64) {
        if value == 0.0 {
            self.send(PlayerCommand::Pause);
        }
    }

    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.shared.state().published().shuffle
    }

    #[zbus(property)]
    fn set_shuffle(&self, value: bool) {
        let mut shuffle = ShuffleMode::Off;
        self.shared.state().assume(|state| {
            let modes = state::with_shuffle(state.modes(), value);
            shuffle = modes.shuffle;
            state.set_modes(modes);
        });
        self.send(PlayerCommand::SetShuffle(shuffle));
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedVariant> {
        to_dbus(&self.shared.state().published().metadata)
    }

    /// The position of the volume slider, from 0.0 to 1.0, as the window shows it.
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.shared.state().published().volume
    }

    #[zbus(property)]
    fn set_volume(&self, value: f64) {
        // Not-a-number and negative values are silence, as the specification says.
        #[allow(clippy::cast_possible_truncation)]
        let volume = Volume::clamped(value as f32);
        self.shared
            .state()
            .assume(|state| state.set_volume(f64::from(volume.position())));
        self.send(PlayerCommand::SetVolume(volume));
    }

    /// Never announced: clients follow `Seeked` and interpolate.
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.shared.state().position_micros()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.shared.state().published().can_go_next
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.shared.state().published().can_go_previous
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.shared.state().published().can_play
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.shared.state().published().can_pause
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.shared.state().published().can_seek
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}

/// The metadata as a D-Bus dictionary (`a{sv}`).
pub(crate) fn to_dbus(metadata: &Metadata) -> HashMap<String, OwnedVariant> {
    metadata
        .iter()
        .filter_map(|(key, value)| Some(((*key).to_owned(), to_variant(value)?)))
        .collect()
}

/// One metadata value as a variant; `None` for an object path that is not one.
pub(crate) fn to_variant(value: &MetaValue) -> Option<OwnedVariant> {
    let variant = match value {
        MetaValue::Path(path) => {
            zbus::Variant::from(ObjectPath::try_from(path.as_str()).ok()?.into_owned())
        }
        MetaValue::Text(text) => zbus::Variant::from(text.clone()),
        MetaValue::List(list) => zbus::Variant::from(list.clone()),
        MetaValue::Int(n) => zbus::Variant::from(*n),
        MetaValue::Long(n) => zbus::Variant::from(*n),
        MetaValue::Real(x) => zbus::Variant::from(*x),
    };
    OwnedVariant::try_from(variant).ok()
}
