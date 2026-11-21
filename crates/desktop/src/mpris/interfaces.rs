// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org.mpris.MediaPlayer2`, `org.mpris.MediaPlayer2.Player`,
//! `org.mpris.MediaPlayer2.TrackList` and `org.mpris.MediaPlayer2.Playlists` at
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
use helikodu_model::settings::SpeedSettings;
use helikodu_service::player::PlayerCommand;
use zbus::object_server::SignalEmitter;
use zbus::{ObjectPath, OwnedObjectPath, OwnedVariant, fdo, interface};

use super::state::{self, MetaValue, Metadata};
use super::{MprisPlaylist, PLAYLIST_ID_PREFIX, Shared, path_from_uri, playlist_id};

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
        true
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

    /// The playback speed. Positions are the track's own time, so clients that
    /// interpolate move them this much faster than the clock.
    #[zbus(property)]
    fn rate(&self) -> f64 {
        self.shared.state().published().rate
    }

    /// A rate of 0 pauses, as the specification allows; a rate from 0.5 to 2
    /// plays at that speed, keeping the pitch as it is set to; other rates are
    /// ignored.
    #[zbus(property)]
    fn set_rate(&self, value: f64) {
        if value == 0.0 {
            self.send(PlayerCommand::Pause);
        } else if SpeedSettings::RATE_RANGE.contains(&value) {
            let keep_pitch = self.shared.state().speed().keep_pitch;
            let speed = SpeedSettings::at(value, keep_pitch);
            self.shared.state().assume(|state| state.set_speed(speed));
            self.send(PlayerCommand::SetSpeed(speed));
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
        *SpeedSettings::RATE_RANGE.start()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn maximum_rate(&self) -> f64 {
        *SpeedSettings::RATE_RANGE.end()
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

/// The track list interface: the queue in play order.
pub(crate) struct TrackList {
    pub(crate) shared: Arc<Shared>,
}

#[interface(name = "org.mpris.MediaPlayer2.TrackList", introspection_docs = false)]
impl TrackList {
    /// The metadata of the shown entries among `track_ids`, in that order;
    /// other ids are left out.
    fn get_tracks_metadata(
        &self,
        track_ids: Vec<ObjectPath<'_>>,
    ) -> Vec<HashMap<String, OwnedVariant>> {
        let wanted: Vec<_> = {
            let state = self.shared.state();
            track_ids
                .iter()
                .filter_map(|id| state.shown_entry(id.as_str()).cloned())
                .collect()
        };
        let tracks: Vec<_> = wanted.iter().map(|entry| entry.track.clone()).collect();
        let details = self.shared.describe(&tracks);
        let state = self.shared.state();
        wanted
            .iter()
            .zip(&details)
            .map(|(entry, details)| {
                to_dbus(&state.entry_metadata(entry.id, &entry.track, details.as_ref()))
            })
            .collect()
    }

    /// Tracks cannot be added through MPRIS (`CanEditTracks` is false).
    fn add_track(&self, _uri: String, _after_track: ObjectPath<'_>, _set_as_current: bool) {}

    /// Tracks cannot be removed through MPRIS (`CanEditTracks` is false).
    fn remove_track(&self, _track_id: ObjectPath<'_>) {}

    /// Plays the entry `track_id`; ignored when it is not in the list.
    fn go_to(&self, track_id: ObjectPath<'_>) {
        let entry = self
            .shared
            .state()
            .shown_entry(track_id.as_str())
            .map(|entry| entry.id);
        if let Some(entry) = entry {
            (self.shared.hooks.command)(PlayerCommand::PlayEntry(entry));
        }
    }

    #[zbus(signal)]
    pub(crate) async fn track_list_replaced(
        emitter: &SignalEmitter<'_>,
        tracks: Vec<ObjectPath<'_>>,
        current_track: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn track_added(
        emitter: &SignalEmitter<'_>,
        metadata: HashMap<String, OwnedVariant>,
        after_track: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn track_removed(
        emitter: &SignalEmitter<'_>,
        track_id: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn track_metadata_changed(
        emitter: &SignalEmitter<'_>,
        track_id: ObjectPath<'_>,
        metadata: HashMap<String, OwnedVariant>,
    ) -> zbus::Result<()>;

    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn tracks(&self) -> Vec<OwnedObjectPath> {
        self.shared
            .state()
            .published_tracks()
            .iter()
            .filter_map(|entry| OwnedObjectPath::try_from(super::track_id(*entry)).ok())
            .collect()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_edit_tracks(&self) -> bool {
        false
    }
}

/// The orderings `GetPlaylists` takes.
const ORDERINGS: [&str; 4] = [
    "Alphabetical",
    "CreationDate",
    "ModifiedDate",
    "UserDefined",
];

/// The playlists interface.
pub(crate) struct Playlists {
    pub(crate) shared: Arc<Shared>,
}

/// A playlist as the interface returns it: id, name and icon (none).
type PlaylistTuple = (OwnedObjectPath, String, String);

fn playlist_tuple(playlist: &MprisPlaylist) -> Option<PlaylistTuple> {
    Some((
        OwnedObjectPath::try_from(playlist_id(playlist.id)).ok()?,
        playlist.name.clone(),
        String::new(),
    ))
}

/// The playlists in `order` (an unknown one is `UserDefined`), reversed if asked.
pub(crate) fn ordered(
    mut playlists: Vec<MprisPlaylist>,
    order: &str,
    reverse: bool,
) -> Vec<MprisPlaylist> {
    match order {
        "Alphabetical" => playlists.sort_by_cached_key(|p| p.name.to_lowercase()),
        "CreationDate" => playlists.sort_by_key(|p| p.created),
        "ModifiedDate" => playlists.sort_by_key(|p| p.modified),
        _ => {}
    }
    if reverse {
        playlists.reverse();
    }
    playlists
}

#[interface(name = "org.mpris.MediaPlayer2.Playlists", introspection_docs = false)]
impl Playlists {
    /// Plays the playlist `playlist_id`; ignored when there is no such playlist.
    fn activate_playlist(&self, playlist_id: ObjectPath<'_>) {
        let Some(source) = &self.shared.hooks.playlists else {
            return;
        };
        let id = playlist_id
            .as_str()
            .strip_prefix(PLAYLIST_ID_PREFIX)
            .and_then(|n| n.parse::<u64>().ok());
        if let Some(id) = id
            && source.playlists().iter().any(|p| p.id == id)
        {
            source.activate(id);
        }
    }

    /// Up to `max_count` playlists from `index` in `order`.
    fn get_playlists(
        &self,
        index: u32,
        max_count: u32,
        order: String,
        reverse_order: bool,
    ) -> Vec<PlaylistTuple> {
        let Some(source) = &self.shared.hooks.playlists else {
            return Vec::new();
        };
        ordered(source.playlists(), &order, reverse_order)
            .iter()
            .skip(usize::try_from(index).unwrap_or(usize::MAX))
            .take(usize::try_from(max_count).unwrap_or(usize::MAX))
            .filter_map(playlist_tuple)
            .collect()
    }

    #[zbus(signal)]
    pub(crate) async fn playlist_changed(
        emitter: &SignalEmitter<'_>,
        playlist: (ObjectPath<'_>, &str, &str),
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn playlist_count(&self) -> u32 {
        u32::try_from(self.shared.playlists().len()).unwrap_or(u32::MAX)
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn orderings(&self) -> Vec<String> {
        ORDERINGS.iter().map(|o| (*o).to_owned()).collect()
    }

    #[zbus(property)]
    fn active_playlist(&self) -> (bool, PlaylistTuple) {
        let (valid, (id, name, icon)) = self.shared.active_playlist();
        let id = OwnedObjectPath::try_from(id)
            .unwrap_or_else(|_| OwnedObjectPath::try_from("/").expect("the root is a path"));
        (valid, (id, name, icon))
    }
}

/// The `ActivePlaylist` value as a variant (`(b(oss))`).
pub(crate) fn active_playlist_variant(
    value: &(bool, (String, String, String)),
) -> Option<OwnedVariant> {
    let (valid, (id, name, icon)) = value;
    let id = ObjectPath::try_from(id.as_str()).ok()?;
    let inner = zbus::Structure::from((id, name.as_str(), icon.as_str()));
    let outer = zbus::Structure::from((*valid, inner));
    OwnedVariant::try_from(zbus::Variant::from(outer)).ok()
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
