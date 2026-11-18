// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MPRIS driven by a fake player, seen by a client on the same private bus.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helikodu_desktop::mpris::{
    BUS_NAME, Mpris, MprisConfig, MprisHooks, MprisPlaylist, OBJECT_PATH, PLAYER_INTERFACE,
    PLAYLISTS_INTERFACE, PlayerView, PlaylistSource, ROOT_INTERFACE, TRACKLIST_INTERFACE,
    TrackDetails, playlist_id, track_id,
};
use helikodu_model::audio::{
    AudioSpec, ChannelLayout, Endianness, MediaTime, SampleFormat, SampleRate,
};
use helikodu_model::library::{MediaSource, TrackRef};
use helikodu_model::player::{
    ContextSource, EntryId, PlaybackState, QueueContext, QueueEntry, RepeatMode, TransportState,
    Volume,
};
use helikodu_service::player::{
    NowPlaying, PlayerCommand, PlayerEvent, PlayerStatus, QueueSnapshot,
};
use zbus::{Connection, Message, ObjectPath, OwnedObjectPath, OwnedVariant};

use super::private_bus::{PrivateBus, Signals};

const WAIT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(300);

/// The player's status and queue as the fake player has them.
#[derive(Clone, Default)]
struct FakeView {
    status: Arc<Mutex<PlayerStatus>>,
    queue: Arc<Mutex<Arc<QueueSnapshot>>>,
}

impl PlayerView for FakeView {
    fn status(&self) -> PlayerStatus {
        self.status.lock().unwrap().clone()
    }

    fn queue(&self) -> Arc<QueueSnapshot> {
        Arc::clone(&self.queue.lock().unwrap())
    }
}

fn spec() -> AudioSpec {
    AudioSpec::new(
        SampleFormat::S16(Endianness::Little),
        SampleRate::HZ_44100,
        ChannelLayout::STEREO,
    )
}

fn path(entry: u64) -> PathBuf {
    PathBuf::from(format!("/music/Track {entry}.flac"))
}

/// Makes `entry` (of `count`, numbered from 1) current in the fake player and
/// returns the event that says so.
fn play_entry(view: &FakeView, entry: u64, count: u64) -> PlayerEvent {
    let entries = (1..=count)
        .map(|id| QueueEntry {
            id: EntryId::new(id),
            track: TrackRef::File(MediaSource::file(path(id))),
            from_context: true,
        })
        .collect();
    let index = usize::try_from(entry - 1).unwrap();
    *view.queue.lock().unwrap() = Arc::new(QueueSnapshot {
        version: entry,
        entries,
        current: Some(index),
        ..QueueSnapshot::default()
    });
    let length = Some(MediaTime::from_millis(200_000));
    view.status.lock().unwrap().now_playing = Some(NowPlaying {
        entry: EntryId::new(entry),
        track: TrackRef::File(MediaSource::file(path(entry))),
        path: path(entry),
        spec: spec(),
        length,
    });
    PlayerEvent::TrackChanged {
        index,
        count: usize::try_from(count).unwrap(),
        entry: EntryId::new(entry),
        track: TrackRef::File(MediaSource::file(path(entry))),
        path: path(entry),
        spec: spec(),
        length,
    }
}

fn set_state(view: &FakeView, playback: PlaybackState) -> PlayerEvent {
    view.status.lock().unwrap().playback = playback;
    PlayerEvent::State {
        playback,
        transport: TransportState::Playing,
    }
}

struct Fixture {
    _bus: PrivateBus,
    client: Connection,
    mpris: Mpris,
    view: FakeView,
    commands: Arc<Mutex<Vec<PlayerCommand>>>,
    raised: Arc<AtomicUsize>,
    art_dir: PathBuf,
}

impl Fixture {
    fn start(name: &str) -> Option<Self> {
        Self::start_with(name, |hooks| hooks)
    }

