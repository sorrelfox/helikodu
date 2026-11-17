# helikodu-library

The SQLite schema and its migrations, the scan coordinator and file watcher, the album and artist model, search, the query language and smart playlists, play statistics and the undo journal.

- **May depend on:** `model`, `meta`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Tests

```sh
cargo test -p helikodu-library
```
