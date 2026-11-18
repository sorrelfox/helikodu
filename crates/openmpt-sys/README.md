# helikodu-openmpt-sys

Raw FFI bindings to libopenmpt (tracker modules). No logic: the safe decoder lives in `extra-decoders`.

- **May depend on:** nothing in the workspace. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate may contain `unsafe` code. Every `unsafe` block needs a `// SAFETY:` comment that explains why it is sound.

## Building

The bindings are hand-written from `libopenmpt/libopenmpt.h` (API 0.6 and newer, tested with 0.8.10): only the functions the player uses, with C enums as integers. No `libclang` is needed. `build.rs` finds the library through pkg-config (`libopenmpt`) and stops with the package names to install when it is missing (Fedora `libopenmpt-devel`, Debian and Ubuntu `libopenmpt-dev`, Arch `libopenmpt`).

## Tests

```sh
cargo test -p helikodu-openmpt-sys
```