    fn start_with(name: &str, more: impl FnOnce(MprisHooks) -> MprisHooks) -> Option<Self> {
        let bus = PrivateBus::start()?;
        let commands = Arc::new(Mutex::new(Vec::new()));
        let raised = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::clone(&commands);
        let raises = Arc::clone(&raised);
        let hooks = MprisHooks::new(move |command| recorded.lock().unwrap().push(command))
            .with_raise(move || {
                raises.fetch_add(1, Ordering::SeqCst);
            });
        let hooks = more(hooks);
        let art_dir =
            std::env::temp_dir().join(format!("helikodu-mpris-test-{name}-{}", std::process::id()));
        let config = MprisConfig {
            supported_mime_types: vec!["audio/flac".into()],
            art_dir: Some(art_dir.clone()),
            ..MprisConfig::default()
        };
        let mpris = Mpris::start_at(&bus.address, config, hooks).unwrap();
        let view = FakeView::default();
        mpris.sync(&view);
        let client = bus.connect();
        Some(Self {
            _bus: bus,
            client,
            mpris,
            view,
            commands,
            raised,
            art_dir,
        })
    }

    fn call(
        &self,
        interface: &str,
        method: &str,
        body: &(impl zbus::export::serde::Serialize + zbus::DynamicType),
    ) {
        zbus::block_on(self.client.call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(interface),
            method,
            body,
        ))
        .unwrap();
    }

    fn get(&self, interface: &str, name: &str) -> OwnedVariant {
        let reply = zbus::block_on(self.client.call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(interface, name),
        ))
        .unwrap();
        reply.body().deserialize::<OwnedVariant>().unwrap()
    }

    fn set(&self, name: &str, value: zbus::Variant<'_>) {
        self.call(
            "org.freedesktop.DBus.Properties",
            "Set",
            &(PLAYER_INTERFACE, name, value),
        );
    }

    fn take_commands(&self) -> Vec<PlayerCommand> {
        std::mem::take(&mut *self.commands.lock().unwrap())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.art_dir);
    }
}

type Changes = HashMap<String, OwnedVariant>;

fn properties_changed(message: &Message) -> Changes {
    let (interface, changed, invalidated): (String, Changes, Vec<String>) =
        message.body().deserialize().unwrap();
    assert_eq!(interface, PLAYER_INTERFACE);
    assert!(invalidated.is_empty());
    changed
}

/// The next `PropertiesChanged` of the player interface, skipping those of the
/// other interfaces.
fn player_changes(signals: &Signals) -> Changes {
    loop {
        let message = signals.next("PropertiesChanged", WAIT).unwrap();
        let (interface, _, _): (String, Changes, Vec<String>) =
            message.body().deserialize().unwrap();
        if interface == PLAYER_INTERFACE {
            return properties_changed(&message);
        }
    }
}

fn seeked(message: &Message) -> i64 {
    message.body().deserialize::<i64>().unwrap()
}

fn metadata(value: &OwnedVariant) -> HashMap<String, OwnedVariant> {
    value.try_clone().unwrap().try_into().unwrap()
}

fn text(value: &OwnedVariant) -> String {
    value.try_clone().unwrap().try_into().unwrap()
}

fn flag(value: &OwnedVariant) -> bool {
    value.try_clone().unwrap().try_into().unwrap()
}

#[test]
fn introspection_lists_both_interfaces_and_seeked() {
    let Some(fixture) = Fixture::start("introspect") else {
        return;
    };
    let reply = zbus::block_on(fixture.client.call_method(
        Some(BUS_NAME),
        OBJECT_PATH,
        Some("org.freedesktop.DBus.Introspectable"),
        "Introspect",
        &(),
    ))
    .unwrap();
    let xml: String = reply.body().deserialize().unwrap();
    for needle in [
        "interface name=\"org.mpris.MediaPlayer2\"",
        "interface name=\"org.mpris.MediaPlayer2.Player\"",
        "signal name=\"Seeked\"",
        "method name=\"SetPosition\"",
        "property name=\"Metadata\" type=\"a{sv}\" access=\"read\"",
        "property name=\"Volume\" type=\"d\" access=\"readwrite\"",
        "property name=\"Position\" type=\"x\" access=\"read\"",
    ] {
        assert!(xml.contains(needle), "{needle} missing in\n{xml}");
    }
    assert_eq!(fixture.mpris.bus_name(), BUS_NAME);
}

