// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MPRIS driven by a fake player, seen by a client on the same private bus.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helikodu_desktop::mpris::{
    BUS_NAME, Mpris, MprisConfig, MprisHooks, OBJECT_PATH, PLAYER_INTERFACE, PlayerView,
    ROOT_INTERFACE, TrackDetails, track_id,
};
use helikodu_model::audio::{
    AudioSpec, ChannelLayout, Endianness, MediaTime, SampleFormat, SampleRate,
};
use helikodu_model::library::{MediaSource, TrackRef};
use helikodu_model::player::{
    EntryId, PlaybackState, QueueEntry, RepeatMode, TransportState, Volume,
};
use helikodu_service::player::{
    NowPlaying, PlayerCommand, PlayerEvent, PlayerStatus, QueueSnapshot,
};
use zbus::{Connection, Message, ObjectPath, OwnedVariant};

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
        let bus = PrivateBus::start()?;
        let commands = Arc::new(Mutex::new(Vec::new()));
        let raised = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::clone(&commands);
        let raises = Arc::clone(&raised);
        let hooks = MprisHooks::new(move |command| recorded.lock().unwrap().push(command))
            .with_raise(move || {
                raises.fetch_add(1, Ordering::SeqCst);
            });
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
    assert!(!flag(&fixture.get(ROOT_INTERFACE, "HasTrackList")));
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
    let changed = properties_changed(&signals.next("PropertiesChanged", WAIT).unwrap());
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
    let changed = properties_changed(&signals.next("PropertiesChanged", WAIT).unwrap());
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
    let changed = properties_changed(&signals.next("PropertiesChanged", WAIT).unwrap());
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
    let changed = properties_changed(&signals.next("PropertiesChanged", WAIT).unwrap());
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
    let modes = helikodu_model::player::PlayModes {
        shuffle: helikodu_model::player::ShuffleMode::Tracks,
        repeat: RepeatMode::All,
    };
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
