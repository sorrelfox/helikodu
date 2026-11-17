# helikodu-scan-worker

Builds `helikodu-scan-worker`, which reads the tags, properties, cover hashes and subsong lists of one file at a time over stdin and stdout. The scanner runs a pool of these processes with time and memory limits, so a decoder that crashes or hangs on a broken file cannot take the application down.

- **May depend on:** `model`, `meta`, `ffmpeg-bridge`, `extra-decoders`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Tests

```sh
cargo test -p helikodu-scan-worker
```
