// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Serves the application's D-Bus interface (`helikodu_desktop::app_api`) on
//! the connection the application owns its bus name on, for `helikodu-cli` and
//! scripts.
//!
//! Every call becomes a request of the window's backend, so playback asked for
//! from outside passes the same full-output confirmation as the window's own;
//! library lookups run on the worker threads and the reply follows when they
//! are done. The status and the queue come from the player state the window
//! follows.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use helikodu_desktop::app_api::{
    ERROR_FAILED, ERROR_INVALID_ARGS, INTERFACE, INTERFACE_XML, OBJECT_PATH, VERSION,
};
use serde_json::{Value, json};

use crate::backend::{
    Backend, PlaybackState, PlayerRequest, PlaylistType, RadioSeed, RepeatMode, ShuffleMode,
    SignalState, TrackKey, TrackQuery, TrackRow,
};
use crate::play_modes::{repeat_id, shuffle_id};
use crate::playback::{Playback, PlayerView};

/// The shuffle modes by the names the interface takes.
const SHUFFLE_MODES: [ShuffleMode; 6] = [
    ShuffleMode::Off,
    ShuffleMode::Tracks,
    ShuffleMode::Albums,
    ShuffleMode::Folders,
    ShuffleMode::Groupings,
    ShuffleMode::WithinAlbum,
];

/// The repeat modes by the names the interface takes.
const REPEAT_MODES: [RepeatMode; 6] = [
    RepeatMode::Off,
    RepeatMode::One,
    RepeatMode::Album,
    RepeatMode::All,
    RepeatMode::Endless,
    RepeatMode::AutoDj,
];

/// The most tracks a query plays or enqueues at once.
const QUERY_LIMIT: usize = 10_000;

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn state_name(state: PlaybackState) -> &'static str {
    match state {
        PlaybackState::Idle => "stopped",
        PlaybackState::Loading => "loading",
        PlaybackState::Playing => "playing",
        PlaybackState::Paused => "paused",
    }
}

fn signal_name(signal: SignalState) -> &'static str {
    match signal {
        SignalState::Idle => "idle",
        SignalState::Unreported => "unreported",
        SignalState::BitPerfect => "bit-perfect",
        SignalState::Processed { .. } => "processed",
        SignalState::NotConnected => "not-connected",
    }
}

/// A track as the JSON of the interface gives it.
fn track_json(row: &TrackRow) -> Value {
    json!({
        "id": (row.key.0 > 0).then_some(row.key.0),
        "title": row.title,
        "artist": row.artist,
        "album": row.album_title,
        "year": row.year,
        "track_number": row.number,
        "disc_number": row.disc,
        "length_ms": row.duration.map(millis),
        "rating": row.rating,
        "plays": row.plays,
        "path": row.path.as_ref().map(|p| p.to_string_lossy()),
        "codec": row.format.codec,
        "sample_rate": row.format.sample_rate,
        "bits": row.format.bits,
    })
}

/// The status of the player as JSON.
pub(crate) fn status_json(view: &PlayerView) -> Value {
    json!({
        "api_version": VERSION,
        "state": state_name(view.state),
        "track": view.now.as_ref().map(|now| track_json(&now.track)),
        "position_ms": view.now.as_ref().map(|_| millis(view.elapsed)),
        "length_ms": view.length.map(millis),
        "volume": view.volume,
        "shuffle": shuffle_id(view.modes.shuffle),
        "repeat": repeat_id(view.modes.repeat),
        "stop_after": view.stop_after,
        "queue_length": view.queue.items.len(),
        "queue_position": view.queue.current,
        "output": {
            "device": view.output.device,
            "signal": signal_name(view.output.signal),
            "sample_rate": view.output.sample_rate,
            "bits": view.output.bits,
        },
    })
}

/// The queue as JSON.
pub(crate) fn queue_json(view: &PlayerView) -> Value {
    json!({
        "context": view.queue.context,
        "current": view.queue.current,
        "tracks": view.queue.items.iter().map(track_json).collect::<Vec<_>>(),
    })
}

