# helikodu-vgmstream-sys

Raw FFI bindings to the public C API of vgmstream (game audio streams). No logic: the safe decoder lives in `extra-decoders`.

- **May depend on:** nothing in the workspace. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate may contain `unsafe` code. Every `unsafe` block needs a `// SAFETY:` comment that explains why it is sound.

## Building

The bindings are hand-written from `libvgmstream.h` and `libvgmstream_streamfile.h` (public API 1.1); a unit test checks the struct layouts against the offsets a C compiler gives on 64-bit Linux. Few distributions package vgmstream, so the library is linked only with the `link` feature (which the `vgmstream` feature of `helikodu-extra-decoders` turns on), and only when `build.rs` finds it: through pkg-config (`libvgmstream`), or in the folder `VGMSTREAM_LIB_DIR` names. Otherwise the build prints a warning and every function is a stub that fails (`LINKED` is false), so `--all-features` builds work without the library. With `HELIKODU_REQUIRE_VGMSTREAM=1` a missing library fails the build, for packages that must ship the decoder (the Flatpak builds vgmstream from its sources with CMake and `BUILD_SHARED_LIBS=ON`).

## Tests

```sh
cargo test -p helikodu-vgmstream-sys
```