#[test]
fn properties_before_anything_plays() {
    let Some(fixture) = Fixture::start("idle") else {
        return;
    };
    assert_eq!(text(&fixture.get(ROOT_INTERFACE, "Identity")), "Helikodu");
    assert_eq!(
        text(&fixture.get(ROOT_INTERFACE, "DesktopEntry")),
        "io.github.sorrelfox.Helikodu"
    );
    assert!(flag(&fixture.get(ROOT_INTERFACE, "CanRaise")));
    assert!(!flag(&fixture.get(ROOT_INTERFACE, "CanQuit")));
    assert!(flag(&fixture.get(ROOT_INTERFACE, "HasTrackList")));
    let schemes: Vec<String> = fixture
        .get(ROOT_INTERFACE, "SupportedUriSchemes")
        .try_into()
        .unwrap();
    assert_eq!(schemes, ["file"]);

    assert_eq!(
        text(&fixture.get(PLAYER_INTERFACE, "PlaybackStatus")),
        "Stopped"
    );
    assert_eq!(text(&fixture.get(PLAYER_INTERFACE, "LoopStatus")), "None");
    assert!(flag(&fixture.get(PLAYER_INTERFACE, "CanControl")));
    assert!(!flag(&fixture.get(PLAYER_INTERFACE, "CanPlay")));
    assert!(!flag(&fixture.get(PLAYER_INTERFACE, "CanGoNext")));
    let metadata = metadata(&fixture.get(PLAYER_INTERFACE, "Metadata"));
    let id: ObjectPath<'_> = metadata["mpris:trackid"].downcast_ref().unwrap();
    assert_eq!(id.as_str(), "/org/mpris/MediaPlayer2/TrackList/NoTrack");

    // Calls that make no sense now reach nobody.
    fixture.call(PLAYER_INTERFACE, "Next", &());
    fixture.call(PLAYER_INTERFACE, "Seek", &(1_000_000i64,));
    fixture.call(ROOT_INTERFACE, "Raise", &());
    assert!(fixture.take_commands().is_empty());
    assert_eq!(fixture.raised.load(Ordering::SeqCst), 1);
}

