// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Links libopenmpt, found through pkg-config.

fn main() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    if let Err(error) = pkg_config::Config::new()
        .atleast_version("0.6")
        .probe("libopenmpt")
    {
        panic!(
            "\n\nlibopenmpt 0.6 or newer was not found through pkg-config ({error}).\n\
             Install its development files (Fedora: libopenmpt-devel, Debian and Ubuntu: \
             libopenmpt-dev, Arch: libopenmpt), or build without the `openmpt` feature of \
             helikodu-extra-decoders.\n\n"
        );
    }
}
