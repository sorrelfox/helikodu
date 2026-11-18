# helikodu-desktop

MPRIS2, the application D-Bus API, notifications, suspend inhibition, the Settings and GlobalShortcuts portals, real-time priority, the tray icon and the audio device reservation helper. Nothing here is specific to one desktop environment.

- **May depend on:** `model`, `service`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

Everything here talks D-Bus through zbus with its own small executor (`default-rt`), not tokio; the XDG portals go through `ashpd` built the same way (`async-io`, only the Settings, Inhibit, OpenURI and GlobalShortcuts portals). Every connection is to the session bus's Unix socket, or to the system bus for the logind fallback.

| Module | What it does |
|---|---|
| `mpris.rs`, `mpris/` | MPRIS2 (`org.mpris.MediaPlayer2`, `.Player`, `.TrackList` and `.Playlists`) under `org.mpris.MediaPlayer2.io.github.sorrelfox.Helikodu` |
| `power.rs` | Suspend inhibition while playing: the Inhibit portal, logind outside a sandbox |
| `portals.rs` | Colour scheme, accent colour, contrast and reduced motion from the Settings portal |
| `open_folder.rs` | "Open containing folder": `org.freedesktop.FileManager1.ShowItems`, or the OpenURI portal |
| `reservation.rs`, `reservation/` | Audio device reservation (`org.freedesktop.ReserveDevice1`) |
| `tray.rs`, `tray/` | The tray icon: `org.kde.StatusNotifierItem` and its menu as `com.canonical.dbusmenu` |
| `global_shortcuts.rs` | Shortcuts in other applications through the GlobalShortcuts portal |
| `session_watch.rs` | The screen lock and sleep: the Inhibit portal's session monitor and logind |

## MPRIS

`Mpris::start(MprisConfig, MprisHooks)` serves the interfaces at `/org/mpris/MediaPlayer2` and takes the bus name (a second instance falls back to `….instance<pid>`, as the specification suggests). Client calls become `PlayerCommand`s through `MprisHooks::command` (`MprisHooks::for_player(player.sender())` sends them to the player); `Raise`, `Quit` and `OpenUri` go to optional hooks, and `CanRaise`/`CanQuit` say whether they are set. `OpenUri` accepts `file://` URIs only; without an `open` hook the file replaces the queue and plays.

The player's events drive it, in one of two ways:

- **From the front end's main loop.** For each `PlayerEvent` the front end already receives, call `mpris.handle(&event, service.player())` (`Player` implements `PlayerView`, which gives the status and queue snapshots that events such as `QueueChanged` do not carry). Call `mpris.sync(service.player())` once after starting.
- **On a thread of its own.** `mpris.follow(player.subscribe(SubscribeOptions::player()), view)` with a `PlayerView + Send` (for example `Arc<Player>`); dropping the returned `MprisFollower` ends the thread.

What it keeps to:

- `mpris:trackid` is `/io/github/sorrelfox/Helikodu/Track/<entry>`, made from the queue entry, so a track queued twice has two IDs. With nothing current it is `/org/mpris/MediaPlayer2/TrackList/NoTrack`.
- Until the front end has looked the track up, the metadata has the trackid, `mpris:length`, `xesam:url` and the file name as `xesam:title`. `mpris.set_details(entry, TrackDetails { … })` adds title, artists, album, album artists, track and disc number, genres, composers, year (`xesam:contentCreated`), rating (`xesam:userRating`) and a cover file. `TrackDetails::from(&QueueItemInfo)` fills what the queue panel already knows.
- The cover file is copied to `$XDG_RUNTIME_DIR/helikodu/cover-<name of the source file>` and announced as `mpris:artUrl`; inside Flatpak to `$XDG_RUNTIME_DIR/app/$FLATPAK_ID/`, which the host sees at the same path. The previous copy is removed.
- `PropertiesChanged` carries only what changed, all changes of one event in one signal. `Position` is never announced: `Seeked` follows every track change (position 0) and every position more than a second away from where interpolation puts it (a seek, a resumed long track).
- `PlaybackStatus` shows `Buffering` as `Playing`. `LoopStatus` maps repeat off/one/all to `None`/`Track`/`Playlist`; `Shuffle` is true for any shuffle mode. `Volume` is the slider position (0.0–1.0) the window shows. `Rate` is fixed at 1.0; setting it to 0 pauses.
- `CanGoNext` is false at the end of a queue that does not repeat; `CanGoPrevious` and `CanPause` are true while an entry is current; `CanPlay` while the queue has entries; `CanSeek` while the length is known; `CanControl` is always true. Calls the capabilities rule out are ignored. `Seek` past the end goes to the next track; `SetPosition` with another track's ID or past the end is ignored.
- Setting `Volume`, `LoopStatus` or `Shuffle` announces the new value at once and sends the command; the player's confirmation then changes nothing.