#[test]
fn events_are_announced_once_and_seeks_are_signalled() {
    let Some(fixture) = Fixture::start("events") else {
        return;
    };
    let signals = Signals::record(&fixture.client, OBJECT_PATH);
    let view = &fixture.view;

    // A track starts: its metadata and capabilities, then a Seeked to 0.
    fixture.mpris.handle(&play_entry(view, 1, 2), view);
    let changed = player_changes(&signals);
    let meta = metadata(&changed["Metadata"]);
    let id: ObjectPath<'_> = meta["mpris:trackid"].downcast_ref().unwrap();
    assert_eq!(id.as_str(), track_id(EntryId::new(1)));
    let length: i64 = meta["mpris:length"].downcast_ref().unwrap();
    assert_eq!(length, 200_000_000);
    assert_eq!(text(&meta["xesam:title"]), "Track 1");
    for name in [
        "CanGoNext",
        "CanGoPrevious",
        "CanPlay",
        "CanPause",
        "CanSeek",
    ] {
        assert!(flag(&changed[name]), "{name}");
    }
    assert!(!changed.contains_key("PlaybackStatus"));
    assert!(!changed.contains_key("Position"));
    assert_eq!(seeked(&signals.next("Seeked", WAIT).unwrap()), 0);

    // Playing: only the status.
    fixture
        .mpris
        .handle(&set_state(view, PlaybackState::Playing), view);
    let changed = player_changes(&signals);
    assert_eq!(changed.len(), 1);
    assert_eq!(text(&changed["PlaybackStatus"]), "Playing");

    // The same state again, and an ordinary position: nothing.
    fixture
        .mpris
        .handle(&set_state(view, PlaybackState::Playing), view);
    let position = PlayerEvent::Position {
        index: 0,
        position: MediaTime::from_millis(100),
        length: None,
    };
    fixture.mpris.handle(&position, view);
    assert!(signals.drain(QUIET).is_empty());

    // A jump: Seeked with the new position.
    let jump = PlayerEvent::Position {
        index: 0,
        position: MediaTime::from_millis(90_000),
        length: None,
    };
    fixture.mpris.handle(&jump, view);
    assert_eq!(seeked(&signals.next("Seeked", WAIT).unwrap()), 90_000_000);
    let position: i64 = fixture
        .get(PLAYER_INTERFACE, "Position")
        .downcast_ref()
        .unwrap();
    assert!((90_000_000..100_000_000).contains(&position), "{position}");

    // The last track: no next one, other metadata.
    fixture.mpris.handle(&play_entry(view, 2, 2), view);
    let changed = player_changes(&signals);
    assert!(!flag(&changed["CanGoNext"]));
    assert_eq!(
        text(&metadata(&changed["Metadata"])["xesam:title"]),
        "Track 2"
    );
    assert_eq!(changed.len(), 2, "{:?}", changed.keys());

    // Details with a cover: the art is exported and its URL announced.
    let cover = fixture.art_dir.with_extension("source.jpg");
    std::fs::write(&cover, b"not really a jpeg").unwrap();
    fixture.mpris.set_details(
        EntryId::new(2),
        TrackDetails {
            title: Some("Second".into()),
            artists: vec!["Someone".into()],
            album: Some("Album".into()),
            cover: Some(cover.clone()),
            ..TrackDetails::default()
        },
    );
    let changed = player_changes(&signals);
    let meta = metadata(&changed["Metadata"]);
    assert_eq!(text(&meta["xesam:title"]), "Second");
    let art = text(&meta["mpris:artUrl"]);
    let art_path = art.strip_prefix("file://").unwrap();
    assert!(
        art_path.starts_with(fixture.art_dir.to_str().unwrap()),
        "{art}"
    );
    assert_eq!(std::fs::read(art_path).unwrap(), b"not really a jpeg");
    std::fs::remove_file(cover).unwrap();
}

