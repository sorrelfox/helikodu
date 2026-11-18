// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MPRIS2 on the session bus: shell media widgets, lock screens, media keys,
//! KDE Connect, playerctl.
//!
//! [`Mpris::start`] serves `org.mpris.MediaPlayer2` and
//! `org.mpris.MediaPlayer2.Player` at `/org/mpris/MediaPlayer2` under the bus
//! name [`BUS_NAME`]. The player's events drive it: either the front end hands
//! each [`PlayerEvent`] to [`Mpris::handle`] from its main loop, or
//! [`Mpris::follow`] does so on a thread of its own. Calls from clients become
//! [`PlayerCommand`]s through [`MprisHooks`].
//!
//! The rules it keeps:
//!
//! - `mpris:trackid` is a D-Bus object path made from the queue entry
//!   ([`track_id`]), never a file path.
//! - `mpris:artUrl` is a `file://` URL of a copy of the cover thumbnail in a
//!   folder other processes can read ([`default_art_dir`]).
//! - `Position` is never announced; `Seeked` is sent after seeks and when
//!   another track starts.
//! - `PropertiesChanged` is sent only for values that changed, all changes of one
//!   event in one signal.
//! - `LoopStatus` is `None`, `Track` or `Playlist` (repeating the queue, which
//!   holds the album when an album plays); `Shuffle` is true for any shuffle.
//! - `CanGoNext`, `CanGoPrevious`, `CanPlay`, `CanPause` and `CanSeek` follow the
//!   queue: no next track at the end of a queue that does not repeat, no seeking
//!   without a known length.
//! - `OpenUri` takes `file://` URIs only.
//!
//! The calls block for a D-Bus round trip at most; zbus runs its own small
//! executor, not tokio.

mod art;
mod interfaces;
mod state;

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helikodu_model::player::EntryId;
use helikodu_service::bus::Subscription;
use helikodu_service::library::QueueItemInfo;
use helikodu_service::player::{
    CommandSender, Player, PlayerCommand, PlayerEvent, PlayerStatus, QueueSnapshot,
};
use thiserror::Error;
use zbus::Connection;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::names::BusName;

use self::art::ArtExport;
pub use self::art::default_art_dir;
use self::state::{Announcement, Changed, State};

/// The well-known bus name.
pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.io.github.sorrelfox.Helikodu";
/// The object path of both interfaces.
pub const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
/// The root interface.
pub const ROOT_INTERFACE: &str = "org.mpris.MediaPlayer2";
/// The player interface.
pub const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
/// The start of every `mpris:trackid`; the queue entry's number follows.
pub const TRACK_ID_PREFIX: &str = "/io/github/sorrelfox/Helikodu/Track/";

/// The `mpris:trackid` of a queue entry: an object path, unique within the
/// queue, also when one track is queued twice.
pub fn track_id(entry: EntryId) -> String {
    format!("{TRACK_ID_PREFIX}{}", entry.get())
}

/// What the front end looked up about the playing entry, for the metadata.
/// Everything is optional; without a title the file name is shown.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackDetails {
    /// `xesam:title`.
    pub title: Option<String>,
    /// `xesam:artist`.
    pub artists: Vec<String>,
    /// `xesam:album`.
    pub album: Option<String>,
    /// `xesam:albumArtist`.
    pub album_artists: Vec<String>,
    /// `xesam:trackNumber`.
    pub track_number: Option<u32>,
    /// `xesam:discNumber`.
    pub disc_number: Option<u32>,
    /// `xesam:genre`.
    pub genres: Vec<String>,
    /// `xesam:composer`.
    pub composers: Vec<String>,
    /// `xesam:contentCreated`, as a year.
    pub year: Option<i32>,
    /// `xesam:userRating`, in whole stars (0 to 5).
    pub rating: Option<u8>,
    /// `mpris:length`, when it is better known than the decoder's.
    pub length: Option<Duration>,
    /// An image file of the cover, such as the thumbnail the window shows. It is
    /// copied to the art folder for `mpris:artUrl`.
    pub cover: Option<PathBuf>,
}

impl From<&QueueItemInfo> for TrackDetails {
    /// What the queue panel shows. The cover is only a hash there; the front end
    /// sets [`cover`](Self::cover) to the thumbnail file once it has one.
    fn from(info: &QueueItemInfo) -> Self {
        Self {
            title: info.title.clone(),
            artists: info.artist.iter().cloned().collect(),
            album: info.album.clone(),
            track_number: info.track_no,
            length: info.duration,
            ..Self::default()
        }
    }
}

