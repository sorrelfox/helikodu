# fuzz

`cargo fuzz` targets and their seed inputs for the parsers and the FFmpeg bridge.

This folder is outside the Cargo workspace because fuzzing needs a nightly toolchain.

## Targets

| Target | What it exercises |
|---|---|
| `ffmpeg_bridge_open` | `helikodu-ffmpeg-bridge`: opening arbitrary bytes with every offline lock on, metadata, chapters, attached pictures, decoding and seeking |

```sh
cargo +nightly fuzz run ffmpeg_bridge_open
```

Good seed inputs are small files of every format the player opens; `cargo test -p helikodu-ffmpeg-bridge` leaves a set in `target/tmp/ffmpeg-bridge-media/`.
