# Flatpak

The Flatpak manifest for `io.github.sorrelfox.Helikodu`, built on the GNOME runtime. Flathub builds run offline, so the Rust crates are vendored.

| File | What it is |
|---|---|
| `io.github.sorrelfox.Helikodu.yml` | The manifest |
| `cargo-sources.json` | The crates of `Cargo.lock` for the offline build; regenerate it whenever `Cargo.lock` changes |
| `helikodu-repo.gpg` | The public key of Helikodu's Flatpak repository, which signs the repository and the release's bundles |

How to build it, update it and why it has each permission: [docs/dev/flatpak.md](../../docs/dev/flatpak.md). Every release publishes it as a bundle on GitHub and in Helikodu's own Flatpak repository on the website ([docs/dev/flatpak.md](../../docs/dev/flatpak.md#the-flatpak-repository)).
