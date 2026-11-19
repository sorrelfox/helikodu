# helikodu-model

The vocabulary every other crate speaks: audio formats, channel layouts and devices; track, album, artist and playlist records; player commands, events and queue types; the settings schema; and the query tree. It does no I/O.

- **May depend on:** nothing in the workspace. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## What is in it now

| Module | Contents |
|---|---|
| `audio` | `SampleFormat` (integer, float and DSD, with byte order), `SampleRate`, `ChannelPosition` and `ChannelLayout` in the canonical WAVE order, `AudioSpec`, `MediaTime` (exact times as ticks at a tick rate), `OutputBackendKind`, `BackendSelection`, `DeviceId`, `DeviceSelection`, `DeviceKey`, `DeviceInfo`, `OutputMode`, `OutputSelection` |
| `analysis` | `TrackAnalysis` of a track (where its sound starts and ends, `SilentSpan`s inside it, the loudness of its first and last seconds, fade points) with its `LoudnessEnvelope`; the byte forms the library stores |
| `library` | `TrackId`, `TrackRef`, `MediaSource` (a path kept as bytes, with an optional subsong or span) |
| `player` | The engine's `EngineCommand`s and `EngineEvent`s, `TransportState`, `PlaybackItem` (with the `Loudness` values of its tags and its album key) and `ItemId`, `Volume` (cubic curve), `EngineError`, the `SignalPath` of what is heard (source, stages with their `StageState`, `SignalDevice`, `SignalLamp`) and `PlaybackStats`; the queue's `EntryId`, `QueueEntry`, `QueueContext` (`ContextSource`), `EnqueueAt`, `ShuffleMode`, `RepeatMode`, `StopAfter`, `PlayModes`, `PlaybackState` |
| `eq` | The equalizer's filter maths, shared by the engine and the front ends so that a drawn response matches the sound: the RBJ cookbook `Coefficients` of a biquad and its magnitude response, the graphic bands with overlap-corrected gains (`graphic`), `sections` of an `EqualizerSettings` for a rate and their summed `response_db` |
| `query` | The query tree shared by search, saved searches and smart playlists: `Query` (filter, `SortKey`s by `SortBy`, `Limit` in `LimitUnit`s), `Expr` (`All`, `Any`, `Not`, `Compare`, `Text`, `Has`, `Is`), `Comparison` of a `Field` (with its `FieldKind` and names) by an `Operator` with a `Value` (`Decimal`, lengths, sizes, `CalendarDate`, `Age`, `MediaKind`, ranges), `Presence`, `Flag`; `Comparison::check` (`TermProblem`) and `Expr::normalized`; `QuickFilter`, the predefined queries (unplayed, highly rated, unrated, recently added and played, forgotten, lossless, hi-res, loved, audiobooks). The text form is read and written by the library |
| `settings` | `Settings` (versioned) with the tables `library`, `playback` (with `transitions`: crossfades and fade lengths), `output` (with per-device `DeviceProfile`s), `loudness`, `dsp` (equalizers with the ISO band frequencies and built-in presets, crossfeed, channel mix, limiter, resampler quality), `appearance` and `advanced`, each with defaults and validation (`SettingsError`); `SettingsGroup`, `Applies`; `UiState` (window size, pane widths and visibility) for `ui-state.toml` |

Album, artist and playlist records are still to come.

Values that appear in `settings.toml` or on the command line have one text form, used by `Display`, `FromStr` and serde alike, for example `fixed:48000`, `hardware:PCM` or `alsa:hw:CARD=D10s,DEV=0`. File paths are never converted to strings: they are serialized as bytes, so names that are not valid UTF-8 survive.

## Tests

```sh
cargo test -p helikodu-model
```
