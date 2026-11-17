# tests

Tests that need the built programs or a desktop session. A test of a single crate stays in that crate's own `tests/` folder.

| Folder | Content |
|---|---|
| `fixtures/` | Small committed samples and their expected results |
| `corpus/` | Recipes and checksums for the generated test corpus. The corpus itself is generated into `target/corpus/` and never committed |
| `system/` | Tests that run the built programs: the CLI, D-Bus and MPRIS, the offline audit, robustness and soak tests |
| `ui/` | Smoke, visual and frame-time tests of the desktop application |

## Synthetic libraries

Benchmarks and UI performance tests use generated libraries of 100k and 500k one-second silent tracks rather than real music. `cargo xtask synth-library --tracks 100k` writes one into `target/synth-library/`; `--format opus` makes Opus files, `--names-only` writes only the track manifest (`tracks.jsonl`) for tests that fill the database directly. The same seed always gives the same library. See [xtask/README.md](../xtask/README.md).