/// What a call asks for, read from its arguments.
#[derive(Debug, Clone, PartialEq)]
enum Call {
    Status,
    Queue,
    Playlists,
    Request(PlayerRequest),
    Pause,
    Enqueue(Vec<PathBuf>, bool),
    EnqueueQuery(String, bool),
    PlayQuery(String, bool),
    Rate(u64, Option<u8>),
    Export(String, PathBuf),
}

/// Reads a method call; `Err` names what is wrong with its arguments.
fn read_call(method: &str, args: &glib::Variant) -> Result<Call, String> {
    let bad = || format!("wrong arguments for {method}");
    Ok(match method {
        "GetStatus" => Call::Status,
        "GetQueue" => Call::Queue,
        "ListPlaylists" => Call::Playlists,
        "PlayPause" => Call::Request(PlayerRequest::PlayPause),
        "Play" => Call::Request(PlayerRequest::Play),
        "Pause" => Call::Pause,
        "Stop" => Call::Request(PlayerRequest::Stop),
        "Next" => Call::Request(PlayerRequest::Next),
        "Previous" => Call::Request(PlayerRequest::Previous),
        "Seek" => {
            let (offset,) = args.get::<(i64,)>().ok_or_else(bad)?;
            Call::Request(PlayerRequest::SeekBy(offset as f64 / 1000.0))
        }
        "SetPosition" => {
            let (position,) = args.get::<(u64,)>().ok_or_else(bad)?;
            Call::Request(PlayerRequest::Seek(Duration::from_millis(position)))
        }
        "SetVolume" => {
            let (volume,) = args.get::<(f64,)>().ok_or_else(bad)?;
            if !(0.0..=1.0).contains(&volume) {
                return Err("the volume is from 0.0 to 1.0".into());
            }
            Call::Request(PlayerRequest::SetVolume(volume))
        }
        "SetShuffle" => {
            let (name,) = args.get::<(String,)>().ok_or_else(bad)?;
            let mode = SHUFFLE_MODES
                .into_iter()
                .find(|m| shuffle_id(*m) == name)
                .ok_or_else(|| format!("no shuffle mode {name}"))?;
            Call::Request(PlayerRequest::SetShuffleMode(mode))
        }
        "SetRepeat" => {
            let (name,) = args.get::<(String,)>().ok_or_else(bad)?;
            let mode = REPEAT_MODES
                .into_iter()
                .find(|m| repeat_id(*m) == name)
                .ok_or_else(|| format!("no repeat mode {name}"))?;
            Call::Request(PlayerRequest::SetRepeatMode(mode))
        }
        "Enqueue" => {
            let (files, next) = args.get::<(Vec<String>, bool)>().ok_or_else(bad)?;
            let files: Vec<PathBuf> = files
                .into_iter()
                .map(|f| match glib::filename_from_uri(&f) {
                    Ok((path, _)) => path,
                    Err(_) => PathBuf::from(f),
                })
                .collect();
            if files.iter().any(|f| !f.is_absolute()) {
                return Err("files must be absolute paths or file:// URIs".into());
            }
            Call::Enqueue(files, next)
        }
        "EnqueueQuery" => {
            let (query, next) = args.get::<(String, bool)>().ok_or_else(bad)?;
            Call::EnqueueQuery(query, next)
        }
        "PlayQuery" => {
            let (query, shuffle) = args.get::<(String, bool)>().ok_or_else(bad)?;
            Call::PlayQuery(query, shuffle)
        }
        "Rate" => {
            let (track, stars) = args.get::<(u64, i32)>().ok_or_else(bad)?;
            let stars = match stars {
                -1 => None,
                0..=5 => Some(u8::try_from(stars).unwrap_or(0)),
                _ => return Err("a rating is 0 to 5 stars, or -1 for none".into()),
            };
            Call::Rate(track, stars)
        }
        "ExportPlaylist" => {
            let (playlist, path) = args.get::<(String, String)>().ok_or_else(bad)?;
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err("the file must be an absolute path".into());
            }
            Call::Export(playlist, path)
        }
        "Revert" => Call::Request(PlayerRequest::Revert),
        "SetSpeed" => {
            let (rate, keep_pitch) = args.get::<(f64, bool)>().ok_or_else(bad)?;
            if !(0.5..=2.0).contains(&rate) {
                return Err("the speed is from 0.5 to 2.0".into());
            }
            Call::Request(PlayerRequest::SetSpeed { rate, keep_pitch })
        }
        "RadioRandom" => Call::Request(PlayerRequest::RadioRandom),
        "SetIntroScan" => {
            let (on,) = args.get::<(bool,)>().ok_or_else(bad)?;
            Call::Request(PlayerRequest::SetIntroScan(on))
        }
        "StartRadio" => {
            let (kind, id) = args.get::<(String, u64)>().ok_or_else(bad)?;
            let number = i64::try_from(id).map_err(|_| bad())?;
            let seed = match kind.as_str() {
                "track" => RadioSeed::Track(id),
                "album" => RadioSeed::Album(number),
                "artist" => RadioSeed::Artist(number),
                _ => return Err("the kind is track, album or artist".into()),
            };
            Call::Request(PlayerRequest::StartRadio(seed))
        }
        "PlayTracks" => {
            let (tracks, title) = args.get::<(Vec<u64>, String)>().ok_or_else(bad)?;
            if tracks.is_empty() {
                return Err("there are no tracks to play".into());
            }
            Call::Request(PlayerRequest::PlayTracks {
                tracks: tracks.into_iter().map(TrackKey).collect(),
                start: 0,
                title,
                shuffle: false,
            })
        }
        other => return Err(format!("no method {other}")),
    })
}

