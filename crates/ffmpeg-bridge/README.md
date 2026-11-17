# helikodu-ffmpeg-bridge

A thin safe layer over FFmpeg: custom I/O locked to local files, probing by content, audio stream selection, decoding, seeking, metadata, chapters and attached pictures, and later the encoders used by the converter. This is one of the few crates allowed to contain `unsafe`; every `unsafe` block carries a `SAFETY:` comment.

- **May depend on:** `model`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate may contain `unsafe` code. Every `unsafe` block needs a `// SAFETY:` comment that explains why it is sound.

## Tests

```sh
cargo test -p helikodu-ffmpeg-bridge
```
