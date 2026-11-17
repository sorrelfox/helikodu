# helikodu-desktop

MPRIS2, the application D-Bus API, notifications, suspend inhibition, the Settings and GlobalShortcuts portals, real-time priority, the tray icon and the audio device reservation helper. Nothing here is specific to one desktop environment.

- **May depend on:** `model`, `service`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Device reservation

`src/reservation.rs` and `src/reservation/` implement `org.freedesktop.ReserveDevice1`, the session-bus protocol through which sound servers hand a sound card to a program that wants it exclusively. WirePlumber owns `org.freedesktop.ReserveDevice1.Audio<N>` (priority -20) for every card `N` it uses. `DeviceReserver::acquire(card, device_name)` serves the interface at `/org/freedesktop/ReserveDevice1/Audio<N>` (properties `ApplicationName` = `Helikodu`, `ApplicationDeviceName`, `Priority` = 10 by default), takes the name at once if nobody owns it, and otherwise sends `RequestRelease` to the owner and takes the name with `REPLACE_EXISTING`. A refusal is `ReserveError::Refused`, naming the holder's `ApplicationName`. The returned `ReservedDevice` answers every `RequestRelease` from others with `false` (WirePlumber asks right away) and notes it in `release_requested()`. Dropping it, or `release()`, gives the name back; WirePlumber then takes it and recreates the card's nodes within about 50 ms. A card is held for at least 250 ms: WirePlumber renames the card's nodes (`….2`) when it gets a card back while still letting go of it. `card_index` maps an ALSA card ID string to `N` through `/proc/asound/cards`. Each reservation has its own bus connection, so the bus frees the name even if the process dies. The calls block for a few D-Bus round trips; zbus runs its own small executor (`default-rt`), not tokio.

**Injecting it into the engine.** The engine (`helikodu-engine`) does not know D-Bus; it takes a `DeviceReservation` trait object in `EngineConfig::device_reservation` (a `SharedReservation`). Because this crate may not depend on the engine, the program that wires everything together (the application, which depends on `service` and `desktop`) writes a small adapter over the engine's types as re-exported by `helikodu-service`: a newtype around `DeviceReserver` whose `acquire(&ReservedCard)` calls `reserver.acquire(card.index, &card.device)`, wraps the `ReservedDevice` as `ReservationGuard::new(device, device.release_requested_flag())`, and maps `ReserveError::Refused { holder }` to `ReservationError::Refused { holder }` and every other error to `ReservationError::Failed(error.to_string())`. It then passes `SharedReservation::new(adapter)` to the service, which puts it in the engine configuration. Inside the Flatpak this needs `--own-name=org.freedesktop.ReserveDevice1.*` and `--talk-name=org.freedesktop.ReserveDevice1.*`.

**zbus and the offline policy.** zbus is built with only `default-rt` and `service`. Even so it pulls in `socket2` and compiles its TCP transport code; Helikodu only ever connects to the session bus's Unix socket, and the offline audit's seccomp filter still forbids network sockets at run time.

## Tests

```sh
cargo test -p helikodu-desktop
```

The reservation test against the real session bus is ignored by default. It takes a card from WirePlumber, opens its `hw:` device and writes 150 ms of digital silence, then checks that WirePlumber owns the card again and that all PipeWire nodes are back under their names. Taking a card removes all its nodes, inputs included, for that moment, so choose one nothing is using:

```sh
HELIKODU_RESERVE_TEST_DEVICE='hw:CARD=Generic_1,DEV=1' \
    cargo test -p helikodu-desktop --test reservation -- --ignored --nocapture
```