/// The running service of the interface; dropping it does not unregister it, the
/// window does when it closes.
#[derive(Debug)]
struct Server {
    backend: Backend,
    playback: Rc<Playback>,
}

impl Server {
    fn handle(
        self: &Rc<Self>,
        method: &str,
        args: &glib::Variant,
        call: gio::DBusMethodInvocation,
    ) {
        let request = match read_call(method, args) {
            Ok(request) => request,
            Err(message) => {
                call.return_dbus_error(ERROR_INVALID_ARGS, &message);
                return;
            }
        };
        let view = self.playback.view();
        match request {
            Call::Status => reply_text(call, &status_json(&view)),
            Call::Queue => reply_text(call, &queue_json(&view)),
            Call::Request(request) => {
                self.backend.send(request);
                call.return_value(None);
            }
            Call::Pause => {
                if view.state == PlaybackState::Playing {
                    self.backend.send(PlayerRequest::PlayPause);
                }
                call.return_value(None);
            }
            Call::Enqueue(files, next) => {
                let count = u32::try_from(files.len()).unwrap_or(u32::MAX);
                self.backend
                    .send(PlayerRequest::EnqueueFiles { files, next });
                call.return_value(Some(&(count,).to_variant()));
            }
            Call::Playlists => {
                self.query(call, |library| {
                    let rows = library.playlists()?;
                    let list: Vec<Value> = rows
                        .iter()
                        .filter(|r| r.kind != PlaylistType::Folder)
                        .map(|r| {
                            json!({
                                "id": r.key.0,
                                "name": r.name,
                                "kind": match r.kind {
                                    PlaylistType::Smart => "smart",
                                    PlaylistType::Generator => "generator",
                                    _ => "static",
                                },
                                "tracks": r.items,
                                "length_ms": millis(r.duration),
                            })
                        })
                        .collect();
                    Ok(Reply::Text(Value::Array(list)))
                });
            }
            Call::EnqueueQuery(query, next) => {
                let backend = self.backend.clone();
                self.tracks_of(call, query, move |tracks| {
                    let count = tracks.len();
                    backend.send(PlayerRequest::Enqueue { tracks, next });
                    count
                });
            }
            Call::PlayQuery(query, shuffle) => {
                let backend = self.backend.clone();
                let title = query.clone();
                self.tracks_of(call, query, move |tracks| {
                    let count = tracks.len();
                    if count > 0 {
                        backend.send(PlayerRequest::PlayTracks {
                            tracks,
                            start: 0,
                            title,
                            shuffle,
                        });
                    }
                    count
                });
            }
            Call::Rate(track, stars) => {
                let key = if track == 0 {
                    match view.now.as_ref() {
                        Some(now) if now.track.key.0 > 0 => now.track.key,
                        _ => {
                            call.return_dbus_error(ERROR_FAILED, "no library track is playing");
                            return;
                        }
                    }
                } else {
                    TrackKey(track)
                };
                self.query(call, move |library| {
                    library.set_rating(key, stars)?;
                    Ok(Reply::Nothing)
                });
            }
            Call::Export(playlist, path) => {
                self.query(call, move |library| {
                    let rows = library.playlists()?;
                    let wanted = playlist.trim();
                    let found = rows
                        .iter()
                        .filter(|r| r.kind != PlaylistType::Folder)
                        .find(|r| {
                            r.name == wanted || wanted.parse::<u64>().is_ok_and(|n| n == r.key.0)
                        })
                        .or_else(|| {
                            rows.iter()
                                .filter(|r| r.kind != PlaylistType::Folder)
                                .find(|r| r.name.to_lowercase() == wanted.to_lowercase())
                        })
                        .ok_or_else(|| {
                            crate::backend::BackendError(format!("there is no playlist {wanted}"))
                        })?;
                    let count = library.export_playlist(found.key, &path)?;
                    Ok(Reply::Count(u32::try_from(count).unwrap_or(u32::MAX)))
                });
            }
        }
    }

