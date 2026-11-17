# helikodu-vgmstream-sys

Raw FFI bindings to the public C API of vgmstream (game audio streams). No logic: the safe decoder lives in `extra-decoders`.

- **May depend on:** nothing in the workspace. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate may contain `unsafe` code. Every `unsafe` block needs a `// SAFETY:` comment that explains why it is sound.

## Tests

```sh
cargo test -p helikodu-vgmstream-sys
```