/// How the player looks to MPRIS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MprisConfig {
    /// `Identity`: the name shown by media widgets.
    pub identity: String,
    /// `DesktopEntry`: the `.desktop` file's name without the extension.
    pub desktop_entry: String,
    /// `SupportedMimeTypes`.
    pub supported_mime_types: Vec<String>,
    /// Where the cover for `mpris:artUrl` is written; `None` sends no art.
    pub art_dir: Option<PathBuf>,
}

impl Default for MprisConfig {
    fn default() -> Self {
        Self {
            identity: "Helikodu".to_owned(),
            desktop_entry: crate::APP_ID.to_owned(),
            supported_mime_types: Vec::new(),
            art_dir: default_art_dir(),
        }
    }
}

/// A hook that takes no arguments.
pub type Hook = Arc<dyn Fn() + Send + Sync>;

/// Where the calls of MPRIS clients go. Every hook runs on zbus's executor
/// thread and must return at once (send the command on; a GLib front end
/// forwards `raise` and `quit` to its main context).
#[derive(Clone)]
pub struct MprisHooks {
    /// Every transport, seek, volume and mode change.
    pub command: Arc<dyn Fn(PlayerCommand) + Send + Sync>,
    /// `Raise`: show the window. Without it `CanRaise` is false.
    pub raise: Option<Hook>,
    /// `Quit`: end the program. Without it `CanQuit` is false.
    pub quit: Option<Hook>,
    /// `OpenUri` with a local file. Without it the file replaces the queue and
    /// plays (`PlayerCommand::PlayFiles`).
    pub open: Option<Arc<dyn Fn(PathBuf) + Send + Sync>>,
}

impl MprisHooks {
    /// Hooks that send commands with `command` and do nothing else.
    pub fn new(command: impl Fn(PlayerCommand) + Send + Sync + 'static) -> Self {
        Self {
            command: Arc::new(command),
            raise: None,
            quit: None,
            open: None,
        }
    }

    /// Hooks that send commands to a player.
    pub fn for_player(sender: CommandSender) -> Self {
        Self::new(move |command| {
            // A player that has ended takes no commands; there is nobody to tell.
            let _ = sender.send(command);
        })
    }

    /// The same hooks with `Raise`.
    #[must_use]
    pub fn with_raise(mut self, raise: impl Fn() + Send + Sync + 'static) -> Self {
        self.raise = Some(Arc::new(raise));
        self
    }

    /// The same hooks with `Quit`.
    #[must_use]
    pub fn with_quit(mut self, quit: impl Fn() + Send + Sync + 'static) -> Self {
        self.quit = Some(Arc::new(quit));
        self
    }

    /// The same hooks with `OpenUri`.
    #[must_use]
    pub fn with_open(mut self, open: impl Fn(PathBuf) + Send + Sync + 'static) -> Self {
        self.open = Some(Arc::new(open));
        self
    }
}

impl fmt::Debug for MprisHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MprisHooks")
            .field("raise", &self.raise.is_some())
            .field("quit", &self.quit.is_some())
            .field("open", &self.open.is_some())
            .finish_non_exhaustive()
    }
}

/// The player's state that events do not carry: its status and queue.
pub trait PlayerView {
    /// The status as of the last change.
    fn status(&self) -> PlayerStatus;
    /// The queue as of the last change.
    fn queue(&self) -> Arc<QueueSnapshot>;
}

impl PlayerView for Player {
    fn status(&self) -> PlayerStatus {
        Player::status(self)
    }

    fn queue(&self) -> Arc<QueueSnapshot> {
        Player::queue(self)
    }
}

impl<T: PlayerView + ?Sized> PlayerView for Arc<T> {
    fn status(&self) -> PlayerStatus {
        (**self).status()
    }

    fn queue(&self) -> Arc<QueueSnapshot> {
        (**self).queue()
    }
}

/// Why MPRIS could not be served.
#[derive(Debug, Error)]
pub enum MprisError {
    /// The session bus could not be used.
    #[error("D-Bus: {0}")]
    Bus(#[from] zbus::Error),
    /// Neither the bus name nor a per-process one could be taken.
    #[error("the bus name {0} is taken")]
    NameTaken(String),
}

/// What the interfaces share with the handle.
pub(crate) struct Shared {
    config: MprisConfig,
    hooks: MprisHooks,
    state: Mutex<State>,
}

impl Shared {
    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct Inner {
    connection: Connection,
    bus_name: String,
    shared: Arc<Shared>,
    art: Mutex<ArtExport>,
    /// Held from computing an announcement until it is sent, so that signals go
    /// out in the order of the changes.
    announcing: Mutex<()>,
}

/// The MPRIS service. Clones share it; it ends when the last one is dropped.
#[derive(Clone)]
pub struct Mpris {
    inner: Arc<Inner>,
}

impl fmt::Debug for Mpris {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mpris")
            .field("bus_name", &self.inner.bus_name)
            .finish_non_exhaustive()
    }
}

impl Mpris {
    /// Serves MPRIS on the session bus.
    ///
    /// When [`BUS_NAME`] is taken (another instance), a per-process name
    /// (`….instance<pid>`) is used, as the specification suggests.
    pub fn start(config: MprisConfig, hooks: MprisHooks) -> Result<Self, MprisError> {
        Self::serve(zbus::connection::Builder::session(), config, hooks)
    }

