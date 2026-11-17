# helikodu-service

The player service (queue, play modes, session, statistics recording), the library facade the front ends query, the settings store, background jobs and the event bus. The GUI, the CLI and the D-Bus interfaces all go through this crate.

- **May depend on:** `model`, `engine`, `library`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

The Cargo features `pipewire`, `alsa` and `dsd` (all on by default) pass on to the engine's features of the same names.

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
| `player/config.rs` | `PlayerConfig`: the output settings built from a backend, device and mode (there is no settings file yet), the start volume, `SinkPacing` |
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

**Outputs.** `list_outputs(backend)` asks a backend for its devices. `next_output` chooses the device after the current one in a list; a current choice of the default device stands for the device the backend marks as default.

Not done yet in the player:

- Shuffle, repeat, stop-after, consume, enqueueing, and the session that keeps the queue across restarts.
- The settings store: the output settings are built from parameters.
- Loudness normalization (ReplayGain): it needs the tag reader of the metadata layer.
- Statistics: plays and skips are not recorded.

## Tests

```sh
cargo test -p helikodu-service
```

`tests/player/` plays generated WAV files: to the WAV sink as fast as it takes them (the joins between files bit-exact, unplayable and missing files skipped, an empty queue), and in real time to the null sink (next, previous, pause, seek, volume, a move from the null sink to a WAV file while playing).

A test against the real sound servers is ignored by default. It never makes a sound:

```sh
# Needs a running PipeWire session with pipewire-pulse (`pactl`) and the ALSA `null`
# PCM. Loads two temporary PipeWire null sinks, moves playback of digital silence
# from the first to the ALSA `null` PCM and on to the second, and unloads the sinks.
cargo test -p helikodu-service --test output_switch -- --ignored
```
