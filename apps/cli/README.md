# helikodu-cli

Builds `helikodu-cli`: remote control of the running application over D-Bus, and offline tools (scan, info, verify, convert, list-outputs) that use the core directly. It turns arguments into calls on the core and prints the results.

- **May depend on:** `model`, `service`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

The offline commands are in `src/offline/`; the remote control of the running application will be in `src/remote/`.

## Usage

```sh
helikodu-cli play FILE... [--backend auto|pipewire|alsa|null|wav] [--device ID]
                          [--mode shared|exclusive] [--volume 0..100] [--wav-out PATH]
helikodu-cli list-outputs [--backend auto|pipewire|alsa|null] [--json]
helikodu-cli --version
```

### play

Plays the files in the order given. Files of the same format join without a gap. A file that cannot be opened or decoded is reported and skipped.

| Option | Meaning |
|---|---|
| `--backend` | `auto` (the default: PipeWire if it is running, otherwise ALSA), `pipewire`, `alsa`, `null` (discards the audio) or `wav` |
| `--device` | The device's stable name, as `list-outputs` shows it: a PipeWire node name such as `alsa_output.usb-Topping_D10s-00.analog-stereo`, or an ALSA PCM such as `hw:CARD=D10s,DEV=0`. The default is the backend's default device |
| `--mode` | `shared` (the default) or `exclusive` |
| `--volume` | The position of the volume control, from 0 to 100 (the default). The control follows the cubic curve of desktop mixers |
| `--wav-out` | The file the `wav` backend writes; required with `--backend wav` and refused otherwise. The file is written as fast as the files decode |

When standard input is a terminal, these keys control playback, and a status line on standard error shows the file, position, volume and output:

| Key | Action |
|---|---|
| Space | Pause or resume |
| `n`, `p` | Next file; previous file (restarts the current one when more than three seconds of it have played) |
| ←, → | Seek back or forward 5 seconds |
| `+`, `-` | Volume up or down by 5 |
| `o` | Move playback to the next output device, through the devices of PipeWire and then of ALSA, without stopping |
| `q`, Esc, Ctrl+C | Quit |

Otherwise the files play to the end without reading input, and a lost device ends playback instead of pausing it.

Messages (the files as they start, skipped files, errors) go to standard error; `play` writes nothing to standard output.

### list-outputs

Lists the devices of every backend that plays sound (PipeWire and ALSA), or of the one given with `--backend`, on standard output. The default device is marked with `*`. With `--json` the output is one JSON array with an object per device: `backend`, `id`, `description`, `is_default` and `capabilities` (`sample_rates`, `formats`, `channel_counts`; empty when the backend cannot tell). A backend that cannot be listed is reported on standard error.

```sh
helikodu-cli list-outputs --backend pipewire --json | jq -r '.[].id'
```

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success. For `play`, at least one file played; skipped files are reported but do not change the code |
| 1 | A runtime error: the output could not be opened or failed, or no backend could be listed |
| 2 | A usage error: an unknown option or value, or options that do not go together |
| 3 | `play` could not play any of the files |

### Not yet available

Loudness normalization (`--replaygain track|album`) comes next, with the tag reader that supplies the gain values. Until then files play without ReplayGain.

## Tests

```sh
cargo test -p helikodu-cli
```

The tests run the built program against generated WAV files with the `null` and `wav` backends; they make no sound.
