# helikodu-sidplayfp-sys

Raw bindings to libsidplayfp (Commodore 64 SID music) through a small C++ shim. No logic: the safe decoder lives in `extra-decoders`.

- **May depend on:** nothing in the workspace. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate may contain `unsafe` code. Every `unsafe` block needs a `// SAFETY:` comment that explains why it is sound.

## Tests

```sh
cargo test -p helikodu-sidplayfp-sys
```
