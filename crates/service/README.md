# helikodu-service

The player service (queue, play modes, session, statistics recording), the library facade the front ends query, the settings store, background jobs and the event bus. The GUI, the CLI and the D-Bus interfaces all go through this crate.

- **May depend on:** `model`, `engine`, `library`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

The Cargo features `pipewire`, `alsa` and `dsd` (all on by default) pass on to the engine's features of the same names; `bundled-sqlite` (off by default) passes on to the library's.

## Overview

| Module | What it holds |
|---|---|
| `service.rs` | `Service::start(ServiceConfig)`: settings, library, player and bus kept in step (playback settings reconfigure the player, the output device follows both ways) |
| `bus.rs`, `event.rs` | `EventBus`, `Subscription` (derefs to a `Receiver`), `Waker` for main loops; `Event` (`Player`, `Settings`, `Library`), `SubscribeOptions` (kinds of events, position throttling) |
| `clock.rs` | `Clock`, `SystemClock`, `FakeClock` for tests |
| `files.rs` | `AppDirs` (XDG config, state, data, cache), `write_atomic` |
| `settings.rs`, `settings/migrate.rs` | `SettingsStore` (`settings.toml`), `migrate` and `MIGRATIONS`, `applies`, `UiStateStore` (`ui-state.toml`) |
| `player.rs`, `player/` | The player service |
| `library.rs`, `library/` | `MusicLibrary` (blocking, typed queries) and `LibraryService` (worker thread, thumbnails, scans, watcher) |

## Player

`Player::spawn(PlayerConfig)` starts the service actor, a thread that owns the engine, the queue and the session, and returns the handle with a subscription to its events. `Player::send` (or a cloned `CommandSender`) takes `PlayerCommand`s; `Player::queue()` and `Player::status()` read the latest state without waiting for the actor; `Player::sync()` waits until earlier commands are handled and the session is saved.

| Command | What it does |
|---|---|
| `PlayFiles`, `PlayTracks { context, tracks, start }` | Replaces the queue (or appends, with `PlayerPrefs::append_contexts`) and plays |
| `PlayContext { context: ContextRequest, title, start }` | An album, artist, genre or search, looked up in the library |
| `Enqueue { tracks, at: Next \| End }`, `PlayEntry`, `Remove`, `Move`, `Clear` | Edit the queue; `Clear` keeps the playing entry |
| `PlayPause`, `Play`, `Pause`, `Stop`, `Next`, `Previous`, `Seek*` | Transport; `Previous` restarts after `PlayerPrefs::previous_restart` |
| `SetVolume`, `AdjustVolume`, `SetOutput`, `NextOutput` | Volume and output, switched while playing |
| `SetShuffle`, `SetRepeat`, `SetStopAfter`, `Configure` | Play modes and preferences |

Events (`PlayerEvent`): `State { playback, transport }`, `TrackChanged` (now playing), `Position`, `QueueChanged { version }`, `ModesChanged`, `StopAfterChanged`, `VolumeChanged`, `OutputChanged`, `DevicesChanged`, `Problem`, `Notice` (crash recovery, suspect skipped, context unavailable, session or listen not saved), `ListenRecorded`, `Ended`.

- **Queue.** Entries (`EntryId`, `TrackRef`, whether they came with the context) in play order, with the context order beside it. Shuffle stores its permutation, so "Previous" retraces what played; turning it off restores context order. With repeat-all and shuffle, each cycle plays every entry once in a new order. `QueueSnapshot::splice_from` gives the list-model change between two versions.
- **Gapless.** The next entry is named at `NearEnd`; when the queue, modes or stop-after change afterwards, the engine gets the new one.
- **Unplayable entries** are reported and skipped in the direction playback moved, and passed over for the rest of that queue.
- **Statistics.** With a library, each listen to a library track (also a file that is in the library) is measured from the engine's positions (seeks excluded) and recorded with `PlayRules` on a thread of its own: finished, skipped (moved on) or stopped. Long tracks start where they stopped when `resume_long_tracks` is on.
- **Session** (`SessionConfig`, `$XDG_STATE_HOME/helikodu`): `session.json` (queue, context order, position, modes, volume, output, suspect tracks) saved by atomic replace after changes (debounced), every 15 s while playing and at the end; restored paused. `playing.json` is the crash sentinel: it names what loads or plays and is removed when nothing plays and at a normal end. Found at the start, it adds a strike to each track it names and the session is restored paused with a notice; two strikes in a row make a track suspect, which automatic playback passes over until it plays to its end.

## Settings

`SettingsStore` reads `$XDG_CONFIG_HOME/helikodu/settings.toml` into `helikodu_model::settings::Settings` (tables `library`, `playback`, `output`, `loudness`, `appearance`, `advanced`, and `version`). Older files are migrated step by step before they are read; a broken file gives the defaults and is kept as `settings.toml.bak` when first replaced; a value out of range resets its table; a file from a newer version is read but never written. `update` validates, saves atomically and publishes `SettingsEvent::Changed` with the changed groups; `applies(key)` says whether a change is live or needs a restart (output format and loudness, until the engine can change them while running).

## Library

`MusicLibrary` opens the database (`open`, `open_existing`, `in_memory`, `from_library`) and offers browsing (`albums`, `album`, `artists`, `genres`, `tracks`, `all_tracks`), `search`, `track_info`, `file_info`, `track_source` (file and CUE span), statistics (`stats`, `record_listen`, `history`), `rate` (whole stars), `overview` and `scan`.

`LibraryService` runs the same queries on its own thread and hands each result to a callback; `describe` gives what the queue panel shows; `thumbnail` answers from the cache or schedules the artwork pool (`LibraryEvent::ThumbnailReady`); `scan`, `watch`, `rescan` announce progress as `LibraryEvent::Scan`, `Watch`, `ScanEnded` and `Changed`.

## Tests

```sh
cargo test -p helikodu-service
```

`tests/library.rs` fills a database file through the library crate and checks the facade: search with paths, track details and statistics, a library file that cannot be read, totals and the most played tracks, a missing library and a scan without folders. The level of ReplayGain playback is checked end to end by `apps/cli/tests/replaygain.rs`.

`tests/player/` covers the player through its public API: the queue, controls, play modes (stop-after, repeat, shuffle cycles, play next, remove, move, choose), the order of events and position thinning, the session saved and restored, the crash sentinel over three simulated abnormal ends, and statistics recorded in a library in memory with a fake clock. The queue model's own unit tests include property tests (`proptest`): every entry once per shuffled cycle, "Previous" retracing the history, and edits keeping the queue consistent. `tests/settings.rs` checks the settings store on disk and `tests/service.rs` the whole service.

The player tests play generated WAV files: to the WAV sink as fast as it takes them (the joins between files bit-exact, unplayable and missing files skipped, an empty queue), and in real time to the null sink (next, previous, pause, seek, volume, a move from the null sink to a WAV file while playing).

A test against the real sound servers is ignored by default. It never makes a sound:

```sh
# Needs a running PipeWire session with pipewire-pulse (`pactl`) and the ALSA `null`
# PCM. Loads two temporary PipeWire null sinks, moves playback of digital silence
# from the first to the ALSA `null` PCM and on to the second, and unloads the sinks.
cargo test -p helikodu-service --test output_switch -- --ignored
```
