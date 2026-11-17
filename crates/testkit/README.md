# helikodu-testkit

Test helpers shared by the other crates: corpus and fixture lookup, PCM comparison and golden files. It is only ever a `[dev-dependencies]` entry; nothing that ships links it.

- **May depend on:** `model`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Tests

```sh
cargo test -p helikodu-testkit
```
