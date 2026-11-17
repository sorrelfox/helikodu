# Contributing to Helikodu

Thank you for your interest in Helikodu. This guide explains how the project works and what a change needs before it can be merged.

By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Before you start

- **Bugs:** open an issue with the bug report template. Include the Helikodu version, your distribution, desktop and audio stack, and the steps that reproduce the problem.
- **Features:** open an issue first and describe the problem you want solved. Some features are out of scope by design: Helikodu has no online features (no streaming, radio, scrobbling or online metadata lookups), no plugin system, and never shows video.
- **Security problems:** do not open a public issue. Follow [SECURITY.md](SECURITY.md).

## Setting up

```sh
git clone https://github.com/sorrelfox/helikodu.git
cd helikodu
cargo build
cargo xtask ci
```

`cargo xtask ci` runs every check that CI runs: formatting, clippy, the tests, the crate dependency rules, `cargo deny` and the REUSE lint. Install [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) and [reuse](https://reuse.software) to run them all locally.

## Where code goes

The workspace is split into programs (`apps/`) and libraries (`crates/`). [crates/README.md](crates/README.md) describes each crate, the dependency rules between them and the shape every crate follows. The short version:

- Programs hold no domain logic; everything that can be tested lives in a library.
- `ui-gtk` talks to the core only through `service`.
- `unsafe` code is allowed only in `ffmpeg-bridge`, `extra-decoders` and the `-sys` binding crates, and every `unsafe` block carries a `// SAFETY:` comment.
- No dependency may open network connections. Helikodu is offline by construction, and `cargo deny` rejects network, TLS and DNS crates.
- A module is one file. There are no `mod.rs` files and no `utils` or `helpers` modules.

## Code style

- Format with `cargo fmt` and keep `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Libraries define error types with `thiserror`; only programs use `anyhow`.
- No allocation, locks or blocking calls in real-time audio callbacks.
- Every user-visible string is translatable, and new interface elements have accessible names and keyboard access.

## Licensing of contributions

Helikodu is licensed under GPL-3.0-or-later. Contributions are accepted under the same licence (inbound = outbound); there is no contributor licence agreement. You keep the copyright of your work.

Every new file carries an SPDX header, for example:

```rust
// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later
```

Documentation and artwork use CC-BY-SA-4.0. Files that cannot hold a comment are covered by `REUSE.toml`. Test samples and other data from elsewhere need their licence recorded before they are added. `reuse lint` checks all of this.

Add yourself to [AUTHORS](AUTHORS) with your first contribution, if you like.

## Commits and pull requests

- Keep each pull request focused on one change.
- Write commit subjects in the imperative mood ("Add the ALSA device list"), at most about 72 characters, followed by a blank line and a body that explains why the change is needed.
- Add tests for new behaviour and for bugs you fix.
- Update [CHANGELOG.md](CHANGELOG.md) under "Unreleased" for changes users will notice.
- `cargo xtask ci` must pass before a pull request is merged. The GitHub workflows run only for releases and when started by a maintainer, to save shared CI resources, so the local run is the check that counts.

## Translations

Translations use GNU gettext; see [po/README.md](po/README.md).
