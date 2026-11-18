# helikodu-gme-sys

Raw FFI bindings to libgme (game console music). No logic: the safe decoder lives in `extra-decoders`.

- **May depend on:** nothing in the workspace. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate may contain `unsafe` code. Every `unsafe` block needs a `// SAFETY:` comment that explains why it is sound.

## Building

The bindings are hand-written from `gme/gme.h` (0.6.2 and newer, tested with 0.6.5): only the functions the player uses, and `gme_info_t` with its reserved fields. No `libclang` is needed. `build.rs` finds the library through pkg-config (`libgme`) and stops with the package names to install when it is missing (Fedora `game-music-emu-devel`, Debian and Ubuntu `libgme-dev`, Arch `libgme`).

## Tests

```sh
cargo test -p helikodu-gme-sys
```