#[test]
fn calls_reach_the_player() {
    let Some(fixture) = Fixture::start("calls") else {
        return;
    };
    let view = &fixture.view;
    fixture.mpris.handle(&play_entry(view, 1, 3), view);
    fixture
        .mpris
        .handle(&set_state(view, PlaybackState::Paused), view);
    let signals = Signals::record(&fixture.client, OBJECT_PATH);

    fixture.call(PLAYER_INTERFACE, "PlayPause", &());
    fixture.call(PLAYER_INTERFACE, "Next", &());
    fixture.call(PLAYER_INTERFACE, "Previous", &());
    fixture.call(PLAYER_INTERFACE, "Seek", &(5_000_000i64,));
    // Past the end: the next track.
    fixture.call(PLAYER_INTERFACE, "Seek", &(500_000_000i64,));
    let current = ObjectPath::try_from(track_id(EntryId::new(1))).unwrap();
    fixture.call(PLAYER_INTERFACE, "SetPosition", &(&current, 1_000_000i64));
    // Another track's id, and a position past the end: ignored.
    let other = ObjectPath::try_from(track_id(EntryId::new(2))).unwrap();
    fixture.call(PLAYER_INTERFACE, "SetPosition", &(&other, 1_000_000i64));
    fixture.call(PLAYER_INTERFACE, "SetPosition", &(&current, 900_000_000i64));
    fixture.call(
        PLAYER_INTERFACE,
        "OpenUri",
        &("file:///music/new%20one.ogg",),
    );
    let refused = zbus::block_on(fixture.client.call_method(
        Some(BUS_NAME),
        OBJECT_PATH,
        Some(PLAYER_INTERFACE),
        "OpenUri",
        &("https://example.org/stream.mp3",),
    ));
    assert!(refused.is_err());

    let commands = fixture.take_commands();
    assert_eq!(
        commands,
        [
            PlayerCommand::PlayPause,
            PlayerCommand::Next,
            PlayerCommand::Previous,
            PlayerCommand::Seek(MediaTime::from_duration(Duration::from_secs(5))),
            PlayerCommand::Next,
            PlayerCommand::Seek(MediaTime::from_duration(Duration::from_secs(1))),
            PlayerCommand::PlayFiles {
                files: vec![PathBuf::from("/music/new one.ogg")],
                start: 0
            },
        ]
    );

    // Setters: the command goes out and the new value is announced once.
    fixture.set("Volume", zbus::Variant::from(0.5f64));
    fixture.set("LoopStatus", zbus::Variant::from("Playlist"));
    fixture.set("Shuffle", zbus::Variant::from(true));
    assert_eq!(
        fixture.take_commands(),
        [
            PlayerCommand::SetVolume(Volume::clamped(0.5)),
            PlayerCommand::SetRepeat(RepeatMode::All),
            PlayerCommand::SetShuffle(helikodu_model::player::ShuffleMode::Tracks),
        ]
    );
    let messages = signals.drain(QUIET);
    let changes: Vec<Changes> = messages
        .iter()
        .filter(|m| {
            m.header()
                .member()
                .is_some_and(|m| m.as_str() == "PropertiesChanged")
        })
        .map(properties_changed)
        .collect();
    assert_eq!(changes.len(), 3, "{changes:?}");
    let volume: f64 = changes[0]["Volume"].downcast_ref().unwrap();
    assert_eq!(volume, 0.5);
    assert_eq!(text(&changes[1]["LoopStatus"]), "Playlist");
    assert!(flag(&changes[2]["Shuffle"]));

    // The player confirms: nothing new to announce.
    let modes = helikodu_model::player::PlayModes::new(
        helikodu_model::player::ShuffleMode::Tracks,
        RepeatMode::All,
    );
    {
        let mut status = view.status.lock().unwrap();
        status.volume = Volume::clamped(0.5);
        status.modes = modes;
    }
    fixture
        .mpris
        .handle(&PlayerEvent::VolumeChanged(Volume::clamped(0.5)), view);
    fixture
        .mpris
        .handle(&PlayerEvent::ModesChanged(modes), view);
    assert!(signals.drain(QUIET).is_empty());
}

/// Sets the fake player's queue to the entries `ids` (in play order), with
/// `current` playing, from `context`; returns an event that makes MPRIS look.
fn set_queue(view: &FakeView, ids: &[u64], current: u64, context: ContextSource) -> PlayerEvent {
    let entries = ids
        .iter()
        .map(|id| QueueEntry {
            id: EntryId::new(*id),
            track: TrackRef::File(MediaSource::file(path(*id))),
            from_context: true,
        })
        .collect();
    let index = ids.iter().position(|id| *id == current);
    let version = view.queue.lock().unwrap().version + 1;
    *view.queue.lock().unwrap() = Arc::new(QueueSnapshot {
        version,
        context: QueueContext {
            source: context,
            title: None,
        },
        entries,
        current: index,
        ..QueueSnapshot::default()
    });
    view.status.lock().unwrap().now_playing = Some(NowPlaying {
        entry: EntryId::new(current),
        track: TrackRef::File(MediaSource::file(path(current))),
        path: path(current),
        spec: spec(),
        length: Some(MediaTime::from_millis(200_000)),
    });
    PlayerEvent::QueueChanged { version }
}

