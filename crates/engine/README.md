# helikodu-engine

Decoder registry and gapless trimming, the render thread and processing path (conversion, gain, DSP, crossfade, resampling), the output backends (PipeWire, PulseAudio, ALSA, JACK, null, WAV) and the transport state machine. The engine plays media sources; it knows nothing about the library.

- **May depend on:** `model`, `ffmpeg-bridge`, `extra-decoders`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Output

`src/output.rs` defines the `OutputBackend` and `OutputStream` traits; each backend has its own file in `src/output/`. Opening a stream negotiates the device format and returns a `SampleWriter`, the producer side of a lock-free ring buffer (`rtrb`) of interleaved `f32` samples. The backend's real-time side only copies out of the ring buffer and converts to the device format; underruns are counted in atomics. Device changes, lost devices and failures arrive as `OutputEvent`s on a channel.

| Backend | Cargo feature | Device names |
|---|---|---|
| PipeWire | `pipewire` (default) | Node name, for example `alsa_output.usb-Topping_D10s-00.analog-stereo` |
| ALSA | `alsa` (default) | `hw:CARD=<card ID>,DEV=<n>`, and PCMs of the ALSA configuration such as `pipewire` |
| Null | always built | `null` |
| WAV file | always built | The file name |

Building the native backends needs the PipeWire and ALSA development files (`pkg-config` names `libpipewire-0.3` and `alsa`) and `libclang` for the PipeWire bindings. `--no-default-features` builds the crate without them.

The `alsa` crate is pinned to the version cpal uses (0.11 with cpal 0.18): two versions of `alsa-sys` cannot be linked into one program, so both are upgraded together.

Not done yet in the output layer:

- **ALSA exclusive access while PipeWire holds the card.** Taking the card through device reservation (`org.freedesktop.ReserveDevice1`, over D-Bus) is left for later, so this crate does not depend on a D-Bus library. Until then, a `hw:` device that the sound server holds fails with `OutputError::DeviceBusy`, naming the program that holds it.
- **Real-time priority for the ALSA writer thread** (rtkit or the Realtime portal). PipeWire's process callback already runs on PipeWire's real-time thread.
- **ALSA hot-plug through udev.** The ALSA backend polls `/proc/asound/cards` every two seconds instead.
- **Device capabilities on PipeWire.** The device list carries none yet; ALSA probes them from the hardware parameters.
- **PulseAudio and JACK** backends.

## Tests

```sh
cargo test -p helikodu-engine
```

The null and WAV sinks are tested end to end, including bit-exact WAV output.

Tests against real sound servers and hardware are ignored by default. They never make a sound:

```sh
# PipeWire: needs a running PipeWire session with pipewire-pulse (`pactl`). Each test
# loads a temporary null sink, plays digital silence into it by name, and unloads it.
cargo test -p helikodu-engine --test pipewire_output -- --ignored

# ALSA: needs sound cards. Lists devices, opens each free hw: device, negotiates a
# format and closes it again, without starting a stream or writing a sample.
cargo test -p helikodu-engine --test alsa_output -- --ignored
```

The PipeWire tests set the stream volume; the session manager remembers the last volume of the application "Helikodu".
