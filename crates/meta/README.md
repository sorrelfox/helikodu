# helikodu-meta

Tag reading and writing (with an FFmpeg fallback for formats the tag library does not cover), legacy charset repair, cover extraction, CUE and LRC parsing, playlist formats, naming templates, and the messages exchanged with scan workers.

- **May depend on:** `model`, `ffmpeg-bridge`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Tests

```sh
cargo test -p helikodu-meta
```