fn path_of(id: u64) -> ObjectPath<'static> {
    ObjectPath::try_from(track_id(EntryId::new(id))).unwrap()
}

fn ids(paths: &[OwnedObjectPath]) -> Vec<String> {
    paths.iter().map(|p| p.as_str().to_owned()).collect()
}

fn id_strings(ids: &[u64]) -> Vec<String> {
    ids.iter().map(|id| track_id(EntryId::new(*id))).collect()
}

#[test]
fn the_track_list_follows_the_queue() {
    let Some(fixture) = Fixture::start_with("tracklist", |hooks| {
        hooks.with_describe(|tracks| {
            tracks
                .iter()
                .map(|track| TrackDetails {
                    title: Some(match track {
                        TrackRef::File(source) => format!(
                            "Described {}",
                            source.path().file_name().unwrap().to_string_lossy()
                        ),
                        _ => "Library".into(),
                    }),
                    artists: vec!["Someone".into()],
                    length: Some(Duration::from_secs(61)),
                    ..TrackDetails::default()
                })
                .collect()
        })
    }) else {
        return;
    };
    let view = &fixture.view;
    let signals = Signals::record(&fixture.client, OBJECT_PATH);

    // A new queue replaces the list.
    fixture
        .mpris
        .handle(&set_queue(view, &[1, 2, 3], 1, ContextSource::Files), view);
    let replaced = signals.next("TrackListReplaced", WAIT).unwrap();
    let (tracks, current): (Vec<OwnedObjectPath>, OwnedObjectPath) =
        replaced.body().deserialize().unwrap();
    assert_eq!(ids(&tracks), id_strings(&[1, 2, 3]));
    assert_eq!(current.as_str(), track_id(EntryId::new(1)));
    let invalidated = signals.next("PropertiesChanged", WAIT).unwrap();
    let (interface, changed, gone): (String, Changes, Vec<String>) =
        invalidated.body().deserialize().unwrap();
    assert_eq!(interface, TRACKLIST_INTERFACE);
    assert!(changed.is_empty());
    assert_eq!(gone, ["Tracks"]);
    let shown: Vec<OwnedObjectPath> = fixture
        .get(TRACKLIST_INTERFACE, "Tracks")
        .try_into()
        .unwrap();
    assert_eq!(ids(&shown), id_strings(&[1, 2, 3]));
    assert!(!flag(&fixture.get(TRACKLIST_INTERFACE, "CanEditTracks")));

    // Metadata: the current entry as the player shows it, others looked up;
    // unknown ids are left out.
    let unknown = ObjectPath::try_from("/io/github/sorrelfox/Helikodu/Track/99").unwrap();
    let reply = zbus::block_on(fixture.client.call_method(
        Some(BUS_NAME),
        OBJECT_PATH,
        Some(TRACKLIST_INTERFACE),
        "GetTracksMetadata",
        &(vec![path_of(2), unknown, path_of(1)],),
    ))
    .unwrap();
    let found: Vec<HashMap<String, OwnedVariant>> = reply.body().deserialize().unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(text(&found[0]["xesam:title"]), "Described Track 2.flac");
    assert_eq!(text(&found[0]["xesam:url"]), "file:///music/Track%202.flac");
    let length: i64 = found[0]["mpris:length"].downcast_ref().unwrap();
    assert_eq!(length, 61_000_000);
    let id: ObjectPath<'_> = found[1]["mpris:trackid"].downcast_ref().unwrap();
    assert_eq!(id.as_str(), track_id(EntryId::new(1)));
    assert_eq!(text(&found[1]["xesam:title"]), "Track 1");

    // One entry added at the end, then one removed: their own signals.
    fixture.mpris.handle(
        &set_queue(view, &[1, 2, 3, 4], 1, ContextSource::Files),
        view,
    );
    let added = signals.next("TrackAdded", WAIT).unwrap();
    let (metadata, after): (HashMap<String, OwnedVariant>, OwnedObjectPath) =
        added.body().deserialize().unwrap();
    assert_eq!(after.as_str(), track_id(EntryId::new(3)));
    assert_eq!(text(&metadata["xesam:title"]), "Described Track 4.flac");
    fixture
        .mpris
        .handle(&set_queue(view, &[1, 3, 4], 1, ContextSource::Files), view);
    let removed = signals.next("TrackRemoved", WAIT).unwrap();
    let (gone,): (OwnedObjectPath,) = removed.body().deserialize().unwrap();
    assert_eq!(gone.as_str(), track_id(EntryId::new(2)));
    // At the start: after NoTrack.
    fixture.mpris.handle(
        &set_queue(view, &[5, 1, 3, 4], 1, ContextSource::Files),
        view,
    );
    let added = signals.next("TrackAdded", WAIT).unwrap();
    let (_, after): (HashMap<String, OwnedVariant>, OwnedObjectPath) =
        added.body().deserialize().unwrap();
    assert_eq!(after.as_str(), "/org/mpris/MediaPlayer2/TrackList/NoTrack");

    // GoTo plays a listed entry; other ids reach nobody.
    fixture.take_commands();
    fixture.call(TRACKLIST_INTERFACE, "GoTo", &(path_of(3),));
    fixture.call(TRACKLIST_INTERFACE, "GoTo", &(path_of(2),));
    assert_eq!(
        fixture.take_commands(),
        [PlayerCommand::PlayEntry(EntryId::new(3))]
    );
}

