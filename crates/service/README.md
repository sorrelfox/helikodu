# helikodu-service

The player service (queue, play modes, session, statistics recording), the library facade the front ends query, the settings store, background jobs and the event bus. The GUI, the CLI and the D-Bus interfaces all go through this crate.

- **May depend on:** `model`, `engine`, `library`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

The Cargo features `pipewire`, `alsa` and `dsd` (all on by default) pass on to the engine's features of the same names; `bundled-sqlite` (off by default) passes on to the library's.

## Player

`src/player.rs` and `src/player/` hold the player service. `Player::spawn(PlayerConfig)` starts the service actor, a thread that owns the engine and the queue, and returns the handle and the receiver of `PlayerEvent`s. `Player::send` takes `PlayerCommand`s; every change of state goes through the actor's mailbox. Dropping the handle stops playback and ends the thread.

```rust
use helikodu_service::player::{Player, PlayerCommand, PlayerConfig, PlayerEvent};

let (player, events) = Player::spawn(PlayerConfig::new(selection))?;
player.send(PlayerCommand::PlayFiles { files, start: 0 })?;
for event in events.iter() {
    if let PlayerEvent::Ended { .. } = event {
        break;
    }
}
```

| File | What it holds |
|---|---|
| `player.rs` | `Player`: spawning, sending, shutting down |
| `player/actor.rs` | `PlayerCommand` and the actor: the queue, the engine's events, skipping |
| `player/config.rs` | `PlayerConfig`: the output settings built from a backend, device and mode (there is no settings file yet), the start volume, `SinkPacing`, `ReplayGain` (mode, preamps, clipping prevention) |
| `player/loudness.rs` | Reading the loudness tags of queued files and whether a file plays as part of its album |
| `player/event.rs` | `PlayerEvent`, `EndReason` |
| `player/outputs.rs` | `list_outputs`, `sound_backends`, `resolve_backend`, `next_output` |
| `player/queue.rs` | The queue of files |
| `player/error.rs` | `PlayerError` |

| Command | What it does |
|---|---|
| `PlayFiles { files, start }` | Replaces the queue and plays from `start` |
| `PlayPause`, `Play`, `Pause`, `Stop` | `Play` after a stop or the end plays the current file again |
| `Next`, `Previous` | `Next` past the last file ends playback; `Previous` restarts the current file after three seconds of it, and goes to the file before otherwise |
| `Seek`, `SeekForward`, `SeekBackward` | Within the current file |
| `SetVolume`, `AdjustVolume` | The volume control's position; `AdjustVolume` adds to it, within 0 to 1 |
| `SetOutput`, `NextOutput` | Moves playback to another backend, device or mode without stopping. `NextOutput` steps through the devices of PipeWire and then ALSA, or, while the null backend plays, stays on it |

| Event | When |
|---|---|
| `State` | The engine's transport changed state |
| `TrackChanged` | A file began to play: its index, the queue length, path, format and length |
| `Position` | A few times per second while playing, and after a seek |
| `VolumeChanged`, `OutputChanged` | After the commands that change them |
| `Problem` | An error, with the file it concerns if any. Playback goes on where it can |
| `Ended` | Playback stopped for good: `QueueFinished`, `Stopped` or `OutputFailed`, with the number of files played and failed |

**Gapless playback.** The next file is handed to the engine when it asks for one (`NearEnd` answered with `SetNext`), so files the engine can join play without a gap.

**Unplayable files.** A file that does not open is reported with `Problem` and skipped in the direction playback was moving: forward normally, backward after `Previous` (and forward again from the first file). A next file that does not open is skipped before it is reached. A file that fails while decoding ends there, and playback goes on with the next one.

**Loudness normalization.** `PlayerConfig::replay_gain` selects `ReplayGainMode::Off` (the default), `Track`, `Album` or `Auto`, with a preamp for tagged files, one for untagged files, and clipping prevention by the peak (on by default). When it is not off, each file's ReplayGain or Opus R128 tags are read (through the library crate, in this process) just before the file is handed to the engine, and go with it as the `PlaybackItem`'s `Loudness`. In `Auto` mode a file plays as part of its album, and so with the album gain, when the file before or after it in the queue has the same album title and album artists.

**Outputs.** `list_outputs(backend)` asks a backend for its devices. `next_output` chooses the device after the current one in a list; a current choice of the default device stands for the device the backend marks as default.

Not done yet in the player:

- Shuffle, repeat, stop-after, consume, enqueueing, and the session that keeps the queue across restarts.
- The settings store: the output settings are built from parameters.
- Statistics: plays and skips are not recorded.

## Library

`src/library.rs` and `src/library/` are the library facade for tools that use the database directly, such as `helikodu-cli scan`, `search`, `info` and `stats`.

| Item | What it does |
|---|---|
| `default_database_path` | `$XDG_DATA_HOME/helikodu/library.db`, or `~/.local/share/helikodu/library.db` |
| `MusicLibrary::open`, `open_existing` | Opens the database, creating it and its folder, or only if it exists (a newer schema opens read-only). The sort order follows `LC_ALL`, `LC_COLLATE` or `LANG` |
| `MusicLibrary::scan` | Starts a scan (`ScanRequest`: folders, or the library's own folders when empty; exclude patterns; worker count and program) and returns the `ScanHandle` |
| `MusicLibrary::search` | A page of results, each with its file path |
| `MusicLibrary::track_info`, `file_info`, `read_file_info` | `TrackInfo`: tags, audio properties, loudness values of the file's tags, and the library entry with statistics. A file is read directly; a library file that cannot be read any more is described from the library |
| `MusicLibrary::overview` | The library folders, totals and most played tracks |

The library crate's result types the front ends need (`SearchHit`, `ScanEvent`, `ScanSummary`, `TrackTags`, `TrackStats`, `LibraryTotals`, …) are re-exported from `helikodu_service::library`.

## Tests

```sh
cargo test -p helikodu-service
```

`tests/library.rs` fills a database file through the library crate and checks the facade: search with paths, track details and statistics, a library file that cannot be read, totals and the most played tracks, a missing library and a scan without folders. The level of ReplayGain playback is checked end to end by `apps/cli/tests/replaygain.rs`.

`tests/player/` plays generated WAV files: to the WAV sink as fast as it takes them (the joins between files bit-exact, unplayable and missing files skipped, an empty queue), and in real time to the null sink (next, previous, pause, seek, volume, a move from the null sink to a WAV file while playing).

A test against the real sound servers is ignored by default. It never makes a sound:

```sh
# Needs a running PipeWire session with pipewire-pulse (`pactl`) and the ALSA `null`
# PCM. Loads two temporary PipeWire null sinks, moves playback of digital silence
# from the first to the ALSA `null` PCM and on to the second, and unloads the sinks.
cargo test -p helikodu-service --test output_switch -- --ignored
```
