# helikodu-testkit

Test helpers shared by the other crates. It is only ever a `[dev-dependencies]` entry; nothing that ships links it.

`longtail` writes small files of the long-tail formats byte by byte, for the tests of `extra-decoders`, `engine` and the scan worker: a ProTracker module with two subsongs, an NSF file with three tracks, a PSID tune with two songs, a MIDI file and a SoundFont with one sine preset. They are the project's own work under its licence. The file depends on `std` only, so `xtask corpus` includes it to write the same files into the corpus's `longtail` set.

- **May depend on:** `model`. `cargo xtask check-deps` enforces this.
- **Unsafe code:** This crate contains no `unsafe` code (`#![forbid(unsafe_code)]`).

## Tests

```sh
cargo test -p helikodu-testkit
```
