// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Links libgme, found through pkg-config.

fn main() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    if let Err(error) = pkg_config::Config::new()
        .atleast_version("0.6.2")
        .probe("libgme")
    {
        panic!(
            "\n\nlibgme 0.6.2 or newer was not found through pkg-config ({error}).\n\
             Install its development files (Fedora: game-music-emu-devel, Debian and \
             Ubuntu: libgme-dev, Arch: libgme), or build without the `gme` feature of \
             helikodu-extra-decoders.\n\n"
        );
    }
}
