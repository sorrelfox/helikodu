# helikodu-model

The vocabulary every other crate speaks: audio formats, channel layouts and devices; track, album, artist and playlist records; player commands, events and queue types; the settings schema; and the query tree. It does no I/O.

- **May depend on:** nothing in the workspace. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Tests

```sh
cargo test -p helikodu-model
```