    /// Serves MPRIS on the bus at `address`, such as a private bus in a test.
    pub fn start_at(
        address: &str,
        config: MprisConfig,
        hooks: MprisHooks,
    ) -> Result<Self, MprisError> {
        Self::serve(zbus::connection::Builder::address(address), config, hooks)
    }

    fn serve(
        builder: zbus::connection::Builder<'_>,
        config: MprisConfig,
        hooks: MprisHooks,
    ) -> Result<Self, MprisError> {
        let art = ArtExport::new(config.art_dir.clone());
        let shared = Arc::new(Shared {
            config,
            hooks,
            state: Mutex::new(State::new()),
        });
        let (connection, bus_name) = zbus::block_on(async {
            let connection = builder
                .serve_at(
                    OBJECT_PATH,
                    interfaces::Root {
                        shared: Arc::clone(&shared),
                    },
                )
                .serve_at(
                    OBJECT_PATH,
                    interfaces::Player {
                        shared: Arc::clone(&shared),
                    },
                )
                .build()
                .await?;
            let instance = format!("{BUS_NAME}.instance{}", std::process::id());
            for name in [BUS_NAME.to_owned(), instance] {
                match connection
                    .request_name_with_flags(name.as_str(), RequestNameFlags::DO_NOT_QUEUE)
                    .await
                {
                    Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
                        return Ok((connection, name));
                    }
                    Ok(_) | Err(zbus::Error::NameTaken) => {}
                    Err(error) => return Err(MprisError::Bus(error)),
                }
            }
            Err(MprisError::NameTaken(BUS_NAME.to_owned()))
        })?;
        Ok(Self {
            inner: Arc::new(Inner {
                connection,
                bus_name,
                shared,
                art: Mutex::new(art),
                announcing: Mutex::new(()),
            }),
        })
    }

    /// The bus name taken.
    pub fn bus_name(&self) -> &str {
        &self.inner.bus_name
    }

    /// Takes everything from the player at once, position included: after
    /// [`start`](Self::start), before the first event.
    pub fn sync(&self, view: &dyn PlayerView) {
        let (status, queue) = (view.status(), view.queue());
        self.change(|state| {
            state.sync(&status, &queue, true);
            None
        });
    }

    /// Follows one event of the player. `view` is asked for the status and queue
    /// after events that change more than they say (a new queue, the end of
    /// playback).
    pub fn handle(&self, event: &PlayerEvent, view: &dyn PlayerView) {
        let refresh = !matches!(event, PlayerEvent::Position { .. });
        let snapshot = refresh.then(|| (view.status(), view.queue()));
        self.change(|state| {
            let seeked = state.apply(event, Instant::now());
            if let Some((status, queue)) = &snapshot {
                state.sync(status, queue, false);
            }
            seeked
        });
    }

    /// Shows what the front end looked up about `entry` (normally the entry that
    /// plays; details of another entry wait until it plays). The cover file is
    /// copied to the art folder.
    pub fn set_details(&self, entry: EntryId, details: TrackDetails) {
        let art_url = self.export_art(details.cover.as_deref());
        self.change(|state| {
            state.set_details(entry, details, art_url);
            None
        });
    }

    /// Changes the cover of `entry`, such as when its thumbnail became ready
    /// after [`set_details`](Self::set_details).
    pub fn set_cover(&self, entry: EntryId, cover: Option<&Path>) {
        if self.inner.shared.state().details_entry() != Some(entry) {
            return;
        }
        let art_url = self.export_art(cover);
        self.change(|state| {
            state.set_art_url(entry, art_url);
            None
        });
    }

