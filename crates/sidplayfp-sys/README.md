# helikodu-sidplayfp-sys

Raw bindings to libsidplayfp (Commodore 64 SID music) through a small C++ shim. No logic: the safe decoder lives in `extra-decoders`.

- **May depend on:** nothing in the workspace. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate may contain `unsafe` code. Every `unsafe` block needs a `// SAFETY:` comment that explains why it is sound.

## Building

libsidplayfp is a C++ library. `shim/shim.cpp` wraps what the player uses in C functions (`shim/shim.h`): loading a PSID/RSID tune from memory, its songs and text, the HVSC MD5, ROM images, selecting a song, rendering 16-bit stereo, and the song-length database (`Songlengths.md5`). Every shim function catches all C++ exceptions, so none reaches Rust. `build.rs` finds the library through pkg-config (`libsidplayfp`, 2.x; tested with 2.15) and compiles the shim with the `cc` crate, so a C++ compiler is needed (Fedora `gcc-c++`); the development files are Fedora `libsidplayfp-devel`, Debian and Ubuntu `libsidplayfp-dev`, Arch `libsidplayfp`.

With libsidplayfp 2.14 and newer the shim uses `play(cycles)` and `buffers()` and mixes the chips itself (one chip on both channels, two as left and right, the third in the centre), without the library mixer's random dither; with older versions it uses the deprecated `play(buffer, count)`. The power-on delay is fixed, so a song starts the same way every time.

## Tests

```sh
cargo test -p helikodu-sidplayfp-sys
```
