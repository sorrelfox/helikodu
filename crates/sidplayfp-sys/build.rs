// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiles the C++ shim and links libsidplayfp, found through pkg-config.

fn main() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-changed=shim/shim.cpp");
    println!("cargo:rerun-if-changed=shim/shim.h");
    // The link lines are printed after the shim's: with `--as-needed` (the
    // default on Debian and Ubuntu toolchains) a shared library named before
    // the static shim that uses it is dropped.
    let library = match pkg_config::Config::new()
        .atleast_version("2.0")
        .cargo_metadata(false)
        .probe("libsidplayfp")
    {
        Ok(library) => library,
        Err(error) => panic!(
            "\n\nlibsidplayfp 2.0 or newer was not found through pkg-config ({error}).\n\
             Install its development files (Fedora: libsidplayfp-devel, Debian and \
             Ubuntu: libsidplayfp-dev, Arch: libsidplayfp), or build without the `sid` \
             feature of helikodu-extra-decoders.\n\n"
        ),
    };
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++11")
        .file("shim/shim.cpp")
        .warnings(true);
    for path in &library.include_paths {
        build.include(path);
    }
    for (name, value) in &library.defines {
        build.define(name, value.as_deref());
    }
    build.compile("helikodu_sidplayfp_shim");
    for path in &library.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }
    for name in &library.libs {
        println!("cargo:rustc-link-lib={name}");
    }
}