    /// Follows `events` on a thread of its own, asking `view` for the status and
    /// queue. Synchronises first. The thread ends when the returned handle is
    /// dropped or the subscription ends.
    pub fn follow<V>(
        &self,
        events: Subscription<PlayerEvent>,
        view: V,
    ) -> std::io::Result<MprisFollower>
    where
        V: PlayerView + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let mpris = self.clone();
        let stopping = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("helikodu-mpris".into())
            .spawn(move || {
                mpris.sync(&view);
                while !stopping.load(Ordering::Acquire) {
                    match events.recv_timeout(Duration::from_millis(100)) {
                        Ok(event) => mpris.handle(&event, &view),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;
        Ok(MprisFollower {
            stop,
            thread: Some(thread),
        })
    }

    fn export_art(&self, cover: Option<&Path>) -> Option<String> {
        let mut art = self
            .inner
            .art
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Without a readable cover, the metadata simply has no art.
        art.export(cover).ok().flatten()
    }

    /// Changes the state and announces what changed.
    fn change(&self, change: impl FnOnce(&mut State) -> Option<i64>) {
        let _order = self
            .inner
            .announcing
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let announcement = {
            let mut state = self.inner.shared.state();
            let seeked = change(&mut state);
            state.announce(seeked)
        };
        if !announcement.is_empty() {
            // A client that is gone or a full socket loses one update; the next
            // one carries the current values again.
            let _ = zbus::block_on(announce(&self.inner.connection, announcement));
        }
    }
}

/// Sends `PropertiesChanged` and `Seeked`.
async fn announce(connection: &Connection, announcement: Announcement) -> zbus::Result<()> {
    if !announcement.changed.is_empty() {
        let mut changed: HashMap<&str, zbus::OwnedVariant> = HashMap::new();
        for change in &announcement.changed {
            let (name, value) = match change {
                Changed::Text(name, text) => {
                    (*name, zbus::OwnedVariant::from(zbus::Str::from(*text)))
                }
                Changed::Flag(name, flag) => (*name, zbus::OwnedVariant::from(*flag)),
                Changed::Real(name, value) => (*name, zbus::OwnedVariant::from(*value)),
                Changed::Metadata(metadata) => (
                    "Metadata",
                    zbus::OwnedVariant::from(interfaces::to_dbus(metadata)),
                ),
            };
            changed.insert(name, value);
        }
        connection
            .emit_signal(
                None::<BusName<'_>>,
                OBJECT_PATH,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &(PLAYER_INTERFACE, changed, Vec::<&str>::new()),
            )
            .await?;
    }
    if let Some(position) = announcement.seeked {
        connection
            .emit_signal(
                None::<BusName<'_>>,
                OBJECT_PATH,
                PLAYER_INTERFACE,
                "Seeked",
                &(position,),
            )
            .await?;
    }
    Ok(())
}

/// The thread of [`Mpris::follow`]; dropping it ends the thread.
#[derive(Debug)]
pub struct MprisFollower {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for MprisFollower {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The `file://` URL of an absolute path, with every byte outside the unreserved
/// set and `/` percent-encoded. `None` for a relative path.
pub fn file_uri(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;

    if !path.is_absolute() {
        return None;
    }
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Some(uri)
}

/// The local path of a `file://` URI (with an empty host or `localhost`), with
/// percent-escapes decoded. `None` for anything else.
pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut path = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3)?;
                let hex = std::str::from_utf8(hex).ok()?;
                path.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'?' | b'#' => break,
            byte => {
                path.push(byte);
                i += 1;
            }
        }
    }
    Some(PathBuf::from(OsString::from_vec(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_ids_are_object_paths() {
        let id = track_id(EntryId::new(42));
        assert_eq!(id, "/io/github/sorrelfox/Helikodu/Track/42");
        assert!(zbus::ObjectPath::try_from(id.as_str()).is_ok());
    }

    #[test]
    fn file_uris_round_trip() {
        use std::os::unix::ffi::OsStrExt;

        let odd = Path::new(std::ffi::OsStr::from_bytes(b"/m\xFCsic/a b#1%.flac"));
        let uri = file_uri(odd).unwrap();
        assert_eq!(uri, "file:///m%FCsic/a%20b%231%25.flac");
        assert_eq!(path_from_uri(&uri).unwrap(), odd);
        assert_eq!(
            path_from_uri("file://localhost/x/y%C3%A9.ogg").unwrap(),
            Path::new("/x/yé.ogg")
        );
        assert_eq!(file_uri(Path::new("relative.flac")), None);
        assert_eq!(path_from_uri("https://example.org/a.mp3"), None);
        assert_eq!(path_from_uri("file://otherhost/a.mp3"), None);
        assert_eq!(path_from_uri("file:///a%2"), None);
    }

    #[test]
    fn queue_items_become_details() {
        let info = QueueItemInfo {
            track: helikodu_model::library::TrackRef::Library(
                helikodu_model::library::TrackId::new(1),
            ),
            title: Some("Title".into()),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            track_no: Some(2),
            duration: Some(Duration::from_secs(61)),
            cover: None,
            path: None,
        };
        let details = TrackDetails::from(&info);
        assert_eq!(details.title.as_deref(), Some("Title"));
        assert_eq!(details.artists, ["Artist"]);
        assert_eq!(details.track_number, Some(2));
        assert_eq!(details.length, Some(Duration::from_secs(61)));
    }
}