    /// Runs `job` on a worker thread and replies with its result.
    fn query<F>(&self, call: gio::DBusMethodInvocation, job: F)
    where
        F: FnOnce(
                &dyn crate::backend::LibrarySource,
            ) -> Result<Reply, crate::backend::BackendError>
            + Send
            + 'static,
    {
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            match backend.query(job).await {
                Ok(Reply::Nothing) => call.return_value(None),
                Ok(Reply::Count(n)) => call.return_value(Some(&(n,).to_variant())),
                Ok(Reply::Text(value)) => reply_text(call, &value),
                Err(error) => call.return_dbus_error(ERROR_FAILED, &error.0),
            }
        });
    }

    /// Finds the tracks of `query`, hands them to `then` on the main loop and
    /// replies with the number it returns.
    fn tracks_of(
        &self,
        call: gio::DBusMethodInvocation,
        query: String,
        then: impl FnOnce(Vec<TrackKey>) -> usize + 'static,
    ) {
        let text = query.trim().to_owned();
        if text.is_empty() {
            call.return_dbus_error(ERROR_INVALID_ARGS, "the query is empty");
            return;
        }
        let backend = self.backend.clone();
        glib::spawn_future_local(async move {
            let found = backend
                .query(move |library| {
                    let mut keys = library.track_keys(&TrackQuery {
                        text,
                        ..TrackQuery::default()
                    })?;
                    keys.truncate(QUERY_LIMIT);
                    Ok(keys)
                })
                .await;
            match found {
                Ok(keys) => {
                    let count = then(keys);
                    call.return_value(Some(
                        &(u32::try_from(count).unwrap_or(u32::MAX),).to_variant(),
                    ));
                }
                Err(error) => call.return_dbus_error(ERROR_FAILED, &error.0),
            }
        });
    }
}

/// What a library call replies.
enum Reply {
    Nothing,
    Count(u32),
    Text(Value),
}

fn reply_text(call: gio::DBusMethodInvocation, value: &Value) {
    call.return_value(Some(&(value.to_string(),).to_variant()));
}