#[derive(Default)]
struct FakePlaylists {
    lists: Arc<Mutex<Vec<MprisPlaylist>>>,
    activated: Arc<Mutex<Vec<u64>>>,
}

impl PlaylistSource for FakePlaylists {
    fn playlists(&self) -> Vec<MprisPlaylist> {
        self.lists.lock().unwrap().clone()
    }

    fn activate(&self, id: u64) {
        self.activated.lock().unwrap().push(id);
    }
}

fn playlist(id: u64, name: &str, created: u64) -> MprisPlaylist {
    let at = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(created);
    MprisPlaylist {
        id,
        name: name.into(),
        created: Some(at),
        modified: Some(at),
    }
}

type PlaylistTuple = (OwnedObjectPath, String, String);

#[test]
fn playlists_are_listed_activated_and_followed() {
    let lists = Arc::new(Mutex::new(vec![
        playlist(1, "workouts", 30),
        playlist(2, "Late evening", 10),
        playlist(3, "Favourites", 20),
    ]));
    let activated = Arc::new(Mutex::new(Vec::new()));
    let source = FakePlaylists {
        lists: Arc::clone(&lists),
        activated: Arc::clone(&activated),
    };
    let Some(fixture) = Fixture::start_with("playlists", |hooks| hooks.with_playlists(source))
    else {
        return;
    };
    let view = &fixture.view;
    let count: u32 = fixture
        .get(PLAYLISTS_INTERFACE, "PlaylistCount")
        .downcast_ref()
        .unwrap();
    assert_eq!(count, 3);
    let orderings: Vec<String> = fixture
        .get(PLAYLISTS_INTERFACE, "Orderings")
        .try_into()
        .unwrap();
    assert!(orderings.contains(&"Alphabetical".to_owned()));
    assert!(orderings.contains(&"UserDefined".to_owned()));

    let get = |index: u32, max: u32, order: &str, reverse: bool| -> Vec<String> {
        let reply = zbus::block_on(fixture.client.call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(PLAYLISTS_INTERFACE),
            "GetPlaylists",
            &(index, max, order, reverse),
        ))
        .unwrap();
        let found: Vec<PlaylistTuple> = reply.body().deserialize().unwrap();
        found.into_iter().map(|(_, name, _)| name).collect()
    };
    assert_eq!(
        get(0, 10, "Alphabetical", false),
        ["Favourites", "Late evening", "workouts"]
    );
    assert_eq!(get(0, 2, "CreationDate", true), ["workouts", "Favourites"]);
    assert_eq!(get(1, 1, "UserDefined", false), ["Late evening"]);

    let active = |fixture: &Fixture| -> (bool, PlaylistTuple) {
        fixture
            .get(PLAYLISTS_INTERFACE, "ActivePlaylist")
            .try_into()
            .unwrap()
    };
    assert!(!active(&fixture).0);

    // The queue comes from a playlist: it is the active one, and that is announced.
    let signals = Signals::record(&fixture.client, OBJECT_PATH);
    fixture.mpris.handle(
        &set_queue(view, &[1, 2], 1, ContextSource::Playlist(2)),
        view,
    );
    let changed = loop {
        let message = signals.next("PropertiesChanged", WAIT).unwrap();
        let (interface, changed, _): (String, Changes, Vec<String>) =
            message.body().deserialize().unwrap();
        if interface == PLAYLISTS_INTERFACE {
            break changed;
        }
    };
    let (valid, (id, name, _)): (bool, PlaylistTuple) = changed["ActivePlaylist"]
        .try_clone()
        .unwrap()
        .try_into()
        .unwrap();
    assert!(valid);
    assert_eq!(id.as_str(), playlist_id(2));
    assert_eq!(name, "Late evening");
    assert_eq!(active(&fixture).1.1, "Late evening");

    // Activation reaches the front end for known playlists only.
    let known = ObjectPath::try_from(playlist_id(3)).unwrap();
    let unknown = ObjectPath::try_from(playlist_id(42)).unwrap();
    fixture.call(PLAYLISTS_INTERFACE, "ActivatePlaylist", &(&known,));
    fixture.call(PLAYLISTS_INTERFACE, "ActivatePlaylist", &(&unknown,));
    assert_eq!(*activated.lock().unwrap(), [3]);

    // A rename and a new playlist: PlaylistChanged, then the count and the
    // active playlist's new name.
    {
        let mut lists = lists.lock().unwrap();
        lists[1].name = "Night".into();
        lists.push(playlist(4, "New", 40));
    }
    fixture.mpris.playlists_changed();
    let renamed = signals.next("PlaylistChanged", WAIT).unwrap();
    let ((id, name, _),): ((OwnedObjectPath, String, String),) =
        renamed.body().deserialize().unwrap();
    assert_eq!(id.as_str(), playlist_id(2));
    assert_eq!(name, "Night");
    let message = signals.next("PropertiesChanged", WAIT).unwrap();
    let (interface, changed, _): (String, Changes, Vec<String>) =
        message.body().deserialize().unwrap();
    assert_eq!(interface, PLAYLISTS_INTERFACE);
    let count: u32 = changed["PlaylistCount"].downcast_ref().unwrap();
    assert_eq!(count, 4);
    assert!(changed.contains_key("ActivePlaylist"));
}

#[test]
fn without_playlists_the_interface_is_not_served() {
    let Some(fixture) = Fixture::start("noplaylists") else {
        return;
    };
    let reply = zbus::block_on(fixture.client.call_method(
        Some(BUS_NAME),
        OBJECT_PATH,
        Some("org.freedesktop.DBus.Introspectable"),
        "Introspect",
        &(),
    ))
    .unwrap();
    let xml: String = reply.body().deserialize().unwrap();
    assert!(xml.contains("interface name=\"org.mpris.MediaPlayer2.TrackList\""));
    assert!(xml.contains("signal name=\"TrackListReplaced\""));
    assert!(!xml.contains("org.mpris.MediaPlayer2.Playlists"));
}
