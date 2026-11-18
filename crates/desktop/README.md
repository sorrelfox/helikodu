# helikodu-desktop

MPRIS2, the application D-Bus API, notifications, suspend inhibition, the Settings and GlobalShortcuts portals, real-time priority, the tray icon and the audio device reservation helper. Nothing here is specific to one desktop environment.

- **May depend on:** `model`, `service`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

Everything here talks D-Bus through zbus with its own small executor (`default-rt`), not tokio; the XDG portals go through `ashpd` built the same way (`async-io`, only the Settings, Inhibit and OpenURI portals). Every connection is to the session bus's Unix socket, or to the system bus for the logind fallback.

| Module | What it does |
|---|---|
| `mpris.rs`, `mpris/` | MPRIS2 (`org.mpris.MediaPlayer2` and `.Player`) under `org.mpris.MediaPlayer2.io.github.sorrelfox.Helikodu` |
| `power.rs` | Suspend inhibition while playing: the Inhibit portal, logind outside a sandbox |
| `portals.rs` | Colour scheme, accent colour, contrast and reduced motion from the Settings portal |
| `open_folder.rs` | "Open containing folder": `org.freedesktop.FileManager1.ShowItems`, or the OpenURI portal |
| `reservation.rs`, `reservation/` | Audio device reservation (`org.freedesktop.ReserveDevice1`) |

## MPRIS

`Mpris::start(MprisConfig, MprisHooks)` serves both interfaces at `/org/mpris/MediaPlayer2` and takes the bus name (a second instance falls back to `….instance<pid>`, as the specification suggests). Client calls become `PlayerCommand`s through `MprisHooks::command` (`MprisHooks::for_player(player.sender())` sends them to the player); `Raise`, `Quit` and `OpenUri` go to optional hooks, and `CanRaise`/`CanQuit` say whether they are set. `OpenUri` accepts `file://` URIs only; without an `open` hook the file replaces the queue and plays.

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

`SupportedMimeTypes` is whatever `MprisConfig::supported_mime_types` holds; the application fills it from the decoder registry. `TrackList` and `Playlists` are not implemented yet (`HasTrackList` is false).

## Suspend inhibition

`SuspendInhibition::system()` inhibits suspend while the player plays and releases it on pause, stop and the end of the queue; opening and seeking (`Buffering`) change nothing. `handle(&event)` follows one event, or `follow(subscription, on_error)` runs it on a thread of its own until the handle is dropped. `SystemInhibitor` asks the Inhibit portal (flag "suspend") and, outside a sandbox when no portal answers, takes a logind `sleep` lock in `block` mode on the system bus. The screen still blanks and locks. A failure is reported and tried again at the next change. Any `Inhibitor` can be used instead, such as a fake in tests.

## Appearance

`AppearanceMonitor::start(on_change)` reads `color-scheme`, `accent-color`, `contrast` and `reduced-motion` from the Settings portal and returns them as an `Appearance`; afterwards `on_change` gets the whole appearance after each change, on the monitor's thread. An accent colour outside 0–1 counts as none; `Rgb::to_hex` gives the CSS form. Without a portal it fails, and the front end keeps `Appearance::default()`. GTK 4.20 and newer follow the colour scheme on their own (4.24 also the accent colour); this reader covers older GTK and the parts that use the values directly.

## Open containing folder

`show_in_folder(path, activation_token)` asks the file manager to show the file selected (`org.freedesktop.FileManager1.ShowItems`), and falls back to the OpenURI portal's `OpenDirectory` (always inside a sandbox, where it needs no permission). It blocks while the file manager starts; `show_in_folder_later` runs it on a thread and reports the result to a callback. Pass the Wayland activation token so the file manager's window can take focus.

## What the GUI does with these

The single instance, "Open With", the desktop actions and notifications belong to `GApplication` on the GTK side:

- The `.desktop` file sets `DBusActivatable=true`, and the D-Bus service file starts `helikodu --gapplication-service`, which `GApplication` handles itself. Files arrive through `GApplication::open` (`HANDLES_OPEN`).
- The quick actions call the application actions `app.play-pause`, `app.next` and `app.previous`. Launchers activate them over D-Bus (`ActivateAction`), which may start the program without `activate`, so the actions must be added in `startup`.
- Notifications use `GNotification` (`GApplication::send_notification`), which picks the portal inside a sandbox.
- The GUI starts `Mpris` with hooks whose `raise` and `quit` forward to the main context (they run on zbus's thread), feeds it the player's events and the now-playing details with the thumbnail file it shows in the deck, and starts `SuspendInhibition` and `AppearanceMonitor`.

## Device reservation

`src/reservation.rs` and `src/reservation/` implement `org.freedesktop.ReserveDevice1`, the session-bus protocol through which sound servers hand a sound card to a program that wants it exclusively. WirePlumber owns `org.freedesktop.ReserveDevice1.Audio<N>` (priority -20) for every card `N` it uses. `DeviceReserver::acquire(card, device_name)` serves the interface at `/org/freedesktop/ReserveDevice1/Audio<N>` (properties `ApplicationName` = `Helikodu`, `ApplicationDeviceName`, `Priority` = 10 by default), takes the name at once if nobody owns it, and otherwise sends `RequestRelease` to the owner and takes the name with `REPLACE_EXISTING`. A refusal is `ReserveError::Refused`, naming the holder's `ApplicationName`. The returned `ReservedDevice` answers every `RequestRelease` from others with `false` (WirePlumber asks right away) and notes it in `release_requested()`. Dropping it, or `release()`, gives the name back; WirePlumber then takes it and recreates the card's nodes within about 50 ms. A card is held for at least 250 ms: WirePlumber renames the card's nodes (`….2`) when it gets a card back while still letting go of it. `card_index` maps an ALSA card ID string to `N` through `/proc/asound/cards`. Each reservation has its own bus connection, so the bus frees the name even if the process dies. The calls block for a few D-Bus round trips; zbus runs its own small executor (`default-rt`), not tokio.

**Injecting it into the engine.** The engine (`helikodu-engine`) does not know D-Bus; it takes a `DeviceReservation` trait object in `EngineConfig::device_reservation` (a `SharedReservation`). Because this crate may not depend on the engine, the program that wires everything together (the application, which depends on `service` and `desktop`) writes a small adapter over the engine's types as re-exported by `helikodu-service`: a newtype around `DeviceReserver` whose `acquire(&ReservedCard)` calls `reserver.acquire(card.index, &card.device)`, wraps the `ReservedDevice` as `ReservationGuard::new(device, device.release_requested_flag())`, and maps `ReserveError::Refused { holder }` to `ReservationError::Refused { holder }` and every other error to `ReservationError::Failed(error.to_string())`. It then passes `SharedReservation::new(adapter)` to the service, which puts it in the engine configuration. Inside the Flatpak this needs `--own-name=org.freedesktop.ReserveDevice1.*` and `--talk-name=org.freedesktop.ReserveDevice1.*`.

**zbus and the offline policy.** zbus is built with only `default-rt` and `service`. Even so it pulls in `socket2` and compiles its TCP transport code; Helikodu only ever connects to the session bus's Unix socket, and the offline audit's seccomp filter still forbids network sockets at run time.

## Tests

```sh
cargo test -p helikodu-desktop
```

The unit tests cover the MPRIS mapping (metadata, capabilities, loop status, which changes are announced, seek detection), the cover export, file URIs, the appearance values and the suspend inhibition with a fake inhibitor. `tests/bus.rs` starts a private bus (`dbus-daemon --session`) for each test, and skips with a note where that is not possible: MPRIS is introspected, read and called by a client while a fake player drives it, and the appearance monitor reads and follows a fake Settings portal. `tests/data_files.rs` checks the files in `data/` against the code and runs `desktop-file-validate` and `appstreamcli validate --no-net` when they are installed.

The reservation test against the real session bus is ignored by default. It takes a card from WirePlumber, opens its `hw:` device and writes 150 ms of digital silence, then checks that WirePlumber owns the card again and that all PipeWire nodes are back under their names. Taking a card removes all its nodes, inputs included, for that moment, so choose one nothing is using:

```sh
HELIKODU_RESERVE_TEST_DEVICE='hw:CARD=Generic_1,DEV=1' \
    cargo test -p helikodu-desktop --test reservation -- --ignored --nocapture
```
