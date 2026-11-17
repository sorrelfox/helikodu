# helikodu-engine

Decoder registry and gapless trimming, the render thread and processing path (conversion, gain, DSP, crossfade, resampling), the output backends (PipeWire, PulseAudio, ALSA, JACK, null, WAV) and the transport state machine. The engine plays media sources; it knows nothing about the library.

- **May depend on:** `model`, `ffmpeg-bridge`, `extra-decoders`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Tests

```sh
cargo test -p helikodu-engine
```