`SupportedMimeTypes` is whatever `MprisConfig::supported_mime_types` holds; the application fills it from the decoder registry.

### Track list

`HasTrackList` is true. `Tracks` is the queue in play order; a queue longer than `TRACK_LIST_LIMIT` (1000) shows 1000 entries starting at a multiple of 500 before the current entry, so the shown part moves seldom. One entry added or removed is announced with `TrackAdded` (after the entry before it, or `NoTrack` at the start) or `TrackRemoved`, any other change with `TrackListReplaced`; each also invalidates `Tracks` in a `PropertiesChanged` of the track list interface. A metadata change of the current track while it stays current also sends `TrackMetadataChanged`. `GetTracksMetadata` returns the published metadata for the current entry; for the others it asks `MprisHooks::with_describe` (the application answers from the library, as the queue panel does) and falls back to the file name. `GoTo` plays a listed entry (`PlayerCommand::PlayEntry`). `CanEditTracks` is false: `AddTrack` and `RemoveTrack` do nothing.

### Playlists

Served only with `MprisHooks::with_playlists(source)`, where `source: PlaylistSource` lists the playlists (`MprisPlaylist`: number, name, creation and change times) and plays one. Ids are `/io/github/sorrelfox/Helikodu/Playlist/<number>`; icons are empty. `Orderings` are `Alphabetical`, `CreationDate`, `ModifiedDate` and `UserDefined` (the source's order; an unknown ordering falls back to it). `ActivePlaylist` is the playlist the queue came from (`ContextSource::Playlist`), announced when it changes. After the playlists change, `mpris.playlists_changed()` reads them again and announces renamed ones (`PlaylistChanged`), a new `PlaylistCount` and a changed active playlist.

## Suspend inhibition

`SuspendInhibition::system()` inhibits suspend while the player plays and releases it on pause, stop and the end of the queue; opening and seeking (`Buffering`) change nothing. `handle(&event)` follows one event, or `follow(subscription, on_error)` runs it on a thread of its own until the handle is dropped. `SystemInhibitor` asks the Inhibit portal (flag "suspend") and, outside a sandbox when no portal answers, takes a logind `sleep` lock in `block` mode on the system bus. The screen still blanks and locks. A failure is reported and tried again at the next change. Any `Inhibitor` can be used instead, such as a fake in tests.

## Appearance

`AppearanceMonitor::start(on_change)` reads `color-scheme`, `accent-color`, `contrast` and `reduced-motion` from the Settings portal and returns them as an `Appearance`; afterwards `on_change` gets the whole appearance after each change, on the monitor's thread. An accent colour outside 0–1 counts as none; `Rgb::to_hex` gives the CSS form. Without a portal it fails, and the front end keeps `Appearance::default()`. GTK 4.20 and newer follow the colour scheme on their own (4.24 also the accent colour); this reader covers older GTK and the parts that use the values directly.

## Tray icon

`Tray::start(TrayConfig, TrayHooks)` serves `org.kde.StatusNotifierItem` at `/StatusNotifierItem` under `org.kde.StatusNotifierItem-<pid>-<n>` and its menu as `com.canonical.dbusmenu` (version 3) at `/MenuBar`, and registers with `org.kde.StatusNotifierWatcher`. It registers again whenever a watcher appears (`NameOwnerChanged`), and `TrayHooks::with_host` is told after each change whether a host shows the icon (`IsStatusNotifierHostRegistered`, `StatusNotifierHost(Un)Registered`); with no watcher it reports `false`. The icon is `TrayConfig::icon_name` (the application's symbolic icon by default).

The front end gives everything shown with `set_content(TrayContent { tooltip, menu })`, so all text stays translated on its side: `TrayMenuItem::Text` (a line that cannot be chosen), `Action` (with `enabled` and an optional check state) and `Separator`. The menu is one level under the root (id 0); items are numbered from 1. A change of the menu sends `LayoutUpdated` with a new revision; a change of the tooltip sends `NewToolTip`; the same content again sends nothing. `Event(id, "clicked")` and `EventGroup` on an enabled action call `TrayHooks::new`'s hook with its `TrayAction`; `Activate` calls `with_activate`, vertical `Scroll` calls `with_scroll` with the host's delta. Underscores in labels are doubled, as dbusmenu reads one as an access key.

## Shortcuts in other applications

`GlobalShortcuts::start(on_event)` opens a session with the GlobalShortcuts portal (`CreateSession`) without asking anything; it fails where there is no portal. `bind(Vec<ShortcutSpec>)` hands over the shortcuts (`BindShortcuts`), which on most desktops asks the user once; the result is `ShortcutEvent::Bound(Vec<BoundShortcut>)` with the keys the desktop describes (`trigger_description`), or `Failed`. Presses arrive as `Activated(id)`, later changes made in the desktop's settings as `Bound`. `configure(activation_token)` opens the desktop's dialog again (`ConfigureShortcuts`, portal version 2, `can_configure()`). Everything runs on the session's own thread; dropping it closes the session.

## Screen lock and sleep

`SessionWatch::start(on_change)` reports `SessionChange::Locked`, `Unlocked`, `GoingToSleep` and `Woke`. The lock comes from the Inhibit portal's session monitor (`CreateMonitor`, `screensaver-active` in `StateChanged`; `QueryEndResponse` is answered) and from logind's session on the system bus (`Lock`, `Unlock` and the `LockedHint` property; the session is `GetSessionByPID`, else `GetSession("auto")`); both may report one lock, and only changes are passed on. Sleep comes from logind's `PrepareForSleep`. While `set_delay_sleep(true)`, a logind `delay` inhibitor lock on sleep is held, so that the system waits `SLEEP_GRACE` (400 ms) after `GoingToSleep` for the front end to pause; it is taken again after waking. A missing source is left out; the watch always starts. Inside the Flatpak the logind parts need `--system-talk-name=org.freedesktop.login1`.

## Open containing folder

`show_in_folder(path, activation_token)` asks the file manager to show the file selected (`org.freedesktop.FileManager1.ShowItems`), and falls back to the OpenURI portal's `OpenDirectory` (always inside a sandbox, where it needs no permission). It blocks while the file manager starts; `show_in_folder_later` runs it on a thread and reports the result to a callback. Pass the Wayland activation token so the file manager's window can take focus.

## What the GUI does with these

The single instance, "Open With", the desktop actions and notifications belong to `GApplication` on the GTK side:

- The `.desktop` file sets `DBusActivatable=true`, and the D-Bus service file starts `helikodu --gapplication-service`, which `GApplication` handles itself. Files arrive through `GApplication::open` (`HANDLES_OPEN`).
- The quick actions call the application actions `app.play-pause`, `app.next` and `app.previous`. Launchers activate them over D-Bus (`ActivateAction`), which may start the program without `activate`, so the actions must be added in `startup`.
- Notifications use `GNotification` (`GApplication::send_notification`), which picks the portal inside a sandbox.
- The GUI starts the tray icon (where the settings want it), the shortcuts in other applications and the lock and sleep watch for the window it shows, feeding their callbacks to the main context.
- The GUI starts `Mpris` with hooks whose `raise` and `quit` forward to the main context (they run on zbus's thread), feeds it the player's events and the now-playing details with the thumbnail file it shows in the deck, and starts `SuspendInhibition` and `AppearanceMonitor`.

## Device reservation

`src/reservation.rs` and `src/reservation/` implement `org.freedesktop.ReserveDevice1`, the session-bus protocol through which sound servers hand a sound card to a program that wants it exclusively. WirePlumber owns `org.freedesktop.ReserveDevice1.Audio<N>` (priority -20) for every card `N` it uses. `DeviceReserver::acquire(card, device_name)` serves the interface at `/org/freedesktop/ReserveDevice1/Audio<N>` (properties `ApplicationName` = `Helikodu`, `ApplicationDeviceName`, `Priority` = 10 by default), takes the name at once if nobody owns it, and otherwise sends `RequestRelease` to the owner and takes the name with `REPLACE_EXISTING`. A refusal is `ReserveError::Refused`, naming the holder's `ApplicationName`. The returned `ReservedDevice` answers every `RequestRelease` from others with `false` (WirePlumber asks right away) and notes it in `release_requested()`. Dropping it, or `release()`, gives the name back; WirePlumber then takes it and recreates the card's nodes within about 50 ms. A card is held for at least 250 ms: WirePlumber renames the card's nodes (`….2`) when it gets a card back while still letting go of it. `card_index` maps an ALSA card ID string to `N` through `/proc/asound/cards`. Each reservation has its own bus connection, so the bus frees the name even if the process dies. The calls block for a few D-Bus round trips; zbus runs its own small executor (`default-rt`), not tokio.

**Injecting it into the engine.** The engine (`helikodu-engine`) does not know D-Bus; it takes a `DeviceReservation` trait object in `EngineConfig::device_reservation` (a `SharedReservation`). Because this crate may not depend on the engine, the program that wires everything together (the application, which depends on `service` and `desktop`) writes a small adapter over the engine's types as re-exported by `helikodu-service`: a newtype around `DeviceReserver` whose `acquire(&ReservedCard)` calls `reserver.acquire(card.index, &card.device)`, wraps the `ReservedDevice` as `ReservationGuard::new(device, device.release_requested_flag())`, and maps `ReserveError::Refused { holder }` to `ReservationError::Refused { holder }` and every other error to `ReservationError::Failed(error.to_string())`. It then passes `SharedReservation::new(adapter)` to the service, which puts it in the engine configuration. Inside the Flatpak this needs `--own-name=org.freedesktop.ReserveDevice1.*` and `--talk-name=org.freedesktop.ReserveDevice1.*`.

**zbus and the offline policy.** zbus is built with only `default-rt` and `service`. Even so it pulls in `socket2` and compiles its TCP transport code; Helikodu only ever connects to the session bus's Unix socket, and the offline audit's seccomp filter still forbids network sockets at run time.

## Tests

```sh
cargo test -p helikodu-desktop
```

The unit tests cover the MPRIS mapping (metadata, capabilities, loop status, which changes are announced, seek detection), the cover export, file URIs, the appearance values and the suspend inhibition with a fake inhibitor. `tests/bus.rs` starts a private bus (`dbus-daemon --session`) for each test, and skips with a note where that is not possible: MPRIS is introspected, read and called by a client while a fake player drives it, the appearance monitor reads and follows a fake Settings portal, the tray icon registers with a fake watcher (also one that appears later and goes away) and its menu is read and clicked as a host does, the global shortcuts bind, report presses and open the configuration with a fake portal that answers through Request objects, and the lock and sleep watch follows a fake logind (Lock, LockedHint, PrepareForSleep, the delay lock taken again after waking) and a fake Inhibit portal's monitor. `tests/data_files.rs` checks the files in `data/` against the code and runs `desktop-file-validate` and `appstreamcli validate --no-net` when they are installed.

The reservation test against the real session bus is ignored by default. It takes a card from WirePlumber, opens its `hw:` device and writes 150 ms of digital silence, then checks that WirePlumber owns the card again and that all PipeWire nodes are back under their names. Taking a card removes all its nodes, inputs included, for that moment, so choose one nothing is using:

```sh
HELIKODU_RESERVE_TEST_DEVICE='hw:CARD=Generic_1,DEV=1' \
    cargo test -p helikodu-desktop --test reservation -- --ignored --nocapture
```
