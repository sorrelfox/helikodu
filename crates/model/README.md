# helikodu-model

The vocabulary every other crate speaks: audio formats, channel layouts and devices; track, album, artist and playlist records; player commands, events and queue types; the settings schema; and the query tree. It does no I/O.

- **May depend on:** nothing in the workspace. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## What is in it now

| Module | Contents |
|---|---|
| `audio` | `SampleFormat` (integer, float and DSD, with byte order), `SampleRate`, `ChannelPosition` and `ChannelLayout` in the canonical WAVE order, `AudioSpec`, `MediaTime` (exact times as ticks at a tick rate), `OutputBackendKind`, `BackendSelection`, `DeviceId`, `DeviceSelection`, `DeviceKey`, `DeviceInfo`, `OutputMode`, `OutputSelection` |
| `library` | `TrackId`, `TrackRef`, `MediaSource` (a path kept as bytes, with an optional subsong or span) |
| `player` | The engine's `EngineCommand`s and `EngineEvent`s, `TransportState`, `PlaybackItem` (with the `Loudness` values of its tags and its album key) and `ItemId`, `Volume` (cubic curve), `EngineError`, the `SignalPath` of what is heard (source, stages with their `StageState`, `SignalDevice`, `SignalLamp`) and `PlaybackStats`; the queue's `EntryId`, `QueueEntry`, `QueueContext` (`ContextSource`), `EnqueueAt`, `ShuffleMode`, `RepeatMode`, `StopAfter`, `PlayModes`, `PlaybackState` |
| `settings` | `Settings` (versioned) with the tables `library`, `playback` (with `transitions`: crossfades and fade lengths), `output` (with per-device `DeviceProfile`s), `loudness`, `dsp` (equalizers with the ISO band frequencies and built-in presets, crossfeed, channel mix, limiter, resampler quality), `appearance` and `advanced`, each with defaults and validation (`SettingsError`); `SettingsGroup`, `Applies`; `UiState` (window size, pane widths and visibility) for `ui-state.toml` |

Album, artist and playlist records and the query tree are still to come.

Values that appear in `settings.toml` or on the command line have one text form, used by `Display`, `FromStr` and serde alike, for example `fixed:48000`, `hardware:PCM` or `alsa:hw:CARD=D10s,DEV=0`. File paths are never converted to strings: they are serialized as bytes, so names that are not valid UTF-8 survive.

## Tests

```sh
cargo test -p helikodu-model
```
