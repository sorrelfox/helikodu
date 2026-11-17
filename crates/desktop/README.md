# helikodu-desktop

MPRIS2, the application D-Bus API, notifications, suspend inhibition, the Settings and GlobalShortcuts portals, real-time priority, the tray icon and the audio device reservation helper. Nothing here is specific to one desktop environment.

- **May depend on:** `model`, `service`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Tests

```sh
cargo test -p helikodu-desktop
```