/// Serves the interface on `application`'s bus connection while `window`
/// lives. Does nothing when the application has no bus connection (a second
/// instance or tests without a session bus).
pub(crate) fn export(window: &impl IsA<gtk::Window>, backend: &Backend, playback: &Rc<Playback>) {
    let Some(application) = window.as_ref().application() else {
        return;
    };
    let Some(connection) = application.dbus_connection() else {
        return;
    };
    let path = application
        .dbus_object_path()
        .map_or_else(|| OBJECT_PATH.to_owned(), |p| p.to_string());
    let info = match gio::DBusNodeInfo::for_xml(INTERFACE_XML) {
        Ok(node) => node.lookup_interface(INTERFACE),
        Err(error) => {
            glib::g_warning!("helikodu", "the D-Bus interface does not parse: {error}");
            return;
        }
    };
    let Some(info) = info else { return };
    let server = Rc::new(Server {
        backend: backend.clone(),
        playback: playback.clone(),
    });
    // One window serves at a time: a newer one takes over from an older one.
    release(None);
    let registered = connection
        .register_object(&path, &info)
        .method_call(move |_, _, _, _, method, args, call| {
            server.handle(method, &args, call);
        })
        .property(|_, _, _, _, name| match name {
            "Version" => VERSION.to_variant(),
            _ => glib::Variant::from_none(glib::VariantTy::UINT32),
        })
        .build();
    match registered {
        Ok(id) => {
            let window = window.as_ref();
            SERVING.with(|serving| {
                serving.replace(Some(Serving {
                    connection,
                    id,
                    window: window.downgrade(),
                }));
            });
            window.connect_destroy(|window| release(Some(window)));
        }
        Err(error) => glib::g_warning!("helikodu", "cannot serve the D-Bus interface: {error}"),
    }
}

/// The registration of the window that serves the interface.
struct Serving {
    connection: gio::DBusConnection,
    id: gio::RegistrationId,
    window: glib::WeakRef<gtk::Window>,
}

thread_local! {
    static SERVING: std::cell::RefCell<Option<Serving>> = const { std::cell::RefCell::new(None) };
}

