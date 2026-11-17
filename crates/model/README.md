# helikodu-model

The vocabulary every other crate speaks: audio formats, channel layouts and devices; track, album, artist and playlist records; player commands, events and queue types; the settings schema; and the query tree. It does no I/O.

- **May depend on:** nothing in the workspace. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## What is in it now

| Module | Contents |
|---|---|
| `audio` | `SampleFormat` (integer, float and DSD, with byte order), `SampleRate`, `ChannelPosition` and `ChannelLayout` in the canonical WAVE order, `AudioSpec`, `MediaTime` (exact times as ticks at a tick rate), `OutputBackendKind`, `BackendSelection`, `DeviceId`, `DeviceSelection`, `DeviceKey`, `DeviceInfo`, `OutputMode`, `OutputSelection` |
| `library` | `TrackId`, `TrackRef`, `MediaSource` (a path kept as bytes, with an optional subsong or span) |
| `player` | The engine's `EngineCommand`s and `EngineEvent`s, `TransportState`, `PlaybackItem` (with the `Loudness` values of its tags) and `ItemId`, `Volume` (cubic curve), `EngineError` |
| `settings` | `Settings` with the `[output]` table (`OutputSettings`, per-device `DeviceProfile`s), read from and written to `settings.toml` with defaults |

Album, artist and playlist records, the queue and the service's player commands, and the query tree are still to come.

Values that appear in `settings.toml` or on the command line have one text form, used by `Display`, `FromStr` and serde alike, for example `fixed:48000`, `hardware:PCM` or `alsa:hw:CARD=D10s,DEV=0`. File paths are never converted to strings: they are serialized as bytes, so names that are not valid UTF-8 survive.

## Tests

```sh
cargo test -p helikodu-model
```
