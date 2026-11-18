# Flatpak

The Flatpak manifest for `io.github.sorrelfox.Helikodu`, built on the GNOME runtime. Flathub builds run offline, so the Rust crates are vendored.

| File | What it is |
|---|---|
| `io.github.sorrelfox.Helikodu.yml` | The manifest |
| `cargo-sources.json` | The crates of `Cargo.lock` for the offline build; regenerate it whenever `Cargo.lock` changes |

How to build it, update it and why it has each permission: [docs/dev/flatpak.md](../../docs/dev/flatpak.md). Not published yet: the first versions are released only on GitHub.
