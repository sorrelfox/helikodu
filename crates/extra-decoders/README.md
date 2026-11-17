# helikodu-extra-decoders

Safe decoders over the binding crates (libopenmpt, libgme, vgmstream, libsidplayfp), the pure-Rust MIDI synthesizer and the DSF/DFF reader used for DoP output. Each decoder sits behind its own Cargo feature, so packagers can leave a library out.

- **May depend on:** `model` and the `-sys` binding crates. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate may contain `unsafe` code. Every `unsafe` block needs a `// SAFETY:` comment that explains why it is sound.

## Tests

```sh
cargo test -p helikodu-extra-decoders
```
