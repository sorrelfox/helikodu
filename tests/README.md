# tests

Tests that need the built programs or a desktop session. A test of a single crate stays in that crate's own `tests/` folder.

| Folder | Content |
|---|---|
| `fixtures/` | Small committed samples and their expected results |
| `corpus/` | Recipes and checksums for the generated test corpus. The corpus itself is generated into `target/corpus/` and never committed |
| `system/` | Tests that run the built programs: the CLI, D-Bus and MPRIS, the offline audit, robustness and soak tests |
| `ui/` | Smoke, visual and frame-time tests of the desktop application |