/// Stops serving: for `window` only if it is the one that serves, or in any
/// case with `None`.
fn release(window: Option<&gtk::Window>) {
    SERVING.with(|serving| {
        let mine = match (window, serving.borrow().as_ref()) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(window), Some(current)) => current.window.upgrade().is_none_or(|w| w == *window),
        };
        if mine && let Some(old) = serving.take() {
            let _ = old.connection.unregister_object(old.id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_are_read_from_their_arguments() {
        let none = ().to_variant();
        assert_eq!(read_call("GetStatus", &none), Ok(Call::Status));
        assert_eq!(
            read_call("Seek", &(-5000i64,).to_variant()),
            Ok(Call::Request(PlayerRequest::SeekBy(-5.0)))
        );
        assert_eq!(
            read_call("SetShuffle", &("albums",).to_variant()),
            Ok(Call::Request(PlayerRequest::SetShuffleMode(
                ShuffleMode::Albums
            )))
        );
        assert!(read_call("SetRepeat", &("sometimes",).to_variant()).is_err());
        assert_eq!(
            read_call("SetRepeat", &("auto-dj",).to_variant()),
            Ok(Call::Request(PlayerRequest::SetRepeatMode(
                RepeatMode::AutoDj
            )))
        );
        assert_eq!(
            read_call("StartRadio", &("album".to_owned(), 4u64).to_variant()),
            Ok(Call::Request(PlayerRequest::StartRadio(RadioSeed::Album(
                4
            ))))
        );
        assert!(read_call("StartRadio", &("genre".to_owned(), 4u64).to_variant()).is_err());
        assert_eq!(
            read_call("SetIntroScan", &(true,).to_variant()),
            Ok(Call::Request(PlayerRequest::SetIntroScan(true)))
        );
        assert!(
            read_call(
                "PlayTracks",
                &(Vec::<u64>::new(), "x".to_owned()).to_variant()
            )
            .is_err()
        );
        assert!(read_call("SetVolume", &(1.5f64,).to_variant()).is_err());
        assert_eq!(
            read_call("SetSpeed", &(1.25f64, false).to_variant()),
            Ok(Call::Request(PlayerRequest::SetSpeed {
                rate: 1.25,
                keep_pitch: false
            }))
        );
        assert!(read_call("SetSpeed", &(3.0f64, true).to_variant()).is_err());
        assert!(read_call("SetSpeed", &(f64::NAN, true).to_variant()).is_err());
        assert_eq!(
            read_call("Rate", &(0u64, -1i32).to_variant()),
            Ok(Call::Rate(0, None))
        );
        assert!(read_call("Rate", &(0u64, 6i32).to_variant()).is_err());
        assert_eq!(
            read_call(
                "Enqueue",
                &(vec!["file:///music/a%20b.flac".to_owned()], true).to_variant()
            ),
            Ok(Call::Enqueue(vec![PathBuf::from("/music/a b.flac")], true))
        );
        assert!(read_call("Enqueue", &(vec!["a.flac".to_owned()], true).to_variant()).is_err());
        assert!(read_call("Seek", &("x",).to_variant()).is_err());
    }

    /// Any caller on the session bus can send anything: extreme numbers, the
    /// wrong types, strange text. Each call is read or refused, never a panic.
    #[test]
    fn hostile_arguments_are_read_or_refused() {
        let methods = [
            "GetStatus",
            "GetQueue",
            "ListPlaylists",
            "PlayPause",
            "Play",
            "Pause",
            "Stop",
            "Next",
            "Previous",
            "Seek",
            "SetPosition",
            "SetVolume",
            "SetSpeed",
            "SetShuffle",
            "SetRepeat",
            "Enqueue",
            "EnqueueQuery",
            "PlayQuery",
            "Rate",
            "ExportPlaylist",
            "",
            "seek",
        ];
        let strings = [
            "",
            "\u{7}",
            "file://",
            "file:///",
            "file://host/x",
            "file:///%00",
            "file:///%zz",
            "file:///../../etc",
            "/../../etc/passwd",
            "relative",
            "http://example.invalid/a",
            "\u{202e}",
            "/a\nb",
        ];
        let mut args: Vec<glib::Variant> = vec![
            ().to_variant(),
            (i64::MIN,).to_variant(),
            (i64::MAX,).to_variant(),
            (u64::MAX,).to_variant(),
            (0u64,).to_variant(),
            (f64::NAN,).to_variant(),
            (f64::INFINITY,).to_variant(),
            (-0.0f64,).to_variant(),
            (u64::MAX, i32::MIN).to_variant(),
            (u64::MAX, i32::MAX).to_variant(),
            (true,).to_variant(),
            (vec![String::new()], true).to_variant(),
        ];
        for text in strings {
            args.push((text,).to_variant());
            args.push((text, true).to_variant());
            args.push((text, text).to_variant());
            args.push((vec![text.to_owned(); 3], false).to_variant());
        }
        for method in methods {
            for arg in &args {
                if let Ok(Call::Enqueue(files, _)) = read_call(method, arg) {
                    assert!(files.iter().all(|f| f.is_absolute()), "{files:?}");
                }
                if let Ok(Call::Export(_, path)) = read_call(method, arg) {
                    assert!(path.is_absolute(), "{path:?}");
                }
                if let Ok(Call::Request(PlayerRequest::SetVolume(v))) = read_call(method, arg) {
                    assert!((0.0..=1.0).contains(&v), "{v}");
                }
                if let Ok(Call::Request(PlayerRequest::SeekBy(by))) = read_call(method, arg) {
                    assert!(by.is_finite());
                    let _ = Duration::from_secs_f64(by.abs());
                }
            }
        }
    }

    #[test]
    fn the_status_is_json() {
        let view = PlayerView::default();
        let status = status_json(&view);
        assert_eq!(status["state"], "stopped");
        assert_eq!(status["api_version"], 1);
        assert!(status["track"].is_null());
        assert_eq!(queue_json(&view)["tracks"], json!([]));
    }
}
